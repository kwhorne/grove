//! `grove bisect` — which commit broke this request?
//!
//! `git bisect` finds the commit that introduced a failure if you can tell it,
//! at each step, whether the failure is there. For a web app that is the hard
//! part: check out the commit, make the dependencies match it, bring the
//! database to its schema, run the request, look at the answer — seven or so
//! times, by hand. Grove owns the pieces, so it can do all of it:
//!
//! - the **request** comes from the timeline Grove already records (`grove
//!   requests`), replayed with its method, path, headers and body;
//! - the **checkout** is a detached worktree beside yours, served at
//!   `<site>--bisect.test`; your own checkout never moves;
//! - the **data** at each step is a fresh copy of your database, migrated to
//!   that commit, so one commit's migrations cannot leak into the next test;
//! - **dependencies** are refreshed with `composer install` only when
//!   `composer.lock` changed between steps;
//! - each step waits three seconds before the request, for OPcache to notice
//!   the new files — see `test_commit` for why it is three.
//!
//! A commit is good when the replay answers below 500, or exactly
//! `--expect-status` when given. One whose setup fails — a migration that does
//! not run — is skipped, which is what `git bisect skip` is for.
//!
//! Everything is taken down at the end, including when something fails.

use std::path::{Path, PathBuf};

use grove_core::paths::GrovePaths;
use grove_ipc::protocol::{Request, ResponseData};

use crate::tries::{
    call, clone_dir, get_env, rewrite_host, run, set_env, step, try_name, try_root,
};

/// One tested commit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Step {
    pub commit: String,
    pub subject: String,
    /// The replay's status, or `None` when the commit could not be set up.
    pub status: Option<u16>,
    pub verdict: &'static str,
}

/// What was found.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Outcome {
    pub first_bad: Option<String>,
    pub first_bad_subject: Option<String>,
    pub steps: Vec<Step>,
    pub note: Option<String>,
}

/// Is `status` what a working commit answers?
pub fn is_good(status: u16, expect: Option<u16>) -> bool {
    match expect {
        Some(want) => status == want,
        None => status < 500,
    }
}

/// The commit `git bisect` named as the first bad one, from its output.
pub fn first_bad_from(output: &str) -> Option<String> {
    output.lines().find_map(|l| {
        l.strip_suffix(" is the first bad commit")
            .map(|sha| sha.trim().to_string())
    })
}

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    grove_core::git::run(dir, args).map_err(|e| anyhow::anyhow!("git {}: {e}", args.join(" ")))
}

/// What has been set up, so it can all be taken down.
#[derive(Default)]
struct Made {
    worktree: Option<(PathBuf, PathBuf)>,
    database: Option<(String, String)>,
    linked: Option<String>,
    bisecting: bool,
}

async fn teardown(socket: &Path, made: &Made) {
    if let (true, Some((_, path))) = (made.bisecting, &made.worktree) {
        let _ = grove_core::git::run(path, &["bisect", "reset", "--quiet"]);
    }
    if let Some(name) = &made.linked {
        let _ = call(socket, Request::Unlink { name: name.clone() }).await;
    }
    if let Some((engine, database)) = &made.database {
        let _ = call(
            socket,
            Request::TryDatabaseDrop {
                engine: engine.clone(),
                database: database.clone(),
            },
        )
        .await;
    }
    if let Some((project, path)) = &made.worktree {
        let _ = grove_core::git::worktree_remove(project, path, true);
    }
}

/// `grove bisect`.
#[allow(clippy::too_many_arguments)]
pub async fn bisect(
    paths: &GrovePaths,
    site: String,
    good: String,
    bad: String,
    request: u64,
    expect: Option<u16>,
    json: bool,
) -> anyhow::Result<()> {
    let socket = paths.ipc_socket();
    let ResponseData::Sites(sites) = call(&socket, Request::ListSites).await? else {
        anyhow::bail!("unexpected response listing sites");
    };
    let main = sites
        .into_iter()
        .map(|s| s.site)
        .find(|s| s.name == site)
        .ok_or_else(|| anyhow::anyhow!("no site named {site:?}"))?;
    let project = main.path.clone();

    let ResponseData::RequestDetail(Some(detail)) =
        call(&socket, Request::RequestDetail { id: request }).await?
    else {
        anyhow::bail!("no captured request with id {request} — `grove requests` lists them");
    };
    let own_host =
        detail.host == main.hostname || detail.host.ends_with(&format!(".{}", main.hostname));
    if !own_host {
        anyhow::bail!(
            "request {request} was to {}, not to {} — pick one of this site's",
            detail.host,
            main.hostname
        );
    }
    for rev in [&good, &bad] {
        git(
            &project,
            &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
        )
        .map_err(|_| anyhow::anyhow!("{rev} is not a commit in {}", project.display()))?;
    }

    let name = try_name(&site, "bisect");
    let tld = main
        .hostname
        .strip_prefix(&format!("{}.", main.name))
        .unwrap_or("test")
        .to_string();
    let host = format!("{name}.{tld}");
    let path = try_root()?.join(&site).join("bisect");
    if path.exists() {
        anyhow::bail!(
            "a bisect of {site} is already set up at {} — remove it with `git worktree remove --force` \
             and `grove unlink {name}`",
            path.display()
        );
    }

    let mut made = Made::default();
    let result = drive(
        paths, &socket, &main, &project, &path, &host, &name, &good, &bad, request, expect, json,
        &mut made,
    )
    .await;
    step(json, "taking the bisect checkout down");
    teardown(&socket, &made).await;
    let outcome = result?;

    if json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
        return Ok(());
    }
    println!("{:<10} {:<6} {:<5} SUBJECT", "COMMIT", "STATUS", "");
    for s in &outcome.steps {
        println!(
            "{:<10} {:<6} {:<5} {}",
            &s.commit[..s.commit.len().min(10)],
            s.status
                .map(|c| c.to_string())
                .unwrap_or_else(|| "—".into()),
            s.verdict,
            s.subject
        );
    }
    match (&outcome.first_bad, &outcome.first_bad_subject) {
        (Some(sha), Some(subject)) => println!(
            "\nfirst bad commit: {} {subject}\n  git show {}",
            &sha[..sha.len().min(10)],
            &sha[..sha.len().min(10)]
        ),
        _ => println!(
            "\n{}",
            outcome
                .note
                .clone()
                .unwrap_or_else(|| "no single commit found".into())
        ),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn drive(
    paths: &GrovePaths,
    socket: &Path,
    main: &grove_core::site::ResolvedSite,
    project: &Path,
    path: &Path,
    host: &str,
    name: &str,
    good: &str,
    bad: &str,
    request: u64,
    expect: Option<u16>,
    json: bool,
    made: &mut Made,
) -> anyhow::Result<Outcome> {
    step(json, &format!("checking out {bad} beside your checkout"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    grove_core::git::worktree_add_detached(project, path, bad)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    made.worktree = Some((project.to_path_buf(), path.to_path_buf()));

    let env_src = project.join(".env");
    if env_src.exists() {
        let text = std::fs::read_to_string(&env_src)?;
        let (mut text, _) = rewrite_host(&text, &main.hostname, host);
        text = set_env(&text, "APP_URL", &format!("http://{host}"));
        if get_env(&text, "DB_CONNECTION").as_deref() == Some("sqlite") {
            if let Some(db) = get_env(&text, "DB_DATABASE").filter(|d| d.starts_with('/')) {
                let db = PathBuf::from(db);
                let inside = db
                    .strip_prefix(project)
                    .map(|rel| path.join(rel))
                    .unwrap_or_else(|_| {
                        path.join("database")
                            .join(db.file_name().unwrap_or_default())
                    });
                text = set_env(&text, "DB_DATABASE", &inside.to_string_lossy());
            }
        }
        grove_core::securefs::write_private(&path.join(".env"), &text)?;
    }
    for dir in ["vendor", "node_modules", "public/build"] {
        clone_dir(&project.join(dir), &path.join(dir))?;
    }
    call(
        socket,
        Request::Link {
            path: path.to_string_lossy().into_owned(),
            name: Some(name.to_string()),
        },
    )
    .await?;
    made.linked = Some(name.to_string());
    call(
        socket,
        Request::Isolate {
            name: name.to_string(),
            version: Some(main.php.clone()),
        },
    )
    .await?;

    let php = crate::mcp::resolve_php_cli(paths, &main.php)?;
    let composer = if path.join("composer.json").exists() {
        Some(
            grove_runtime::scaffold::ensure_composer(paths)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .to_string_lossy()
                .into_owned(),
        )
    } else {
        None
    };
    let ctx = Ctx {
        socket,
        project,
        path,
        host,
        php,
        composer,
        request,
        json,
    };
    let mut last_lock: Option<String> = None;
    let mut steps: Vec<Step> = Vec::new();

    // The bad end has to be bad, or there is nothing to find.
    let (commit, subject, status) = test_commit(&ctx, made, &mut last_lock).await?;
    let bad_really_bad = matches!(status, Some(s) if !is_good(s, expect));
    steps.push(Step {
        commit,
        subject,
        status,
        verdict: if bad_really_bad { "bad" } else { "good" },
    });
    if !bad_really_bad {
        return Ok(Outcome {
            first_bad: None,
            first_bad_subject: None,
            steps,
            note: Some(format!(
                "the request does not fail at {bad} (it answered {}), so there is nothing to bisect",
                status.map(|s| s.to_string()).unwrap_or_else(|| "nothing — setup failed".into())
            )),
        });
    }

    git(path, &["bisect", "start", bad, good])?;
    made.bisecting = true;
    for _ in 0..64 {
        let (commit, subject, status) = test_commit(&ctx, made, &mut last_lock).await?;
        let verdict = match status {
            None => "skip",
            Some(s) if is_good(s, expect) => "good",
            Some(_) => "bad",
        };
        steps.push(Step {
            commit,
            subject,
            status,
            verdict,
        });
        let said = git(path, &["bisect", verdict])?;
        if let Some(sha) = first_bad_from(&said) {
            let subject = git(path, &["log", "-1", "--format=%s", &sha]).unwrap_or_default();
            return Ok(Outcome {
                first_bad: Some(sha),
                first_bad_subject: Some(subject),
                steps,
                note: None,
            });
        }
        if said.contains("only 'skip'ped commits left") {
            return Ok(Outcome {
                first_bad: None,
                first_bad_subject: None,
                steps,
                note: Some(format!(
                    "the first bad commit is among commits that could not be set up:\n{said}"
                )),
            });
        }
    }
    Ok(Outcome {
        first_bad: None,
        first_bad_subject: None,
        steps,
        note: Some("stopped after 64 steps without an answer".into()),
    })
}

/// What every step needs, gathered once.
struct Ctx<'a> {
    socket: &'a Path,
    project: &'a Path,
    path: &'a Path,
    host: &'a str,
    php: PathBuf,
    composer: Option<String>,
    request: u64,
    json: bool,
}

/// One commit: fresh data, matching dependencies, its migrations, the
/// request. `None` for the status when the commit could not be set up.
async fn test_commit(
    ctx: &Ctx<'_>,
    made: &mut Made,
    last_lock: &mut Option<String>,
) -> anyhow::Result<(String, String, Option<u16>)> {
    let path = ctx.path;
    let commit = git(path, &["rev-parse", "HEAD"])?;
    let subject = git(path, &["log", "-1", "--format=%s"])?;
    step(
        ctx.json,
        &format!("testing {} {subject}", &commit[..10.min(commit.len())]),
    );

    if let Some((engine, database)) = made.database.take() {
        call(ctx.socket, Request::TryDatabaseDrop { engine, database }).await?;
    }
    let ResponseData::TryDatabase { engine, database } = call(
        ctx.socket,
        Request::TryDatabaseCreate {
            project: ctx.project.to_string_lossy().into_owned(),
            worktree: path.to_string_lossy().into_owned(),
            branch: "bisect".into(),
        },
    )
    .await?
    else {
        anyhow::bail!("unexpected response copying the database");
    };
    if engine != "none" {
        made.database = Some((engine.clone(), database.clone()));
    }
    if engine == "mysql" {
        let env = path.join(".env");
        let text = std::fs::read_to_string(&env)?;
        grove_core::securefs::write_private(&env, set_env(&text, "DB_DATABASE", &database))?;
    }

    if let Some(composer) = &ctx.composer {
        let lock = std::fs::read(path.join("composer.lock"))
            .map(|b| grove_core::checksum::sha256_hex(&b))
            .ok();
        if lock != *last_lock {
            let args = [
                composer.as_str(),
                "install",
                "--no-interaction",
                "--no-progress",
            ];
            if run(path, &ctx.php, &args, "composer install").is_err() {
                return Ok((commit, subject, None));
            }
            *last_lock = lock;
        }
    }
    if path.join("artisan").exists()
        && engine != "none"
        && run(
            path,
            &ctx.php,
            &["artisan", "migrate", "--force", "--no-interaction"],
            "migrate",
        )
        .is_err()
    {
        return Ok((commit, subject, None));
    }
    // OPcache re-checks a file's timestamp at most every `revalidate_freq`
    // (2) seconds, and it counts in whole seconds of request time: a script
    // cached just after a second ticked over is not looked at again until the
    // third second after. So the wait is three seconds, not two — at 2.1 s the
    // previous commit's compiled code answered, and bisect blamed a commit that
    // only changed a text file.
    tokio::time::sleep(std::time::Duration::from_millis(3100)).await;

    let ResponseData::Replayed { status, .. } = call(
        ctx.socket,
        Request::ReplayRequestAt {
            id: ctx.request,
            host: ctx.host.to_string(),
        },
    )
    .await?
    else {
        anyhow::bail!("unexpected response replaying the request");
    };
    Ok((commit, subject, Some(status)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_error_is_bad_unless_a_status_is_expected() {
        assert!(is_good(200, None));
        assert!(is_good(302, None));
        assert!(is_good(404, None), "a 404 is an answer, not a crash");
        assert!(!is_good(500, None));
        assert!(!is_good(200, Some(201)));
        assert!(is_good(201, Some(201)));
    }

    #[test]
    fn git_bisect_names_the_first_bad_commit_like_this() {
        let out = "3f2a9c0d8e7b6a5f4e3d2c1b0a9f8e7d6c5b4a39 is the first bad commit\n\
                   commit 3f2a9c0d8e7b6a5f4e3d2c1b0a9f8e7d6c5b4a39\nAuthor: t <t@t>\n";
        assert_eq!(
            first_bad_from(out).as_deref(),
            Some("3f2a9c0d8e7b6a5f4e3d2c1b0a9f8e7d6c5b4a39")
        );
        assert_eq!(first_bad_from("Bisecting: 3 revisions left to test"), None);
    }
}
