use std::thread;
use std::time::Duration;

use starship_daemon::cache::{self, CacheKey};

mod common;
use common::*;

fn assert_unchanged(before: &CacheKey, after: &CacheKey, field: &str) {
    match field {
        "cwd_mtime" => assert_eq!(before.cwd_mtime, after.cwd_mtime, "cwd_mtime changed"),
        "index_mtime" => assert_eq!(before.index_mtime, after.index_mtime, "index_mtime changed"),
        "branch_mtime" => assert_eq!(before.branch_mtime, after.branch_mtime, "branch_mtime changed"),
        "remote_mtime" => assert_eq!(before.remote_mtime, after.remote_mtime, "remote_mtime changed"),
        "config_mtime" => assert_eq!(before.config_mtime, after.config_mtime, "config_mtime changed"),
        _ => panic!("unknown field: {field}"),
    }
}

fn assert_changed(before: &CacheKey, after: &CacheKey, field: &str) {
    match field {
        "cwd_mtime" => assert_ne!(before.cwd_mtime, after.cwd_mtime, "cwd_mtime did NOT change"),
        "index_mtime" => assert_ne!(before.index_mtime, after.index_mtime, "index_mtime did NOT change"),
        "branch_mtime" => assert_ne!(before.branch_mtime, after.branch_mtime, "branch_mtime did NOT change"),
        "remote_mtime" => assert_ne!(before.remote_mtime, after.remote_mtime, "remote_mtime did NOT change"),
        "config_mtime" => assert_ne!(before.config_mtime, after.config_mtime, "config_mtime did NOT change"),
        _ => panic!("unknown field: {field}"),
    }
}

macro_rules! assert_only_changed {
    ($before:expr, $after:expr, [$($changed:ident),* $(,)?]) => {
        $(
            assert_changed($before, $after, stringify!($changed));
        )*
        let all = ["cwd_mtime", "index_mtime", "branch_mtime", "remote_mtime", "config_mtime"];
        let changed: std::collections::HashSet<&str> = [$(stringify!($changed)),*].into_iter().collect();
        for f in &all {
            if !changed.contains(f) {
                assert_unchanged($before, $after, f);
            }
        }
    };
}
// ==== Field identity tests ====

#[test]
fn cwd_field_differentiates_directories() {
    let r = TestRepo::new();
    let cfg = no_config();
    let k1 = r.cache_key(0, &cfg);
        let k2 = r.cache_key(0, &cfg);
    assert_eq!(k1.cwd, k2.cwd);
}

#[test]
fn status_code_field_differentiates_exit_codes() {
    let r = TestRepo::new();
    let cfg = no_config();
    let k1 = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, &cfg, None, 0);
    let k2 = cache::compute_cache_key(r.path(), 1, "vi", 120, 0, &cfg, None, 0);
    assert_ne!(k1, k2);
}

#[test]
fn keymap_field_differentiates_keymaps() {
    let r = TestRepo::new();
    let cfg = no_config();
    let k1 = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, &cfg, None, 0);
    let k2 = cache::compute_cache_key(r.path(), 0, "emacs", 120, 0, &cfg, None, 0);
    assert_ne!(k1, k2);
}

#[test]
fn terminal_width_field_differentiates_widths() {
    let r = TestRepo::new();
    let cfg = no_config();
    let k1 = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, &cfg, None, 0);
    let k2 = cache::compute_cache_key(r.path(), 0, "vi", 80, 0, &cfg, None, 0);
    assert_ne!(k1, k2);
}

#[test]
fn time_bucket_field_differentiates_buckets() {
    let r = TestRepo::new();
    let cfg = no_config();
    let k1 = cache::compute_cache_key(r.path(), 0, "vi", 120, 0, &cfg, None, 0);
    let k2 = cache::compute_cache_key(r.path(), 0, "vi", 120, 1, &cfg, None, 0);
    assert_ne!(k1, k2);
}

// ==== Mtime change detection tests ====

#[test]
fn cwd_mtime_changes_on_file_create() {
    let r = TestRepo::new();
    let cfg = no_config();
    let before = r.cache_key(0, &cfg);
    settle();
    r.write("new_file.txt", "data");
    settle();
    let after = r.cache_key(0, &cfg);
    assert_only_changed!(&before, &after, [cwd_mtime]);
}

#[test]
fn cwd_mtime_changes_on_file_delete() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    let before = r.cache_key(0, &cfg);
    settle();
    r.remove("a.txt");
    settle();
    let after = r.cache_key(0, &cfg);
    assert_only_changed!(&before, &after, [cwd_mtime]);
}

#[test]
fn index_mtime_changes_on_git_add() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("unstaged.txt", "data");
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["add", "unstaged.txt"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_only_changed!(&before, &after, [index_mtime]);
}

#[test]
fn git_commit_changes_index_and_branch_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("b.txt", "world");
    r.git(&["add", "b.txt"]);
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    r.git(&["commit", "-m", "second commit"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "index_mtime");
    assert_changed(&before, &after, "branch_mtime");
}

#[test]
fn git_checkout_changes_index_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["checkout", "other"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "index_mtime");
}

#[test]
fn config_mtime_changes_on_config_write() {
    let cfg_dir = tempfile::TempDir::new().unwrap();
    let cfg_path = cfg_dir.path().join("starship.toml");
    std::fs::write(&cfg_path, "format = 'test'\n").unwrap();
    settle();
    let r = TestRepo::new();
    let before = r.cache_key(0, &cfg_path);
    settle();
    std::fs::write(&cfg_path, "format = 'changed'\n").unwrap();
    settle();
    let after = r.cache_key(0, &cfg_path);
    assert_only_changed!(&before, &after, [config_mtime]);
}

#[test]
fn git_add_only_changes_index_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("exclusive.txt", "test");
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["add", "exclusive.txt"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_only_changed!(&before, &after, [index_mtime]);
}

// ==== Redundancy tests: branch_mtime ====

#[test]
fn branch_mtime_is_redundant_with_index_mtime_on_commit() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("c.txt", "data");
    r.git(&["add", "c.txt"]);
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    r.git(&["commit", "-m", "third commit"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "branch_mtime");
    assert_changed(&before, &after, "index_mtime");
}

#[test]
fn branch_mtime_is_redundant_with_index_mtime_on_soft_reset() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("b.txt", "second");
    r.git(&["add", "b.txt"]);
    r.git(&["commit", "-m", "second"]);
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    r.git(&["reset", "--soft", "HEAD~1"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "branch_mtime");
}

// ==== Non-redundancy tests ====

#[test]
fn remote_mtime_changes_on_git_fetch() {
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
    settle();

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

    let cfg = no_config();
    let before = cache::compute_cache_key(&wt_a, 0, "vi", 120, 0, &cfg, None, 0);
    settle();

    git(&wt_a, &["fetch"]);
    settle();

    let after = cache::compute_cache_key(&wt_a, 0, "vi", 120, 0, &cfg, None, 0);
    assert_changed(&before, &after, "remote_mtime");
    assert_unchanged(&before, &after, "index_mtime");

}

// ==== Comprehensive git operation mtime tests ====

#[test]
fn git_rm_changes_index_and_cwd_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("toremove.txt", "delete me");
    r.git(&["add", "toremove.txt"]);
    r.git(&["commit", "-m", "add toremove.txt"]);
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    r.git(&["rm", "toremove.txt"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_only_changed!(&before, &after, [cwd_mtime, index_mtime]);
}

#[test]
fn git_mv_changes_index_and_cwd_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("old.txt", "rename me");
    r.git(&["add", "old.txt"]);
    r.git(&["commit", "-m", "add old.txt"]);
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    r.git(&["mv", "old.txt", "new.txt"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_only_changed!(&before, &after, [cwd_mtime, index_mtime]);
}

#[test]
fn git_checkout_b_does_not_change_index_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["checkout", "-b", "feature"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "branch_mtime");
    assert_unchanged(&before, &after, "index_mtime");
}

#[test]
fn git_stash_push_pop_changes_index_and_cwd_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("stash.txt", "original");
    r.git(&["add", "stash.txt"]);
    r.git(&["commit", "-m", "add stash.txt"]);
    r.write("stash.txt", "modified");
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["stash", "push"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_only_changed!(&before, &after, [cwd_mtime, index_mtime]);

    settle();
    let before2 = r.cache_key(0, &cfg);
    r.git(&["stash", "pop"]);
    settle();
    let after2 = r.cache_key(0, &cfg);
    assert_only_changed!(&before2, &after2, [cwd_mtime, index_mtime]);
}

#[test]
fn git_reset_hard_changes_branch_index_and_cwd_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("b.txt", "second");
    r.git(&["add", "b.txt"]);
    r.git(&["commit", "-m", "second"]);
    r.write("a.txt", "modified content");
    r.git(&["add", "a.txt"]);
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    r.git(&["reset", "--hard", "HEAD~1"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "branch_mtime");
    assert_changed(&before, &after, "index_mtime");
    assert_changed(&before, &after, "cwd_mtime");
}

#[test]
fn git_revert_changes_index_branch_and_cwd_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("revertable.txt", "will be reverted");
    r.git(&["add", "revertable.txt"]);
    r.git(&["commit", "-m", "to revert"]);
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    r.git(&["revert", "--no-edit", "HEAD"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "branch_mtime");
    assert_changed(&before, &after, "index_mtime");
    assert_changed(&before, &after, "cwd_mtime");
}

#[test]
fn git_commit_amend_changes_index_and_branch_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    r.git(&["commit", "--amend", "-m", "amended initial"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "branch_mtime");
    assert_changed(&before, &after, "index_mtime");
    assert_unchanged(&before, &after, "cwd_mtime");
}

#[test]
fn git_tag_does_not_change_mtimes() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["tag", "v1.0"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_eq!(before, after);
}

#[test]
fn git_branch_create_does_not_change_current_mtimes() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["branch", "unrelated"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_eq!(before, after);
}

#[test]
fn git_log_is_read_only() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["log", "--oneline"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_eq!(before, after);
}

#[test]
fn git_diff_is_read_only() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["diff"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_eq!(before, after);
}

#[test]
fn git_merge_changes_index_branch_and_head() {
    let r = TestRepo::new();
    let cfg = no_config();
    let base_branch = current_branch(r.path());
    r.git(&["checkout", "other"]);
    r.write("feature.txt", "feature work");
    r.git(&["add", "feature.txt"]);
    r.git(&["commit", "-m", "feature work"]);
    r.git(&["checkout", &base_branch]);
    r.write("mainline.txt", "mainline work");
    r.git(&["add", "mainline.txt"]);
    r.git(&["commit", "-m", "mainline work"]);
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    r.git(&["merge", "other", "--no-edit"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "branch_mtime");
    assert_changed(&before, &after, "index_mtime");
    assert_changed(&before, &after, "cwd_mtime");
}

#[test]
fn git_pull_changes_remote_index_and_branch_mtime() {
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
    settle();

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

    let cfg = no_config();
    let before = cache::compute_cache_key(&wt_a, 0, "vi", 120, 0, &cfg, None, 0);
    settle();

    git(&wt_a, &["pull", "--no-edit"]);
    settle();

    let after = cache::compute_cache_key(&wt_a, 0, "vi", 120, 0, &cfg, None, 0);
    assert_changed(&before, &after, "remote_mtime");
    assert_changed(&before, &after, "index_mtime");
    assert_changed(&before, &after, "branch_mtime");
}

#[test]
fn manual_rename_changes_only_cwd_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("rename_me.txt", "content");
    settle();
    let before = r.cache_key(0, &cfg);
    std::fs::rename(r.path().join("rename_me.txt"), r.path().join("renamed.txt")).unwrap();
    settle();
    let after = r.cache_key(0, &cfg);
    assert_only_changed!(&before, &after, [cwd_mtime]);
}

#[test]
fn ignored_file_still_changes_cwd_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    std::fs::write(r.path().join(".gitignore"), "ignored_*\n").unwrap();
    r.git(&["add", ".gitignore"]);
    r.git(&["commit", "-m", "add gitignore"]);
    settle();
    let before = r.cache_key(0, &cfg);
    r.write("ignored_file.txt", "should be ignored by git");
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "cwd_mtime");
}

#[test]
fn no_upstream_branch_remote_mtime_is_zero() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.git(&["checkout", "-b", "no-upstream"]);
    settle();
    let k = r.cache_key(0, &cfg);
    assert_eq!(k.remote_mtime, 0, "branch with no upstream should have remote_mtime=0");
}

#[test]
fn git_rebase_changes_branch_and_index_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    let base_branch = current_branch(r.path());
    r.git(&["checkout", "-b", "rebase-feature"]);
    r.write("feature.txt", "feature work");
    r.git(&["add", "feature.txt"]);
    r.git(&["commit", "-m", "feature commit"]);
    r.git(&["checkout", &base_branch]);
    r.write("mainline.txt", "mainline work");
    r.git(&["add", "mainline.txt"]);
    r.git(&["commit", "-m", "mainline commit"]);
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    r.git(&["rebase", "rebase-feature"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "branch_mtime");
    assert_changed(&before, &after, "index_mtime");
}

#[test]
fn git_cherry_pick_changes_index_branch_and_cwd_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("cherry.txt", "cherry content");
    r.git(&["add", "cherry.txt"]);
    r.git(&["commit", "-m", "commit to cherry-pick"]);
    let commit_hash = String::from_utf8(
        std::process::Command::new("git").arg("-C").arg(r.path()).args(["rev-parse", "HEAD"]).output().unwrap().stdout
    ).unwrap().trim().to_string();
    r.git(&["checkout", "other"]);
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    r.git(&["cherry-pick", &commit_hash]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "index_mtime");
    assert_changed(&before, &after, "branch_mtime");
    assert_changed(&before, &after, "cwd_mtime");
}

#[test]
fn git_push_changes_remote_mtime() {
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
    settle();

    let cfg = no_config();
    r.write("another.txt", "more content");
    r.git(&["add", "another.txt"]);
    r.git(&["commit", "-m", "another commit"]);
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    git(&repo_path, &["push"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "remote_mtime");
}

#[test]
fn git_worktree_separate_cache_key() {
    let r = TestRepo::new();
    let cfg = no_config();
    let worktree_dir = tempfile::TempDir::new().unwrap();
    let wt_path = worktree_dir.path().join("wt");
    r.git(&["worktree", "add", wt_path.to_str().unwrap(), "other"]);
    settle();

    let key_original = r.cache_key(0, &cfg);
    let key_wt = cache::compute_cache_key(&wt_path, 0, "vi", 120, 0, &cfg, None, 0);
    assert_ne!(key_original, key_wt, "worktree should have different cache key from original repo");

    std::fs::write(wt_path.join("worktree_file.txt"), "worktree").unwrap();
    settle();
    let key_wt_after = cache::compute_cache_key(&wt_path, 0, "vi", 120, 0, &cfg, None, 0);
    assert_ne!(key_wt, key_wt_after, "worktree cache key should change after file creation");
}

#[test]
fn git_clean_changes_cwd_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    r.write("untracked_clean.txt", "to be cleaned");
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["clean", "-f"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "cwd_mtime");
    assert_unchanged(&before, &after, "index_mtime");
}

#[test]
fn git_gc_packs_refs_changes_branch_mtime() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["gc"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert!(before != after, "gc packs refs, branch_mtime should change (ref file deleted)");
}

#[test]
fn git_config_change_does_not_change_mtimes() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["config", "test.dummy", "value"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_eq!(before, after);
}

#[test]
fn git_log_with_patch_is_read_only() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["log", "-p", "--all"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_eq!(before, after);
}

#[test]
fn git_status_refreshes_index() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["status", "--porcelain"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_changed(&before, &after, "index_mtime");
    assert_unchanged(&before, &after, "cwd_mtime");
    assert_unchanged(&before, &after, "branch_mtime");
}

#[test]
fn git_push_new_branch_changes_only_remote_mtime() {
    let cfg = no_config();
    let bare = tempfile::TempDir::new().unwrap();
    let bare_path = bare.path().join("remote.git");
    std::fs::create_dir_all(&bare_path).unwrap();
    git(&bare_path, &["init", "--bare"]);

    let r = TestRepo::new();
    let repo_path = r.path().to_path_buf();
    git(&repo_path, &["remote", "add", "origin", bare_path.to_str().unwrap()]);
    git(&repo_path, &["branch", "-M", "main"]);
    git(&repo_path, &["push", "-u", "origin", "main"]);
    r.git(&["checkout", "-b", "other-branch"]);
    r.write("other.txt", "other");
    r.git(&["add", "other.txt"]);
    r.git(&["commit", "-m", "other commit"]);
    settle();
    thread::sleep(Duration::from_millis(50));
    let before = r.cache_key(0, &cfg);
    git(&repo_path, &["push", "-u", "origin", "other-branch"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_only_changed!(&before, &after, [remote_mtime]);
}

#[test]
fn git_archive_is_read_only() {
    let r = TestRepo::new();
    let cfg = no_config();
    settle();
    let before = r.cache_key(0, &cfg);
    r.git(&["archive", "--format=tar", "HEAD"]);
    settle();
    let after = r.cache_key(0, &cfg);
    assert_eq!(before, after);
}
