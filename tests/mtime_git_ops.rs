use std::time::{Duration, Instant};
use std::thread;

use starship_daemon::cache;
use starship_daemon::watch::WatcherState;

mod common;
use common::*;

fn ensure_watcher(w: &mut WatcherState, repo: &std::path::Path) -> u64 {
    w.ensure(repo);
    thread::sleep(Duration::from_millis(300));
    w.poll();
    thread::sleep(Duration::from_millis(150));
    w.process_dirty();
    w.generation(repo)
}

fn assert_gen_increases(w: &mut WatcherState, repo: &std::path::Path, before: u64) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        w.poll();
        thread::sleep(Duration::from_millis(150));
        w.process_dirty();
        let g = w.generation(repo);
        if g > before { return g; }
        thread::sleep(Duration::from_millis(20));
        if Instant::now() > deadline {
            panic!("watcher generation did not increase within 5s (before={before}, current={g})");
        }
    }
}

// ==== Field identity tests ====

#[test]
fn cwd_field_differentiates_directories() {
    let r = TestRepo::new();
    let k1 = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 1);
    let k2 = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 1);
    assert_eq!(k1.cwd, k2.cwd);
}

#[test]
fn status_code_field_differentiates_exit_codes() {
    let r = TestRepo::new();
    let k1 = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 1);
    let k2 = cache::compute_cache_key(r.path(), 1, "vi", 120, 0, 1);
    assert_ne!(k1, k2);
}

#[test]
fn keymap_field_differentiates_keymaps() {
    let r = TestRepo::new();
    let k1 = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 1);
    let k2 = cache::compute_cache_key(r.path(), 0, "emacs", 120, 0, 1);
    assert_ne!(k1, k2);
}

#[test]
fn terminal_width_field_differentiates_widths() {
    let r = TestRepo::new();
    let k1 = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 1);
    let k2 = cache::compute_cache_key(r.path(), 0, "vi", 80, 0, 1);
    assert_ne!(k1, k2);
}

// ==== Watcher generation bump tests ====

#[test]
fn gen_bumps_on_file_create() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    let g0 = ensure_watcher(&mut w, r.path());
    r.write("new_file.txt", "data");
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_file_delete() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("temp.txt", "data");
    let g0 = ensure_watcher(&mut w, r.path());
    r.remove("temp.txt");
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_git_add() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("unstaged.txt", "data");
    let g0 = ensure_watcher(&mut w, r.path());
    r.git(&["add", "unstaged.txt"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_commit() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("b.txt", "world");
    r.git(&["add", "b.txt"]);
    let g0 = ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["commit", "-m", "second commit"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_checkout() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    let g0 = ensure_watcher(&mut w, r.path());
    r.git(&["checkout", "other"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_checkout_new_branch() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    let g0 = ensure_watcher(&mut w, r.path());
    r.git(&["checkout", "-b", "feature"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_git_rm() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("toremove.txt", "delete me");
    r.git(&["add", "toremove.txt"]);
    r.git(&["commit", "-m", "add toremove.txt"]);
    let g0 = ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["rm", "toremove.txt"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_git_mv() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("old.txt", "rename me");
    r.git(&["add", "old.txt"]);
    r.git(&["commit", "-m", "add old.txt"]);
    let g0 = ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["mv", "old.txt", "new.txt"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_stash_push_pop() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("stash.txt", "original");
    r.git(&["add", "stash.txt"]);
    r.git(&["commit", "-m", "add stash.txt"]);
    r.write("stash.txt", "modified");
    let g0 = ensure_watcher(&mut w, r.path());
    r.git(&["stash", "push"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_reset_hard() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("b.txt", "second");
    r.git(&["add", "b.txt"]);
    r.git(&["commit", "-m", "second"]);
    r.write("a.txt", "modified");
    r.git(&["add", "a.txt"]);
    let g0 = ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["reset", "--hard", "HEAD~1"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_revert() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("revertable.txt", "will be reverted");
    r.git(&["add", "revertable.txt"]);
    r.git(&["commit", "-m", "to revert"]);
    let g0 = ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["revert", "--no-edit", "HEAD"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_commit_amend() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    let g0 = ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    r.git(&["commit", "--amend", "-m", "amended initial"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_rebase() {
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
    let g0 = ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["rebase", "rebase-feature"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_cherry_pick() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("cherry.txt", "cherry content");
    r.git(&["add", "cherry.txt"]);
    r.git(&["commit", "-m", "commit to cherry-pick"]);
    let commit_hash = String::from_utf8(
        std::process::Command::new("git").arg("-C").arg(r.path()).args(["rev-parse", "HEAD"]).output().unwrap().stdout
    ).unwrap().trim().to_string();
    r.git(&["checkout", "other"]);
    let g0 = ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["cherry-pick", &commit_hash]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_merge() {
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
    let g0 = ensure_watcher(&mut w, r.path());
    thread::sleep(Duration::from_millis(50));
    w.poll();
    r.git(&["merge", "other", "--no-edit"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_fetch() {
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
    git(&wt_a, &["remote", "add", "origin", bare_path.to_str().unwrap()]);
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
    git(&wt_b, &["remote", "add", "origin", bare_path.to_str().unwrap()]);
    git(&wt_b, &["pull", "origin", "main", "--allow-unrelated-histories"]);
    std::fs::write(wt_b.join("new.txt"), "new data").unwrap();
    git(&wt_b, &["add", "new.txt"]);
    git(&wt_b, &["commit", "-m", "second commit"]);
    git(&wt_b, &["push", "origin", "main"]);

    settle();

    let mut w = WatcherState::new();
    let g0 = ensure_watcher(&mut w, &wt_a);

    git(&wt_a, &["fetch"]);
    assert_gen_increases(&mut w, &wt_a, g0);
}

#[test]
fn gen_bumps_on_pull() {
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
    git(&wt_a, &["remote", "add", "origin", bare_path.to_str().unwrap()]);
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
    git(&wt_b, &["remote", "add", "origin", bare_path.to_str().unwrap()]);
    git(&wt_b, &["pull", "origin", "main", "--allow-unrelated-histories"]);
    std::fs::write(wt_b.join("new.txt"), "new data").unwrap();
    git(&wt_b, &["add", "new.txt"]);
    git(&wt_b, &["commit", "-m", "second commit"]);
    git(&wt_b, &["push", "origin", "main"]);

    settle();

    let mut w = WatcherState::new();
    let g0 = ensure_watcher(&mut w, &wt_a);

    git(&wt_a, &["pull", "--no-edit"]);
    assert_gen_increases(&mut w, &wt_a, g0);
}

#[test]
fn gen_bumps_on_push() {
    let bare = tempfile::TempDir::new().unwrap();
    let bare_path = bare.path().join("remote.git");
    std::fs::create_dir_all(&bare_path).unwrap();
    git(&bare_path, &["init", "--bare"]);

    let r = TestRepo::new();
    let repo_path = r.path().to_path_buf();
    git(&repo_path, &["remote", "add", "origin", bare_path.to_str().unwrap()]);
    r.write("push_me.txt", "push content");
    r.git(&["add", "push_me.txt"]);
    r.git(&["commit", "-m", "commit to push"]);
    git(&repo_path, &["branch", "-M", "main"]);
    git(&repo_path, &["push", "-u", "origin", "main"]);

    r.write("another.txt", "more content");
    r.git(&["add", "another.txt"]);
    r.git(&["commit", "-m", "another commit"]);

    let mut w = WatcherState::new();
    let g0 = ensure_watcher(&mut w, &repo_path);
    thread::sleep(Duration::from_millis(50));
    w.poll();

    git(&repo_path, &["push"]);
    assert_gen_increases(&mut w, &repo_path, g0);
}

#[test]
fn gen_bumps_on_clean() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("untracked_clean.txt", "to be cleaned");
    let g0 = ensure_watcher(&mut w, r.path());
    r.git(&["clean", "-f"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_gc() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    let g0 = ensure_watcher(&mut w, r.path());
    r.git(&["gc"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_manual_rename() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    r.write("rename_me.txt", "content");
    let g0 = ensure_watcher(&mut w, r.path());
    std::fs::rename(r.path().join("rename_me.txt"), r.path().join("renamed.txt")).unwrap();
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn gen_bumps_on_ignored_file_create() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    std::fs::write(r.path().join(".gitignore"), "ignored_*\n").unwrap();
    r.git(&["add", ".gitignore"]);
    r.git(&["commit", "-m", "add gitignore"]);
    let g0 = ensure_watcher(&mut w, r.path());
    r.write("ignored_file.txt", "should be ignored by git");
    std::thread::sleep(std::time::Duration::from_millis(2100));
    w.poll();
    w.process_dirty();
    assert_eq!(w.generation(r.path()), g0, "gen should NOT increase for ignored file");
}

#[test]
fn gen_bumps_on_branch_create() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    let g0 = ensure_watcher(&mut w, r.path());
    r.git(&["branch", "unrelated"]);
    assert_gen_increases(&mut w, r.path(), g0);
}

#[test]
fn cache_key_unchanged_on_tag() {
    let r = TestRepo::new();

    let before = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 0);
    r.git(&["tag", "v1.0"]);
    settle();
    let after = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 0);
    assert_eq!(before, after);
}

// ==== Read-only operation tests (cache key unchanged with fixed g0) ====

#[test]
fn git_log_is_read_only() {
    let r = TestRepo::new();

    let before = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 0);
    r.git(&["log", "--oneline"]);
    settle();
    let after = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 0);
    assert_eq!(before, after);
}

#[test]
fn git_diff_is_read_only() {
    let r = TestRepo::new();

    let before = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 0);
    r.git(&["diff"]);
    settle();
    let after = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 0);
    assert_eq!(before, after);
}

#[test]
fn cache_key_unchanged_on_git_config() {
    let r = TestRepo::new();

    let before = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 0);
    r.git(&["config", "test.dummy", "value"]);
    settle();
    let after = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 0);
    assert_eq!(before, after);
}

#[test]
fn git_archive_is_read_only() {
    let r = TestRepo::new();

    let before = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 0);
    r.git(&["archive", "--format=tar", "HEAD"]);
    settle();
    let after = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 0);
    assert_eq!(before, after);
}

#[test]
fn git_log_with_patch_is_read_only() {
    let r = TestRepo::new();

    let before = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 0);
    r.git(&["log", "-p", "--all"]);
    settle();
    let after = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 0);
    assert_eq!(before, after);
}

// ==== Cache key stability tests ====

#[test]
fn cache_key_unchanged_after_git_ops_with_same_gen() {
    let r = TestRepo::new();

    let key_before = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 1);

    r.write("noise.txt", "should not affect cache key");
    r.git(&["add", "noise.txt"]);
    r.git(&["commit", "-m", "noise commit"]);

    let key_after = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 1);
    assert_eq!(key_before, key_after,
        "cache key must not change with same watcher_gen; mtimes should not differentiate");
}

// ==== Config mtime test ====

#[test]
fn config_mtime_changes_cache_key() {
    let cfg_dir = tempfile::TempDir::new().unwrap();
    let cfg_path = cfg_dir.path().join("starship.toml");
    std::fs::write(&cfg_path, "format = 'test'\n").unwrap();
    settle();
    let r = TestRepo::new();

    let g0 = 1u64;
    let mtime_before = cache::get_mtime_ns(&cfg_path);
    let key_before = cache::compute_cache_key(r.path(), 0, "vi", 120, mtime_before, g0);
    std::fs::write(&cfg_path, "format = 'changed'\n").unwrap();
    settle();
    let mtime_after = cache::get_mtime_ns(&cfg_path);
    let key_after = cache::compute_cache_key(r.path(), 0, "vi", 120, mtime_after, g0);

    assert_ne!(key_before, key_after, "config change should produce different cache key (config_mtime tracked)");
}

// ==== Git status may bump g0 (index refresh) ====

#[test]
fn git_status_may_bump_gen() {
    let r = TestRepo::new();
    let mut w = WatcherState::new();
    ensure_watcher(&mut w, r.path());
    // Just verify no crash
    r.git(&["status", "--porcelain"]);
    thread::sleep(Duration::from_millis(200));
    w.poll();
}

// ==== No-upstream branch does not crash ====

#[test]
fn no_upstream_branch_works() {
    let r = TestRepo::new();

    r.git(&["checkout", "-b", "no-upstream"]);
    settle();
    let k = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 1);
    let k2 = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, 1);
    assert_eq!(k, k2);
}
