use starship_daemon::ffi;
use starship_daemon::watch::WatcherState;

mod common;
use common::{TestRepo, assert_version_bumped};

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
fn ensure_creates_watcher_entry() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    let mut w = WatcherState::new();
    w.ensure(&p);
    assert_eq!(w.num_entries(), 1);
}

#[test]
fn unknown_repo_returns_zero_version() {
    let w = WatcherState::new();
    assert_eq!(w.version(&std::path::PathBuf::from("__nonexistent__")), 0);
}

#[test]
fn ensure_is_idempotent() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    let mut w = WatcherState::new();
    w.ensure(&p);
    w.ensure(&p);
    assert_eq!(w.num_entries(), 1);
}

#[test]
fn poll_increases_version_on_file_change() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    let mut w = WatcherState::new();
    w.ensure(&p);

    let v0 = w.version(&p);

    write_file(&repo, "trigger", "hello");
    settle_watcher();
    w.poll();
    assert!(
        w.version(&p) > v0,
        "version should increase after file write"
    );

    let v1 = w.version(&p);

    write_file(&repo, "trigger2", "world");
    settle_watcher();
    w.poll();
    assert!(
        w.version(&p) > v1,
        "version should increase again after second file write"
    );
}

#[test]
fn anchored_doublestar_rule_does_not_ignore_sibling_paths() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    repo.write(".gitignore", "a/**/b\n");
    std::fs::create_dir_all(p.join("x").join("a")).unwrap();

    let mut w = WatcherState::new();
    w.ensure(&p);

    repo.write("x/a/b", "hello");
    assert_version_bumped(&mut w, &p);
}

#[test]
fn anchored_doublestar_rule_suppresses_matching_paths() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    repo.write(".gitignore", "a/**/b\n");
    std::fs::create_dir_all(p.join("a").join("x")).unwrap();

    let mut w = WatcherState::new();
    w.ensure(&p);
    let v0 = w.version(&p);

    repo.write("a/x/b", "hello");
    for _ in 0..10 {
        w.poll();
        assert_eq!(
            w.version(&p),
            v0,
            "a/x/b must match anchored a/**/b and not bump"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    repo.write("visible.txt", "hello");
    assert_version_bumped(&mut w, &p);
}

#[test]
fn trailing_doublestar_does_not_ignore_bare_component() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    repo.write(".gitignore", "a/**\n");

    let mut w = WatcherState::new();
    w.ensure(&p);

    repo.write("a", "file named a at root");
    assert_version_bumped(&mut w, &p);
}

#[test]
fn cancel_io_is_needed_readdirectorychangesw_pending_at_drop() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    let mut w = WatcherState::new();
    w.ensure(&p);

    // After ensure, ReadDirectoryChangesW is submitted asynchronously.
    // change_event is a manual-reset event, initially unsignaled.
    // Since no change has occurred yet, the IO completion hasn't fired.
    // Overlapped IO is pending — CancelIoEx is required before CloseHandle.
    let rc = unsafe { ffi::WaitForSingleObject(w.change_event(0), 0) };

    assert_eq!(
        rc,
        ffi::WAIT_TIMEOUT,
        "ReadDirectoryChangesW unexpectedly completed immediately (rc={rc}). \
         IO should be pending. Without CancelIoEx, CloseHandle in drop races with it."
    );

    // w is dropped here. CancelIoEx + GetOverlappedResult ensures safety.
}
