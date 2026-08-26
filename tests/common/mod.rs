#![allow(dead_code)]

use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use starship_daemon::watch::WatcherState;

pub const SLEEP_MS: u64 = 15;

pub fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git command failed");
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

pub fn assert_version_bumped(w: &mut WatcherState, repo: &Path) {
    let before = w.version(repo);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        w.poll();
        if w.version(repo) > before {
            return;
        }
        thread::sleep(Duration::from_millis(20));
        if Instant::now() > deadline {
            panic!("repo version did not increase within 5s (before={before})");
        }
    }
}

pub fn settle() {
    thread::sleep(Duration::from_millis(SLEEP_MS));
}

pub fn settle_watcher() {
    thread::sleep(Duration::from_millis(200));
}

pub fn ensure_watcher(w: &mut WatcherState, repo: &Path) {
    w.ensure(repo);
    thread::sleep(Duration::from_millis(300));
    w.poll();
}

pub struct RemoteScaffold {
    pub bare: tempfile::TempDir,
    pub work: tempfile::TempDir,
    pub path: std::path::PathBuf,
}

pub fn remote_with_worktree(name: &str) -> RemoteScaffold {
    let bare = tempfile::TempDir::new().unwrap();
    let bare_path = bare.path().join("remote.git");
    std::fs::create_dir_all(&bare_path).unwrap();
    git(&bare_path, &["init", "--bare"]);

    let work = tempfile::TempDir::new().unwrap();
    let wt = work.path().join(name);
    std::fs::create_dir_all(&wt).unwrap();
    git(&wt, &["init"]);
    git(&wt, &["branch", "-M", "main"]);
    git(&wt, &["config", "user.email", "test@test"]);
    git(&wt, &["config", "user.name", "test"]);
    git(
        &wt,
        &["remote", "add", "origin", bare_path.to_str().unwrap()],
    );
    std::fs::write(wt.join("init.txt"), "init").unwrap();
    git(&wt, &["add", "init.txt"]);
    git(&wt, &["commit", "-m", "init"]);
    git(&wt, &["push", "-u", "origin", "main"]);

    RemoteScaffold {
        bare,
        work,
        path: wt,
    }
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
