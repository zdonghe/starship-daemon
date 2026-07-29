use std::path::Path;

use crate::cache::get_mtime_ns;

pub fn get_branch_ref_mtimes(git_dir: &Path) -> (u64, u64) {
    let head = git_dir.join("HEAD");
    let content = std::fs::read_to_string(&head).ok();
    let branch = content
        .and_then(|s| s.strip_prefix("ref: refs/heads/").map(|s| s.trim().to_string()));
    let branch_mtime = branch.as_ref()
        .map(|b| get_mtime_ns(&git_dir.join("refs").join("heads").join(b)))
        .unwrap_or(0);
    let remote_mtime = branch.as_ref()
        .map(|b| get_mtime_ns(&git_dir.join("refs").join("remotes").join("origin").join(b)))
        .unwrap_or(0);
    (branch_mtime, remote_mtime)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    fn init_repo(path: &Path) {
        fs::create_dir_all(path).unwrap();
        let git = |a: &[&str]| {
            let out = Command::new("git").arg("-C").arg(path).args(a).output().unwrap();
            assert!(out.status.success(), "git {} failed: {}", a.join(" "), String::from_utf8_lossy(&out.stderr));
        };
        git(&["init"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        fs::write(path.join("f"), "x").unwrap();
        git(&["add", "f"]);
        git(&["commit", "-m", "init"]);
    }

    #[test]
    fn detached_head_returns_zero_mtimes() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().join("r");
        init_repo(&repo);
        let git = |a: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&repo).args(a).output().unwrap();
            assert!(out.status.success());
        };
        git(&["checkout", "--detach"]);

        let git_dir = repo.join(".git");
        let (br, rr) = get_branch_ref_mtimes(&git_dir);
        assert_eq!(br, 0, "branch mtime should be 0 on detached HEAD");
        assert_eq!(rr, 0, "remote mtime should be 0 on detached HEAD");
    }

    #[test]
    fn on_branch_returns_nonzero_branch_mtime() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().join("r");
        init_repo(&repo);

        let git_dir = repo.join(".git");
        let (br, rr) = get_branch_ref_mtimes(&git_dir);
        assert!(br > 0, "branch mtime should be > 0 on a branch, got {br}");
        assert_eq!(rr, 0, "remote mtime should be 0 with no upstream");
    }

    #[test]
    fn branch_mtime_increases_after_commit() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().join("r");
        init_repo(&repo);
        let git = |a: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&repo).args(a).output().unwrap();
            assert!(out.status.success());
        };

        let git_dir = repo.join(".git");
        let (br_before, _) = get_branch_ref_mtimes(&git_dir);
        thread::sleep(Duration::from_millis(50));
        fs::write(repo.join("g"), "y").unwrap();
        git(&["add", "g"]);
        git(&["commit", "-m", "second"]);
        let (br_after, _) = get_branch_ref_mtimes(&git_dir);
        assert!(br_after > br_before, "branch mtime should increase after commit, was {br_before}, now {br_after}");
    }

    #[test]
    fn on_new_branch_returns_nonzero_mtime() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().join("r");
        init_repo(&repo);
        let git = |a: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&repo).args(a).output().unwrap();
            assert!(out.status.success());
        };
        git(&["checkout", "-b", "feature"]);

        let git_dir = repo.join(".git");
        let (br, rr) = get_branch_ref_mtimes(&git_dir);
        assert!(br > 0, "new branch mtime should be > 0, got {br}");
        assert_eq!(rr, 0);
    }
}
