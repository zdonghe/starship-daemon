use starship_daemon::ffi;
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

fn poll_and_process(w: &mut WatcherState) {
    w.poll();
    std::thread::sleep(std::time::Duration::from_millis(150));
    w.process_dirty();
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
fn poll_process_bumps_generation_on_file_change() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    let mut w = WatcherState::new();
    w.ensure(&p);

    write_file(&repo, "trigger", "hello");
    settle_watcher();
    poll_and_process(&mut w);
    let g1 = w.generation(&p);
    assert!(g1 > 1, "generation should have been bumped after file write, got {g1}");

    write_file(&repo, "trigger2", "world");
    settle_watcher();
    poll_and_process(&mut w);
    let g2 = w.generation(&p);
    assert!(g2 > g1, "generation should bump again after second file write, got {g2} vs {g1}");
}

#[test]
fn poll_process_detects_file_creation() {
    let repo = TestRepo::new();
    let p = repopath(&repo);

    let mut w = WatcherState::new();
    w.ensure(&p);

    write_file(&repo, "new.txt", "world");
    settle_watcher();
    poll_and_process(&mut w);
    let g1 = w.generation(&p);
    assert!(g1 > 1, "gen should have been bumped from file creation, got {g1}");

    write_file(&repo, "another.txt", "more");
    settle_watcher();
    poll_and_process(&mut w);
    let g2 = w.generation(&p);
    assert!(g2 > g1, "gen should have been bumped from second file creation, got {g2} vs {g1}");
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
    let rc = unsafe { ffi::WaitForSingleObject(w.entries[0].change_event, 0) };

    assert_eq!(rc, ffi::WAIT_TIMEOUT,
        "ReadDirectoryChangesW unexpectedly completed immediately (rc={rc}). \
         IO should be pending. Without CancelIoEx, CloseHandle in drop races with it.");

    // w is dropped here. CancelIoEx + GetOverlappedResult ensures safety.
}
