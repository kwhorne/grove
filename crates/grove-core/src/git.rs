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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("grove-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
