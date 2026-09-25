//! Which branch a project is on, read straight from `.git`.
//!
//! Grove follows a project's branch so its database can follow too. That needs
//! one fact, often — the daemon asks every second or so for every project that
//! opted in — so this reads `HEAD` as a file rather than running `git`. No
//! subprocess, no dependency on which `git` is on `PATH`, and no way for a
//! hook, an alias or a pager in someone's git config to get involved.
//!
//! ## What "on a branch" means here
//!
//! Only a symbolic `HEAD` — `ref: refs/heads/<name>` — counts. A detached
//! `HEAD` is reported as such and deliberately not turned into a branch name:
//! git detaches during a rebase, a bisect, and `git checkout <sha>`, and in all
//! three the right thing for a database that follows branches is to stay
//! exactly where it is until a real branch is checked out again. Treating a
//! detached `HEAD` as a branch would swap the database on every step of an
//! interactive rebase.

use std::path::{Path, PathBuf};

/// What `HEAD` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// Checked out on a local branch, by its short name (`feature/login`).
    Branch(String),
    /// A commit, not a branch: mid-rebase, mid-bisect, or a checked-out sha.
    Detached(String),
    /// No `.git` here, or one this could not read.
    NotARepo,
}

/// Where the repository's own files are for the checkout at `project`.
///
/// Usually `project/.git`. In a linked worktree or a submodule `.git` is a
/// *file* holding `gitdir: <path>`, and `HEAD` lives wherever that points — a
/// worktree's own `HEAD`, which is the one that matters, since two worktrees of
/// one repository sit on different branches.
pub fn git_dir(project: &Path) -> Option<PathBuf> {
    let dot_git = project.join(".git");
    let meta = std::fs::metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git);
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let path = Path::new(target);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        project.join(path)
    })
}

/// The branch — or lack of one — checked out at `project`.
pub fn head(project: &Path) -> Head {
    let Some(dir) = git_dir(project) else {
        return Head::NotARepo;
    };
    let Ok(text) = std::fs::read_to_string(dir.join("HEAD")) else {
        return Head::NotARepo;
    };
    parse_head(&text)
}

fn parse_head(text: &str) -> Head {
    let text = text.trim();
    if let Some(reference) = text.strip_prefix("ref:") {
        let reference = reference.trim();
        return match reference.strip_prefix("refs/heads/") {
            Some(name) if !name.is_empty() => Head::Branch(name.to_string()),
            // A symbolic ref to something other than a local branch is not a
            // branch anyone checked out; treat it like a detached `HEAD`.
            _ => Head::Detached(reference.to_string()),
        };
    }
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_hexdigit()) {
        return Head::Detached(text.to_string());
    }
    Head::NotARepo
}

/// Does the local branch `name` still exist?
///
/// For telling which parked databases belong to branches that have since been
/// deleted. A branch is either a loose ref file under `refs/heads/` or a line
/// in `packed-refs`, and git moves refs between the two at will, so both are
/// checked. In a worktree the refs live in the *common* directory, which the
/// worktree's `commondir` file names.
pub fn branch_exists(project: &Path, name: &str) -> bool {
    let Some(dir) = git_dir(project) else {
        return false;
    };
    let common = std::fs::read_to_string(dir.join("commondir"))
        .ok()
        .map(|c| {
            let c = PathBuf::from(c.trim());
            if c.is_absolute() {
                c
            } else {
                dir.join(c)
            }
        })
        .unwrap_or(dir);
    if common.join("refs/heads").join(name).is_file() {
        return true;
    }
    let wanted = format!("refs/heads/{name}");
    std::fs::read_to_string(common.join("packed-refs"))
        .map(|packed| {
            packed
                .lines()
                .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
                .any(|l| l.split_whitespace().nth(1) == Some(wanted.as_str()))
        })
        .unwrap_or(false)
}

/// Why a worktree could not be made or removed, in git's own words.
#[derive(Debug)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GitError {}

/// Run `git` in `repo`, returning stdout, or stderr as the error.
///
/// Unlike [`head`], which is read on a timer and so reads files, worktrees are
/// made once, on request, and git owns their bookkeeping — `.git/worktrees`,
/// the lock files, `commondir`. Re-implementing that would be the wrong kind of
/// cleverness, so this shells out.
fn git(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        // Never wait on a credential prompt nobody will see.
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| GitError(format!("could not run git: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(GitError(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

/// Check `branch` out into a new worktree at `path`.
///
/// A branch that exists only on `origin` — a colleague's, not yet pulled — is
/// fetched first, and git then creates the local tracking branch itself. A
/// branch that exists nowhere is an error rather than a new empty branch:
/// `grove try` is for trying something that exists.
pub fn worktree_add(repo: &Path, path: &Path, branch: &str) -> Result<(), GitError> {
    let local = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok();
    if !local {
        let remote = format!("refs/remotes/origin/{branch}");
        if git(repo, &["rev-parse", "--verify", "--quiet", &remote]).is_err() {
            git(repo, &["fetch", "origin", branch]).map_err(|e| {
                GitError(format!(
                    "{branch} is not a local branch, and fetching it from origin failed: {e}"
                ))
            })?;
        }
    }
    let path = path.to_string_lossy();
    git(repo, &["worktree", "add", &path, branch]).map(|_| ())
}

/// Make a new branch `branch` from the checkout's current commit, and check it
/// out into a new worktree at `path`. For a sandbox someone is about to work
/// in, rather than a branch that already exists. An existing branch of that
/// name is an error, not reused: the caller asked for a fresh one.
pub fn worktree_add_new(repo: &Path, path: &Path, branch: &str) -> Result<(), GitError> {
    if git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
    {
        return Err(GitError(format!("a branch named {branch} already exists")));
    }
    let path = path.to_string_lossy();
    git(repo, &["worktree", "add", "-b", branch, &path, "HEAD"]).map(|_| ())
}

/// Does a local branch `branch` exist in `repo`?
pub fn has_local_branch(repo: &Path, branch: &str) -> bool {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

/// Remove the worktree at `path`. Git refuses one with uncommitted changes
/// unless `force` — and that refusal is passed on, not overridden.
pub fn worktree_remove(repo: &Path, path: &Path, force: bool) -> Result<(), GitError> {
    let path = path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path);
    git(repo, &args).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("grove-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Against a real `git`: a worktree for an existing branch is made and
    /// reports its own branch; an unknown branch is refused rather than made;
    /// a dirty worktree is not removed without `force`.
    #[test]
    fn worktrees_are_made_refused_and_removed_like_git_does() {
        let root = scratch("wt-real");
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        run(&["init", "-q", "-b", "main"]);
        run(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ]);
        run(&["branch", "feature/x"]);

        let wt = root.join("try");
        worktree_add(&repo, &wt, "feature/x").unwrap();
        assert_eq!(head(&wt), Head::Branch("feature/x".into()));

        let err = worktree_add(&repo, &root.join("nope"), "does-not-exist").unwrap_err();
        assert!(err.0.contains("does-not-exist"), "{err}");

        // A fresh branch from HEAD, and a refusal to reuse a name.
        let fresh = root.join("fresh");
        worktree_add_new(&repo, &fresh, "agent/one").unwrap();
        assert_eq!(head(&fresh), Head::Branch("agent/one".into()));
        assert!(has_local_branch(&repo, "agent/one"));
        assert!(worktree_add_new(&repo, &root.join("again"), "agent/one").is_err());
        worktree_remove(&repo, &fresh, false).unwrap();
        assert!(
            has_local_branch(&repo, "agent/one"),
            "removing a worktree keeps its branch"
        );

        std::fs::write(wt.join("dirty.txt"), "x").unwrap();
        assert!(
            worktree_remove(&repo, &wt, false).is_err(),
            "a dirty worktree is kept"
        );
        worktree_remove(&repo, &wt, true).unwrap();
        assert!(!wt.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_symbolic_head_is_a_branch_including_one_with_a_slash() {
        assert_eq!(
            parse_head("ref: refs/heads/main\n"),
            Head::Branch("main".into())
        );
        assert_eq!(
            parse_head("ref: refs/heads/feature/login\n"),
            Head::Branch("feature/login".into())
        );
    }

    /// Rebase, bisect and `git checkout <sha>` all detach. A followed database
    /// must not move during any of them, so a sha must never become a branch.
    #[test]
    fn a_sha_is_detached_not_a_branch() {
        let sha = "3f2a9c0d8e7b6a5f4e3d2c1b0a9f8e7d6c5b4a39";
        assert_eq!(parse_head(sha), Head::Detached(sha.into()));
        assert_eq!(
            parse_head("ref: refs/remotes/origin/main"),
            Head::Detached("refs/remotes/origin/main".into())
        );
        assert_eq!(parse_head(""), Head::NotARepo);
        assert_eq!(parse_head("not a head file"), Head::NotARepo);
    }

    #[test]
    fn head_is_read_from_an_ordinary_checkout() {
        let dir = scratch("plain");
        std::fs::create_dir_all(dir.join(".git/refs/heads")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/develop\n").unwrap();
        assert_eq!(head(&dir), Head::Branch("develop".into()));
        assert_eq!(head(&dir.join("missing")), Head::NotARepo);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A linked worktree has its own `HEAD` — that is the whole point of one —
    /// reached through a `.git` *file*, and its branches live in the common
    /// directory. Reading the main checkout's `HEAD` here would put two
    /// worktrees on the same database.
    #[test]
    fn a_worktree_reports_its_own_branch_and_sees_the_shared_refs() {
        let root = scratch("worktree");
        let main = root.join("repo");
        let common = main.join(".git");
        std::fs::create_dir_all(common.join("refs/heads")).unwrap();
        std::fs::write(common.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(common.join("refs/heads/main"), "a".repeat(40)).unwrap();
        std::fs::write(
            common.join("packed-refs"),
            format!("# pack-refs\n{} refs/heads/feature/x\n", "b".repeat(40)),
        )
        .unwrap();

        let wt_git = common.join("worktrees/wt");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(wt_git.join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        let wt = root.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();

        assert_eq!(head(&main), Head::Branch("main".into()));
        assert_eq!(head(&wt), Head::Branch("feature/x".into()));
        // Loose in one place, packed in the other: both count.
        assert!(branch_exists(&wt, "main"));
        assert!(branch_exists(&wt, "feature/x"));
        assert!(branch_exists(&main, "feature/x"));
        assert!(!branch_exists(&main, "deleted"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
