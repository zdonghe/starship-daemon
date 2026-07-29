use starship_daemon::watch::WatcherState;

mod common;
use common::TestRepo;

fn write_file(repo: &TestRepo, name: &str, content: &str) {
    repo.write(name, content);
}

fn repopath(repo: &TestRepo) -> std::path::PathBuf {
    repo.path().to_path_buf()
}

fn settle_watcher() {
    std::thread::sleep(std::time::Duration::from_millis(200));
}

#[test]
fn ensure_inserts_generation_on_success() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    let mut w = WatcherState::new();
    w.ensure(&p);
    assert_eq!(w.generation(&p), 1);
}

#[test]
fn unknown_repo_returns_zero() {
    let w = WatcherState::new();
    assert_eq!(w.generation(&std::path::PathBuf::from("__nonexistent__")), 0);
}

#[test]
fn ensure_is_idempotent() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    let mut w = WatcherState::new();
    w.ensure(&p);
    let gen1 = w.generation(&p);
    w.ensure(&p);
    assert_eq!(w.generation(&p), gen1);
    assert_eq!(w.entries.len(), 1);
}

#[test]
fn poll_bumps_generation_on_file_change() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    let mut w = WatcherState::new();
    w.ensure(&p);

    write_file(&repo, "trigger", "hello");
    settle_watcher();
    w.poll();
    let g1 = w.generation(&p);
    assert!(g1 > 1, "generation should have been bumped after file write, got {g1}");

    write_file(&repo, "trigger2", "world");
    settle_watcher();
    w.poll();
    let g2 = w.generation(&p);
    assert!(g2 > g1, "generation should bump again after second file write, got {g2} vs {g1}");
}

#[test]
fn poll_detects_file_creation() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    let mut w = WatcherState::new();
    w.ensure(&p);

    write_file(&repo, "new.txt", "world");
    settle_watcher();
    w.poll();
    let g1 = w.generation(&p);
    assert!(g1 > 1, "gen should have been bumped from file creation, got {g1}");

    write_file(&repo, "another.txt", "more");
    settle_watcher();
    w.poll();
    let g2 = w.generation(&p);
    assert!(g2 > g1, "gen should have been bumped from second file creation, got {g2} vs {g1}");
}
