#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

pub const SLEEP_MS: u64 = 15;

pub fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C").arg(repo)
        .args(args)
        .output()
        .expect("git command failed");
    assert!(out.status.success(), "git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
}

pub fn settle() {
    thread::sleep(Duration::from_millis(SLEEP_MS));
}

pub struct TestRepo {
    dir: tempfile::TempDir,
}

impl TestRepo {
    pub fn new() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = TestRepo { dir };
        settle();
        repo.git(&["init"]);
        repo.git(&["config", "user.email", "test@test"]);
        repo.git(&["config", "user.name", "test"]);
        repo.write("a.txt", "hello");
        repo.git(&["add", "a.txt"]);
        repo.git(&["commit", "-m", "initial"]);
        repo.git(&["branch", "other"]);
        settle();
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn git(&self, args: &[&str]) {
        git(self.path(), args);
    }

    pub fn write(&self, name: &str, content: &str) {
        std::fs::write(self.path().join(name), content).unwrap();
    }

    pub fn remove(&self, name: &str) {
        std::fs::remove_file(self.path().join(name)).unwrap();
    }

}

pub fn no_config() -> PathBuf {
    PathBuf::from("__nonexistent_config__")
}

pub fn current_branch(repo: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .expect("git rev-parse failed");
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
