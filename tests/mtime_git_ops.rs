use std::thread;
use std::time::Duration;

use starship_daemon::cache;
use starship_daemon::watch::WatcherState;

mod common;
use common::*;

fn ensure_watcher(w: &mut WatcherState, repo: &std::path::Path) {
    w.ensure(repo);
    thread::sleep(Duration::from_millis(300));
    w.poll();
}

// ==== Version bump tests ====

#[test]
fn bumps_on_file_create() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    ensure_watcher(&mut w, r.path());
    r.write("new_file.txt", "data");
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_file_delete() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("temp.txt", "data");
    ensure_watcher(&mut w, r.path());
    r.remove("temp.txt");
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_git_add() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("unstaged.txt", "data");
    ensure_watcher(&mut w, r.path());
    r.git(&["add", "unstaged.txt"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_commit() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("b.txt", "world");
    r.git(&["add", "b.txt"]);
    ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["commit", "-m", "second commit"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_checkout() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    ensure_watcher(&mut w, r.path());
    r.git(&["checkout", "other"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_checkout_new_branch() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    ensure_watcher(&mut w, r.path());
    r.git(&["checkout", "-b", "feature"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_git_rm() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("toremove.txt", "delete me");
    r.git(&["add", "toremove.txt"]);
    r.git(&["commit", "-m", "add toremove.txt"]);
    ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["rm", "toremove.txt"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_git_mv() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("old.txt", "rename me");
    r.git(&["add", "old.txt"]);
    r.git(&["commit", "-m", "add old.txt"]);
    ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["mv", "old.txt", "new.txt"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_stash_push() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("stash.txt", "original");
    r.git(&["add", "stash.txt"]);
    r.git(&["commit", "-m", "add stash.txt"]);
    r.write("stash.txt", "modified");
    ensure_watcher(&mut w, r.path());
    r.git(&["stash", "push"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_reset_hard() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("b.txt", "second");
    r.git(&["add", "b.txt"]);
    r.git(&["commit", "-m", "second"]);
    r.write("a.txt", "modified");
    r.git(&["add", "a.txt"]);
    ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["reset", "--hard", "HEAD~1"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_revert() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("revertable.txt", "will be reverted");
    r.git(&["add", "revertable.txt"]);
    r.git(&["commit", "-m", "to revert"]);
    ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["revert", "--no-edit", "HEAD"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_commit_amend() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    r.git(&["commit", "--amend", "-m", "amended initial"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_rebase() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    let base_branch = current_branch(r.path());
    r.git(&["checkout", "-b", "rebase-feature"]);
    r.write("feature.txt", "feature work");
    r.git(&["add", "feature.txt"]);
    r.git(&["commit", "-m", "feature commit"]);
    r.git(&["checkout", &base_branch]);
    r.write("mainline.txt", "mainline work");
    r.git(&["add", "mainline.txt"]);
    r.git(&["commit", "-m", "mainline commit"]);
    ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["rebase", "rebase-feature"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_cherry_pick() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("cherry.txt", "cherry content");
    r.git(&["add", "cherry.txt"]);
    r.git(&["commit", "-m", "commit to cherry-pick"]);
    let commit_hash = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(r.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    r.git(&["checkout", "other"]);
    ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["cherry-pick", &commit_hash]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_merge() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    let base_branch = current_branch(r.path());
    r.git(&["checkout", "other"]);
    r.write("feature.txt", "feature work");
    r.git(&["add", "feature.txt"]);
    r.git(&["commit", "-m", "feature work"]);
    r.git(&["checkout", &base_branch]);
    r.write("mainline.txt", "mainline work");
    r.git(&["add", "mainline.txt"]);
    r.git(&["commit", "-m", "mainline work"]);
    ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["merge", "other", "--no-edit"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_fetch() {
    let bare = tempfile::TempDir::new().unwrap();
    let bare_path = bare.path().join("remote.git");
    std::fs::create_dir_all(&bare_path).unwrap();
    git(&bare_path, &["init", "--bare"]);

    let a_dir = tempfile::TempDir::new().unwrap();
    let wt_a = a_dir.path().join("a");
    std::fs::create_dir_all(&wt_a).unwrap();
    git(&wt_a, &["init"]);
    git(&wt_a, &["branch", "-M", "main"]);
    git(&wt_a, &["config", "user.email", "test@test"]);
    git(&wt_a, &["config", "user.name", "test"]);
    git(
        &wt_a,
        &["remote", "add", "origin", bare_path.to_str().unwrap()],
    );
    std::fs::write(wt_a.join("init.txt"), "init").unwrap();
    git(&wt_a, &["add", "init.txt"]);
    git(&wt_a, &["commit", "-m", "init"]);
    git(&wt_a, &["push", "-u", "origin", "main"]);

    let b_dir = tempfile::TempDir::new().unwrap();
    let wt_b = b_dir.path().join("b");
    std::fs::create_dir_all(&wt_b).unwrap();
    git(&wt_b, &["init"]);
    git(&wt_b, &["branch", "-M", "main"]);
    git(&wt_b, &["config", "user.email", "test@test"]);
    git(&wt_b, &["config", "user.name", "test"]);
    git(
        &wt_b,
        &["remote", "add", "origin", bare_path.to_str().unwrap()],
    );
    git(
        &wt_b,
        &["pull", "origin", "main", "--allow-unrelated-histories"],
    );
    std::fs::write(wt_b.join("new.txt"), "new data").unwrap();
    git(&wt_b, &["add", "new.txt"]);
    git(&wt_b, &["commit", "-m", "second commit"]);
    git(&wt_b, &["push", "origin", "main"]);

    settle();

    let mut w = WatcherState::new();
    ensure_watcher(&mut w, &wt_a);

    git(&wt_a, &["fetch"]);
    assert_version_bumped(&mut w, &wt_a);
}

#[test]
fn bumps_on_pull() {
    let bare = tempfile::TempDir::new().unwrap();
    let bare_path = bare.path().join("remote.git");
    std::fs::create_dir_all(&bare_path).unwrap();
    git(&bare_path, &["init", "--bare"]);

    let a_dir = tempfile::TempDir::new().unwrap();
    let wt_a = a_dir.path().join("a");
    std::fs::create_dir_all(&wt_a).unwrap();
    git(&wt_a, &["init"]);
    git(&wt_a, &["branch", "-M", "main"]);
    git(&wt_a, &["config", "user.email", "test@test"]);
    git(&wt_a, &["config", "user.name", "test"]);
    git(
        &wt_a,
        &["remote", "add", "origin", bare_path.to_str().unwrap()],
    );
    std::fs::write(wt_a.join("init.txt"), "init").unwrap();
    git(&wt_a, &["add", "init.txt"]);
    git(&wt_a, &["commit", "-m", "init"]);
    git(&wt_a, &["push", "-u", "origin", "main"]);

    let b_dir = tempfile::TempDir::new().unwrap();
    let wt_b = b_dir.path().join("b");
    std::fs::create_dir_all(&wt_b).unwrap();
    git(&wt_b, &["init"]);
    git(&wt_b, &["branch", "-M", "main"]);
    git(&wt_b, &["config", "user.email", "test@test"]);
    git(&wt_b, &["config", "user.name", "test"]);
    git(
        &wt_b,
        &["remote", "add", "origin", bare_path.to_str().unwrap()],
    );
    git(
        &wt_b,
        &["pull", "origin", "main", "--allow-unrelated-histories"],
    );
    std::fs::write(wt_b.join("new.txt"), "new data").unwrap();
    git(&wt_b, &["add", "new.txt"]);
    git(&wt_b, &["commit", "-m", "second commit"]);
    git(&wt_b, &["push", "origin", "main"]);

    settle();

    let mut w = WatcherState::new();
    ensure_watcher(&mut w, &wt_a);

    git(&wt_a, &["pull", "--no-edit"]);
    assert_version_bumped(&mut w, &wt_a);
}

#[test]
fn bumps_on_push() {
    let bare = tempfile::TempDir::new().unwrap();
    let bare_path = bare.path().join("remote.git");
    std::fs::create_dir_all(&bare_path).unwrap();
    git(&bare_path, &["init", "--bare"]);

    let r = TestRepo::new();
    let repo_path = r.path().to_path_buf();
    git(
        &repo_path,
        &["remote", "add", "origin", bare_path.to_str().unwrap()],
    );
    r.write("push_me.txt", "push content");
    r.git(&["add", "push_me.txt"]);
    r.git(&["commit", "-m", "commit to push"]);
    git(&repo_path, &["branch", "-M", "main"]);
    git(&repo_path, &["push", "-u", "origin", "main"]);

    r.write("another.txt", "more content");
    r.git(&["add", "another.txt"]);
    r.git(&["commit", "-m", "another commit"]);

    let mut w = WatcherState::new();
    ensure_watcher(&mut w, &repo_path);
    thread::sleep(Duration::from_millis(50));
    w.poll();

    git(&repo_path, &["push"]);
    assert_version_bumped(&mut w, &repo_path);
}

#[test]
fn bumps_on_clean() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("untracked_clean.txt", "to be cleaned");
    ensure_watcher(&mut w, r.path());
    r.git(&["clean", "-f"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_gc() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    ensure_watcher(&mut w, r.path());
    r.git(&["gc"]);
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn bumps_on_manual_rename() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("rename_me.txt", "content");
    ensure_watcher(&mut w, r.path());
    std::fs::rename(r.path().join("rename_me.txt"), r.path().join("renamed.txt")).unwrap();
    assert_version_bumped(&mut w, r.path());
}

#[test]
fn ignored_file_does_not_increase_version() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    std::fs::write(r.path().join(".gitignore"), "ignored_*\n").unwrap();
    r.git(&["add", ".gitignore"]);
    r.git(&["commit", "-m", "add gitignore"]);
    ensure_watcher(&mut w, r.path());
    let before = w.version(r.path());
    r.write("ignored_file.txt", "should be ignored by git");
    std::thread::sleep(std::time::Duration::from_millis(300));
    w.poll();
    assert_eq!(
        w.version(r.path()),
        before,
        "ignored file should NOT increase version"
    );
}

#[test]
fn bumps_on_branch_create() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    ensure_watcher(&mut w, r.path());
    r.git(&["branch", "unrelated"]);
    assert_version_bumped(&mut w, r.path());
}

// ==== Config mtime test ====

#[test]
fn config_mtime_changes_cache_key() {
    let cfg_dir = tempfile::TempDir::new().unwrap();
    let cfg_path = cfg_dir.path().join("starship.toml");
    std::fs::write(&cfg_path, "format = 'test'\n").unwrap();
    settle();
    let r = TestRepo::new();

    let mtime_before = cache::get_mtime_ns(&cfg_path);
    let key_before = cache::compute_cache_key(r.path(), "vi", 120, mtime_before, 0);

    // Retry with escalating backoff: coarse-mtime filesystems (FAT, network)
    // may not observe a rewrite within the same tick.
    let mut mtime_after = mtime_before;
    for delay_ms in [50u64, 100, 200, 400, 800] {
        std::thread::sleep(Duration::from_millis(delay_ms));
        std::fs::write(&cfg_path, "format = 'changed'\n").unwrap();
        mtime_after = cache::get_mtime_ns(&cfg_path);
        if mtime_after != mtime_before {
            break;
        }
    }
    assert_ne!(
        mtime_after, mtime_before,
        "config mtime did not change across backoff retries (mtime granularity too coarse for this filesystem)"
    );

    let key_after = cache::compute_cache_key(r.path(), "vi", 120, mtime_after, 0);
    assert_ne!(
        key_before, key_after,
        "config change should produce different cache key (config_mtime tracked)"
    );
}
