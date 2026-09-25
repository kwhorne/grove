//! `grove try <branch>` — another branch of the same project, running beside
//! yours, without touching your checkout or your database.
//!
//! Reviewing a colleague's branch used to mean stashing, checking out,
//! migrating your own database into their schema, looking, then migrating back
//! and hoping. A try is a second checkout instead:
//!
//! 1. a git worktree of the branch, under `~/.grove/try/` — outside any parked
//!    directory, which would otherwise pick it up as a site of its own;
//! 2. its own `.env` (the file is gitignored, so a worktree has none), with the
//!    site's hostname and the database pointed at the try;
//! 3. its own copy of the database, made by the daemon — which is where the
//!    knowledge of which servers are Grove's own lives;
//! 4. `vendor/`, `node_modules/` and `public/build/` cloned from your checkout
//!    (on APFS a clone shares blocks until either side changes, so it takes
//!    seconds and no space), then `composer install` to catch up with the
//!    branch's lock file, and the branch's migrations;
//! 5. linked as `<site>--<branch>.test`, on the same PHP, secured if yours is.
//!
//! The name has a double hyphen and no dot on purpose: `feature.myapp.test`
//! would route to `myapp`, because Grove sends every subdomain of a site to it.
//!
//! `grove try --done <branch>` undoes all of it. The try's database is dropped
//! only because this command made it, and the worktree is removed the way git
//! removes one — refusing, unless `--force`, if it holds uncommitted work.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use grove_core::paths::GrovePaths;
use grove_ipc::client;
use grove_ipc::protocol::{Request, ResponseData};
use serde::{Deserialize, Serialize};

/// One try, as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TryRecord {
    /// The site it was made from.
    pub site: String,
    pub branch: String,
    /// The main checkout.
    pub project: PathBuf,
    /// The worktree.
    pub path: PathBuf,
    pub url: String,
    /// `mysql`, `sqlite` or `none`.
    pub engine: String,
    /// The schema name or the SQLite file.
    pub database: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TryState {
    #[serde(default)]
    tries: BTreeMap<String, TryRecord>,
}

fn state_path(paths: &GrovePaths) -> PathBuf {
    paths.base().join("tries.json")
}

fn load(paths: &GrovePaths) -> TryState {
    std::fs::read_to_string(state_path(paths))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(paths: &GrovePaths, state: &TryState) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(state)?;
    grove_core::securefs::write_public_atomic(&state_path(paths), json)?;
    Ok(())
}

/// A branch name as a hostname label: lowercase letters, digits and single
/// hyphens. `feature/Login_Page` becomes `feature-login-page`.
pub fn slug(branch: &str) -> String {
    let mut out = String::new();
    for c in branch.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// The try's site name: `<site>--<branch>`, within DNS's 63-character label
/// limit. A branch too long to fit is cut and given a short hash, so two long
/// branch names that share a prefix still get different sites.
pub fn try_name(site: &str, branch: &str) -> String {
    let full = format!("{site}--{}", slug(branch));
    if full.len() <= 63 {
        return full;
    }
    let hash = &grove_core::checksum::sha256_hex(branch.as_bytes())[..6];
    let room = 63usize.saturating_sub(site.len() + 2 + 7);
    let cut: String = slug(branch).chars().take(room).collect();
    format!("{site}--{}-{hash}", cut.trim_end_matches('-'))
}

/// Replace `old_host` with `new_host` wherever it appears as a hostname in the
/// `.env` — `APP_URL`, `SESSION_DOMAIN`, `SANCTUM_STATEFUL_DOMAINS`, a
/// `VITE_*` URL — including as the tail of a subdomain (`api.myapp.test`) and
/// never inside a longer name (`notmyapp.test`). Returns the text and how many
/// were replaced.
pub fn rewrite_host(text: &str, old_host: &str, new_host: &str) -> (String, usize) {
    let is_name = |c: char| c.is_ascii_alphanumeric() || c == '-';
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    let mut rest = text;
    while let Some(i) = rest.find(old_host) {
        let before = rest[..i].chars().next_back();
        let after = rest[i + old_host.len()..].chars().next();
        let bounded =
            !before.is_some_and(is_name) && !after.is_some_and(|c| is_name(c) || c == '.');
        out.push_str(&rest[..i]);
        if bounded {
            out.push_str(new_host);
            count += 1;
        } else {
            out.push_str(old_host);
        }
        rest = &rest[i + old_host.len()..];
    }
    out.push_str(rest);
    (out, count)
}

/// Set `key` in a `.env`'s text, keeping every other line exactly as it was.
/// Appended when absent; quoted only when the value needs it.
pub fn set_env(text: &str, key: &str, value: &str) -> String {
    let rendered = if value
        .chars()
        .any(|c| c.is_whitespace() || c == '#' || c == '"')
    {
        format!(
            "{key}=\"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        format!("{key}={value}")
    };
    let mut found = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| {
            let bare = line
                .trim_start()
                .strip_prefix("export ")
                .unwrap_or(line.trim_start());
            if !found && bare.split('=').next().map(str::trim) == Some(key) && bare.contains('=') {
                found = true;
                rendered.clone()
            } else {
                line.to_string()
            }
        })
        .collect();
    if !found {
        lines.push(rendered);
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') || !found {
        out.push('\n');
    }
    out
}

/// The value of `key` in a `.env`'s text, unquoted.
fn get_env(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let bare = line
            .trim_start()
            .strip_prefix("export ")
            .unwrap_or(line.trim_start());
        let (k, v) = bare.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').trim_matches('\'').to_string())
    })
}

/// Clone `from` to `to`: copy-on-write where the filesystem can (APFS on
/// macOS, btrfs/XFS on Linux), a plain copy where it cannot.
fn clone_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.exists() || to.exists() {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let cow = if cfg!(target_os = "macos") {
        std::process::Command::new("cp")
            .arg("-cR")
            .arg(from)
            .arg(to)
            .status()
    } else {
        std::process::Command::new("cp")
            .args(["-R", "--reflink=auto"])
            .arg(from)
            .arg(to)
            .status()
    };
    if matches!(cow, Ok(s) if s.success()) {
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(to);
    let status = std::process::Command::new("cp")
        .arg("-R")
        .arg(from)
        .arg(to)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "copying {} failed",
            from.display()
        )))
    }
}

async fn call(socket: &Path, req: Request) -> anyhow::Result<ResponseData> {
    let resp = client::send(socket, &req)
        .await
        .context("talking to the Grove daemon (is it running? `grove start`)")?;
    if !resp.ok {
        anyhow::bail!("{}", resp.error.unwrap_or_else(|| "request failed".into()));
    }
    resp.data
        .ok_or_else(|| anyhow::anyhow!("no data in response"))
}

fn step(json: bool, msg: &str) {
    if !json {
        eprintln!("  {msg}");
    }
}

/// Run a command in `dir`, and fail with the tail of what it printed.
fn run(dir: &Path, program: &Path, args: &[&str], what: &str) -> anyhow::Result<()> {
    let out = std::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("running {what}"))?;
    if !out.status.success() {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let tail: Vec<&str> = text
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        anyhow::bail!("{what} failed:\n    {}", tail.join("\n    "));
    }
    Ok(())
}

fn try_root() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".grove").join("try"))
}

/// What has been made so far, so a failure half-way can be unwound.
#[derive(Default)]
struct Made {
    worktree: Option<(PathBuf, PathBuf)>,
    database: Option<(String, String)>,
    linked: Option<String>,
}

async fn unwind(socket: &Path, made: &Made) {
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

/// Make a try of `branch`, reporting each step to `progress`, and return it —
/// or the one already running for that branch, with `true`.
///
/// Writes nothing to stdout: the MCP server calls this, and its stdout is the
/// protocol. `new_branch` makes a fresh branch from the checkout's current
/// commit instead of checking out one that exists — what an agent that is
/// about to work needs, where a reviewer wants an existing one.
pub async fn create(
    paths: &GrovePaths,
    site: &str,
    branch: &str,
    new_branch: bool,
    progress: &(dyn Fn(&str) + Sync),
) -> anyhow::Result<(TryRecord, bool)> {
    let (site, branch) = (site.to_string(), branch.to_string());
    let socket = paths.ipc_socket();
    let mut state = load(paths);
    let name = try_name(&site, &branch);
    if let Some(existing) = state.tries.get(&name) {
        return Ok((existing.clone(), true));
    }

    let ResponseData::Sites(sites) = call(&socket, Request::ListSites).await? else {
        anyhow::bail!("unexpected response listing sites");
    };
    let main = sites
        .into_iter()
        .map(|s| s.site)
        .find(|s| s.name == site)
        .ok_or_else(|| anyhow::anyhow!("no site named {site:?}"))?;
    let project = main.path.clone();
    if grove_core::git::git_dir(&project).is_none() {
        anyhow::bail!("{} is not a git checkout", project.display());
    }
    let tld = main
        .hostname
        .strip_prefix(&format!("{}.", main.name))
        .unwrap_or("test")
        .to_string();
    let new_host = format!("{name}.{tld}");
    let scheme = if main.secure { "https" } else { "http" };
    let url = format!("{scheme}://{new_host}");
    let path = try_root()?.join(&site).join(slug(&branch));

    let mut made = Made::default();
    let result: anyhow::Result<TryRecord> = async {
        progress(&format!("checking out {branch} into {}", path.display()));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if new_branch {
            grove_core::git::worktree_add_new(&project, &path, &branch)
        } else {
            grove_core::git::worktree_add(&project, &path, &branch)
        }
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        made.worktree = Some((project.clone(), path.clone()));

        // `.env` is gitignored, so the worktree has none: start from yours,
        // with the hostname moved to the try's.
        let env_src = project.join(".env");
        if env_src.exists() {
            let text = std::fs::read_to_string(&env_src)?;
            let (mut text, moved) = rewrite_host(&text, &main.hostname, &new_host);
            text = set_env(&text, "APP_URL", &url);
            // An absolute SQLite path would still point at your database.
            if get_env(&text, "DB_CONNECTION").as_deref() == Some("sqlite") {
                if let Some(db) = get_env(&text, "DB_DATABASE").filter(|d| d.starts_with('/')) {
                    let db = PathBuf::from(db);
                    let inside = db
                        .strip_prefix(&project)
                        .map(|rel| path.join(rel))
                        .unwrap_or_else(|_| {
                            path.join("database")
                                .join(db.file_name().unwrap_or_default())
                        });
                    text = set_env(&text, "DB_DATABASE", &inside.to_string_lossy());
                }
            }
            grove_core::securefs::write_private(&path.join(".env"), &text)?;
            progress(&format!(
                ".env copied, {moved} mention(s) of {} moved to {new_host}",
                main.hostname
            ));
        }

        progress("copying the database");
        let ResponseData::TryDatabase { engine, database } = call(
            &socket,
            Request::TryDatabaseCreate {
                project: project.to_string_lossy().into_owned(),
                worktree: path.to_string_lossy().into_owned(),
                branch: branch.clone(),
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
        progress(&format!("database: {engine} {database}"));

        progress("cloning vendor/, node_modules/ and public/build/ from your checkout");
        for dir in ["vendor", "node_modules", "public/build"] {
            clone_dir(&project.join(dir), &path.join(dir))?;
        }

        let php = crate::mcp::resolve_php_cli(paths, &main.php)?;
        if path.join("composer.json").exists() {
            progress("composer install (catching up with the branch's lock file)");
            let composer = grove_runtime::scaffold::ensure_composer(paths)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let composer = composer.to_string_lossy().into_owned();
            run(
                &path,
                &php,
                &[
                    &composer,
                    "install",
                    "--no-interaction",
                    "--prefer-dist",
                    "--no-progress",
                ],
                "composer install",
            )?;
        }
        if path.join("artisan").exists() && engine != "none" {
            progress("running the branch's migrations");
            run(
                &path,
                &php,
                &["artisan", "migrate", "--force", "--no-interaction"],
                "php artisan migrate",
            )?;
        }

        progress(&format!("linking {new_host}"));
        call(
            &socket,
            Request::Link {
                path: path.to_string_lossy().into_owned(),
                name: Some(name.clone()),
            },
        )
        .await?;
        made.linked = Some(name.clone());
        call(
            &socket,
            Request::Isolate {
                name: name.clone(),
                version: Some(main.php.clone()),
            },
        )
        .await?;
        if main.secure {
            call(
                &socket,
                Request::Secure {
                    name: name.clone(),
                    enable: true,
                },
            )
            .await?;
        }
        Ok(TryRecord {
            site: site.clone(),
            branch: branch.clone(),
            project: project.clone(),
            path: path.clone(),
            url: url.clone(),
            engine,
            database,
        })
    }
    .await;

    match result {
        Ok(record) => {
            state.tries.insert(name, record.clone());
            save(paths, &state)?;
            Ok((record, false))
        }
        Err(e) => {
            progress("undoing what was set up");
            unwind(&socket, &made).await;
            Err(e)
        }
    }
}

/// `grove try <branch>`.
pub async fn start(
    paths: &GrovePaths,
    site: String,
    branch: String,
    new_branch: bool,
    json: bool,
) -> anyhow::Result<()> {
    let report = |msg: &str| step(json, msg);
    let (record, existed) = create(paths, &site, &branch, new_branch, &report).await?;
    let msg = if existed {
        format!("{branch} is already running as a try: {}", record.url)
    } else {
        format!(
            "{branch} is running at {}\n  code:     {}\n  database: {} {}\n  done:     grove try --done {branch}",
            record.url,
            record.path.display(),
            record.engine,
            record.database
        )
    };
    crate::output::print_message(&msg, json);
    Ok(())
}

/// `grove try --list`.
pub fn list(paths: &GrovePaths, json: bool) -> anyhow::Result<()> {
    let state = load(paths);
    if json {
        println!("{}", serde_json::to_string_pretty(&state.tries)?);
        return Ok(());
    }
    if state.tries.is_empty() {
        println!("no tries — `grove try <branch>` in a project starts one");
    }
    for t in state.tries.values() {
        let here = if t.path.exists() {
            ""
        } else {
            "  (worktree missing)"
        };
        println!("{:<28} {:<40} {}{here}", t.branch, t.url, t.path.display());
    }
    Ok(())
}

/// Every try that is recorded.
pub fn all(paths: &GrovePaths) -> Vec<TryRecord> {
    load(paths).tries.into_values().collect()
}

/// `grove try --done <branch>`.
pub async fn done(
    paths: &GrovePaths,
    site: String,
    branch: String,
    force: bool,
    json: bool,
) -> anyhow::Result<()> {
    remove(paths, &site, &branch, force).await?;
    crate::output::print_message(
        &format!("{branch}'s try is gone: site, database and worktree"),
        json,
    );
    Ok(())
}

/// Take a try down and return what it was. The branch itself — and any
/// commits on it — stays in the repository.
pub async fn remove(
    paths: &GrovePaths,
    site: &str,
    branch: &str,
    force: bool,
) -> anyhow::Result<TryRecord> {
    let (site, branch) = (site.to_string(), branch.to_string());
    let socket = paths.ipc_socket();
    let mut state = load(paths);
    let name = try_name(&site, &branch);
    let Some(t) = state.tries.get(&name).cloned() else {
        anyhow::bail!(
            "{branch} is not a try of {site} — `grove try --list` shows the ones there are"
        );
    };
    // The worktree first: if git refuses because of uncommitted work, nothing
    // else has been taken away yet.
    if t.path.exists() {
        grove_core::git::worktree_remove(&t.project, &t.path, force).map_err(|_| {
            // Git's own message names the directory, not what is in it. Say
            // which files would be lost, which is the thing to decide on.
            let changed = std::process::Command::new("git")
                .arg("-C")
                .arg(&t.path)
                .args(["status", "--porcelain"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            let list: Vec<&str> = changed.lines().take(10).collect();
            anyhow::anyhow!(
                "the try has uncommitted work, so nothing was removed:\n    {}\n  commit or stash it in {}, or pass --force to throw it away",
                list.join("\n    "),
                t.path.display()
            )
        })?;
    }
    let _ = call(&socket, Request::Unlink { name: name.clone() }).await;
    if t.engine == "mysql" {
        call(
            &socket,
            Request::TryDatabaseDrop {
                engine: t.engine.clone(),
                database: t.database.clone(),
            },
        )
        .await?;
    }
    state.tries.remove(&name);
    save(paths, &state)?;
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_become_hostname_labels() {
        assert_eq!(slug("feature/Login_Page"), "feature-login-page");
        assert_eq!(slug("fix--double//slash"), "fix-double-slash");
        assert_eq!(slug("-leading-and-trailing-"), "leading-and-trailing");
        assert_eq!(try_name("shop", "feature/x"), "shop--feature-x");
    }

    /// A DNS label is at most 63 characters, and two long branches sharing a
    /// prefix must not collapse into one site.
    #[test]
    fn long_branch_names_still_fit_and_stay_distinct() {
        let a = try_name("abonnementsoversikt", &format!("{}-one", "x".repeat(80)));
        let b = try_name("abonnementsoversikt", &format!("{}-two", "x".repeat(80)));
        assert!(a.len() <= 63 && b.len() <= 63, "{a} {b}");
        assert_ne!(a, b);
        assert!(!a.contains('.'), "a dot would route to the main site");
    }

    #[test]
    fn the_hostname_moves_everywhere_it_appears_and_nowhere_else() {
        let env = "APP_URL=https://shop.test\nSESSION_DOMAIN=.shop.test\n\
                   SANCTUM_STATEFUL_DOMAINS=shop.test,localhost\nAPI=https://api.shop.test/v1\n\
                   OTHER=https://notshop.test\nMORE=https://shop.test.example.com\n";
        let (out, n) = rewrite_host(env, "shop.test", "shop--x.test");
        assert_eq!(n, 4, "{out}");
        assert!(out.contains("APP_URL=https://shop--x.test\n"));
        assert!(out.contains("SESSION_DOMAIN=.shop--x.test\n"));
        assert!(out.contains("SANCTUM_STATEFUL_DOMAINS=shop--x.test,localhost\n"));
        assert!(out.contains("API=https://api.shop--x.test/v1\n"));
        assert!(
            out.contains("OTHER=https://notshop.test\n"),
            "a longer name is not ours"
        );
        assert!(out.contains("MORE=https://shop.test.example.com\n"));
    }

    #[test]
    fn setting_one_key_keeps_every_other_line() {
        let env = "# comment\nAPP_NAME=\"My App\"\nDB_DATABASE=shop\n\nexport X=1\n";
        let out = set_env(env, "DB_DATABASE", "shop__gt_1234abcd");
        assert_eq!(
            out,
            "# comment\nAPP_NAME=\"My App\"\nDB_DATABASE=shop__gt_1234abcd\n\nexport X=1\n"
        );
        let out = set_env(env, "APP_URL", "https://a b");
        assert!(out.ends_with("APP_URL=\"https://a b\"\n"), "{out}");
        assert_eq!(get_env("A=\"q\"\n", "A").as_deref(), Some("q"));
    }
}
