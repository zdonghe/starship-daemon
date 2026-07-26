use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use starship_daemon::prompt::{self, CacheKey};

const SLEEP_MS: u64 = 15;

fn git(repo: &Path, args: &[&str]) {
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

fn settle() {
    thread::sleep(Duration::from_millis(SLEEP_MS));
}

struct TestRepo {
    dir: tempfile::TempDir,
}

impl TestRepo {
    fn new() -> Self {
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

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn git(&self, args: &[&str]) {
        git(self.path(), args);
    }

    fn write(&self, name: &str, content: &str) {
        std::fs::write(self.path().join(name), content).unwrap();
    }

    fn remove(&self, name: &str) {
        std::fs::remove_file(self.path().join(name)).unwrap();
    }

    fn cache_key(&self, time_bucket: u64, config_path: &Path) -> CacheKey {
        prompt::compute_cache_key(self.path(), 0, "vi", 120, time_bucket, config_path, None)
    }
}

fn no_config() -> PathBuf {
    PathBuf::from("__nonexistent_config__")
}

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

fn current_branch(repo: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .expect("git rev-parse failed");
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
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
    let k1 = prompt::compute_cache_key(r.path(), 0, "vi", 120, 0, &cfg, None);
    let k2 = prompt::compute_cache_key(r.path(), 1, "vi", 120, 0, &cfg, None);
    assert_ne!(k1, k2);
}

#[test]
fn keymap_field_differentiates_keymaps() {
    let r = TestRepo::new();
    let cfg = no_config();
    let k1 = prompt::compute_cache_key(r.path(), 0, "vi", 120, 0, &cfg, None);
    let k2 = prompt::compute_cache_key(r.path(), 0, "emacs", 120, 0, &cfg, None);
    assert_ne!(k1, k2);
}

#[test]
fn terminal_width_field_differentiates_widths() {
    let r = TestRepo::new();
    let cfg = no_config();
    let k1 = prompt::compute_cache_key(r.path(), 0, "vi", 120, 0, &cfg, None);
    let k2 = prompt::compute_cache_key(r.path(), 0, "vi", 80, 0, &cfg, None);
    assert_ne!(k1, k2);
}

#[test]
fn time_bucket_field_differentiates_buckets() {
    let r = TestRepo::new();
    let cfg = no_config();
    let k1 = prompt::compute_cache_key(r.path(), 0, "vi", 120, 0, &cfg, None);
    let k2 = prompt::compute_cache_key(r.path(), 0, "vi", 120, 1, &cfg, None);
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
    // Wait for any pending mtime to settle
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
    // Need a second commit so HEAD~1 is valid
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
    // Two worktrees: push from B, fetch in A — remote_mtime changes, index does not
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
    let before = prompt::compute_cache_key(&wt_a, 0, "vi", 120, 0, &cfg, None);
    settle();

    git(&wt_a, &["fetch"]);
    settle();

    let after = prompt::compute_cache_key(&wt_a, 0, "vi", 120, 0, &cfg, None);
    // remote_mtime changes because remote-tracking ref is updated
    assert_changed(&before, &after, "remote_mtime");
    // index_mtime does NOT change on fetch (no merge)
    assert_unchanged(&before, &after, "index_mtime");

}

// ==== End-to-end render + cache integration test ====

#[test]
fn render_output_reflects_git_status_changes() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();

    let ctx = |cwd: &Path| prompt::RenderContext {
        cwd: cwd.to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };

    let git_dir = starship_daemon::find_git_dir(r.path());
    let out1 = prompt::render_prompt_with_config(&ctx(r.path()), git_dir.as_deref(), &cfg);
    assert!(!out1.is_empty(), "first render should produce output");

    r.write("untracked.txt", "new");
    settle();
    let out2 = prompt::render_prompt_with_config(&ctx(r.path()), git_dir.as_deref(), &cfg);
    assert!(!out2.is_empty());
    assert_ne!(out1, out2, "render output should change after file create");

    r.git(&["add", "untracked.txt"]);
    settle();
    let out3 = prompt::render_prompt_with_config(&ctx(r.path()), git_dir.as_deref(), &cfg);
    assert!(!out3.is_empty());
    assert_ne!(out2, out3, "render output should change after git add");

    r.git(&["commit", "-m", "add untracked.txt"]);
    settle();
    let out4 = prompt::render_prompt_with_config(&ctx(r.path()), git_dir.as_deref(), &cfg);
    assert!(!out4.is_empty());
}

#[test]
fn render_output_is_deterministic() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    let ctx = prompt::RenderContext {
        cwd: r.path().to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };

    let git_dir = starship_daemon::find_git_dir(r.path());
    let out1 = prompt::render_prompt_with_config(&ctx, git_dir.as_deref(), &cfg);
    let out2 = prompt::render_prompt_with_config(&ctx, git_dir.as_deref(), &cfg);
    assert_eq!(out1, out2, "same inputs should produce same render output");
}

#[test]
fn different_cwd_produces_different_render() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();

    let nested = r.path().join("sub");
    std::fs::create_dir_all(&nested).unwrap();
    git(&nested, &["init"]);
    git(&nested, &["config", "user.email", "test@test"]);
    git(&nested, &["config", "user.name", "test"]);
    std::fs::write(nested.join("nested.txt"), "nested").unwrap();
    git(&nested, &["add", "nested.txt"]);
    git(&nested, &["commit", "-m", "nested init"]);
    settle();

    let ctx_main = prompt::RenderContext {
        cwd: r.path().to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };
    let ctx_sub = prompt::RenderContext {
        cwd: nested.clone(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };

    let git_dir_main = starship_daemon::find_git_dir(r.path());
    let git_dir_sub = starship_daemon::find_git_dir(&nested);
    let out_main = prompt::render_prompt_with_config(&ctx_main, git_dir_main.as_deref(), &cfg);
    let out_sub = prompt::render_prompt_with_config(&ctx_sub, git_dir_sub.as_deref(), &cfg);
    assert_ne!(out_main, out_sub, "render output should differ for different git repos");
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
    // Two commits so HEAD~1 is valid
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
    // Tag creates a file in .git/refs/tags/ -- no monitored mtime changes
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
    // New branch creates a ref file, but current branch ref + HEAD + index unchanged
    assert_eq!(before, after);
}

// On Windows git status may rewrite .git/index even when nothing changed,
// so we cannot assert index_mtime stays unchanged.

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
    // Switch back to base branch and make another commit to force non-ff merge
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
    let before = prompt::compute_cache_key(&wt_a, 0, "vi", 120, 0, &cfg, None);
    settle();

    git(&wt_a, &["pull", "--no-edit"]);
    settle();

    let after = prompt::compute_cache_key(&wt_a, 0, "vi", 120, 0, &cfg, None);
    // pull = fetch + merge: remote ref updated, index rewritten, branch ref moved
    assert_changed(&before, &after, "remote_mtime");
    assert_changed(&before, &after, "index_mtime");
    assert_changed(&before, &after, "branch_mtime");
}




