use std::num::NonZeroUsize;
use std::thread;
use std::time::Duration;

use lru::LruCache;
use starship_daemon::cache::{self, CacheKey, CachedValue, RenderContext};
use starship_daemon::watch::WatcherState;

mod common;
use common::{no_config, TestRepo};

struct MultiRepoHarness {
    a: TestRepo,
    b: TestRepo,
    watcher: WatcherState,
    lru: LruCache<CacheKey, CachedValue>,
    config: toml::Table,
    config_path: std::path::PathBuf,
}

impl MultiRepoHarness {
    fn new() -> Self {
        let a = TestRepo::new();
        let b = TestRepo::new();
        let config = toml::Table::new();
        let config_path = no_config();
        let mut watcher = WatcherState::new();
        watcher.ensure(a.path());
        watcher.ensure(b.path());
        let mut h = MultiRepoHarness {
            a, b, watcher,
            lru: LruCache::new(NonZeroUsize::new(256).unwrap()),
            config, config_path,
        };
        h.settle_and_poll();
        h
    }

    fn settle_and_poll(&mut self) {
        for _ in 0..2 {
            thread::sleep(Duration::from_millis(300));
            self.watcher.poll();
            thread::sleep(Duration::from_millis(150));
            self.watcher.process_dirty();
        }
    }

    fn render_a(&mut self) -> String {
        let gd = starship_daemon::find_git_dir(self.a.path());
        let gen_val = self.watcher.generation(self.a.path());
        let key = cache::compute_cache_key(
            self.a.path(), 0, "vi", 120,
            self.config_path.as_path(),
            gd.as_deref(), gen_val,
        );
        let ctx = RenderContext {
            cwd: self.a.path().to_path_buf(),
            terminal_width: 120, status_code: 0,
            keymap: "vi".to_string(),
        };
        cache::render_cached(&ctx, gd.as_deref(), &self.config, &key, &mut self.lru)
    }

    fn render_b(&mut self) -> String {
        let gd = starship_daemon::find_git_dir(self.b.path());
        let gen_val = self.watcher.generation(self.b.path());
        let key = cache::compute_cache_key(
            self.b.path(), 0, "vi", 120,
            self.config_path.as_path(),
            gd.as_deref(), gen_val,
        );
        let ctx = RenderContext {
            cwd: self.b.path().to_path_buf(),
            terminal_width: 120, status_code: 0,
            keymap: "vi".to_string(),
        };
        cache::render_cached(&ctx, gd.as_deref(), &self.config, &key, &mut self.lru)
    }

    fn gen_a(&self) -> u64 { self.watcher.generation(self.a.path()) }
    fn gen_b(&self) -> u64 { self.watcher.generation(self.b.path()) }
    fn write_a(&self, name: &str, content: &str) { self.a.write(name, content); }
    fn write_b(&self, name: &str, content: &str) { self.b.write(name, content); }
    fn remove_a(&self, name: &str) { self.a.remove(name); }
    fn git_a(&self, args: &[&str]) { self.a.git(args); }
    fn git_b(&self, args: &[&str]) { self.b.git(args); }
}

// ============================================================
// Watcher gen isolation
// ============================================================

#[test]
fn gen_isolation() {
    let mut h = MultiRepoHarness::new();
    let g0a = h.gen_a();
    let g0b = h.gen_b();

    h.write_a("trigger_a", "change");
    h.settle_and_poll();
    let g1a = h.gen_a();
    let g1b = h.gen_b();
    assert!(g1a > g0a, "A gen should bump from write to A (start={g0a}, now={g1a})");
    assert!(g1b >= g0b, "B gen should not regress");

    h.write_b("trigger_b", "change");
    h.settle_and_poll();
    let g2a = h.gen_a();
    let g2b = h.gen_b();
    assert!(g2b > g1b, "B gen should bump from write to B (start={g1b}, now={g2b})");
    assert!(g2a >= g1a, "A gen should not regress");

    h.write_a("trigger_a2", "another");
    h.settle_and_poll();
    let g3a = h.gen_a();
    let g3b = h.gen_b();
    assert!(g3a > g2a, "A gen should bump again from second write to A (start={g2a}, now={g3a})");
    assert!(g3b >= g2b, "B gen should not regress");
}

// ============================================================
// Render isolation — change only affects the modified repo
// ============================================================

#[test]
fn two_repos_untracked_file() {
    let mut h = MultiRepoHarness::new();
    let a1 = h.render_a();
    let b1 = h.render_b();

    h.write_a("new_file.txt", "untracked content");
    h.settle_and_poll();

    let a2 = h.render_a();
    let b2 = h.render_b();
    assert_ne!(a1, a2, "repo A should change after untracked file create");
    assert_eq!(b1, b2, "repo B should be unaffected");
}

#[test]
fn two_repos_modified_tracked_file() {
    let mut h = MultiRepoHarness::new();
    let a1 = h.render_a();
    let b1 = h.render_b();

    h.write_a("a.txt", "modified content");
    h.settle_and_poll();

    let a2 = h.render_a();
    let b2 = h.render_b();
    assert_ne!(a1, a2, "repo A should change after modifying tracked file");
    assert_eq!(b1, b2, "repo B should be unaffected");
}

#[test]
fn two_repos_deleted_tracked_file() {
    let mut h = MultiRepoHarness::new();
    let a1 = h.render_a();
    let b1 = h.render_b();

    h.remove_a("a.txt");
    h.settle_and_poll();

    let a2 = h.render_a();
    let b2 = h.render_b();
    assert_ne!(a1, a2, "repo A should change after deleting tracked file");
    assert_eq!(b1, b2, "repo B should be unaffected");
}

#[test]
fn two_repos_git_add() {
    let mut h = MultiRepoHarness::new();
    let a1 = h.render_a();
    let b1 = h.render_b();

    h.write_a("staged.txt", "to be staged");
    h.settle_and_poll();
    h.git_a(&["add", "staged.txt"]);
    h.settle_and_poll();

    let a2 = h.render_a();
    let b2 = h.render_b();
    assert_ne!(a1, a2, "repo A should change after creating and staging file");
    assert_eq!(b1, b2, "repo B should be unaffected");
}

#[test]
fn two_repos_manual_rename() {
    let mut h = MultiRepoHarness::new();
    h.write_a("rename_me.txt", "rename target");
    h.git_a(&["add", "rename_me.txt"]);
    h.git_a(&["commit", "-m", "add rename target"]);
    h.settle_and_poll();

    let a1 = h.render_a();
    let b1 = h.render_b();

    std::fs::rename(h.a.path().join("rename_me.txt"), h.a.path().join("renamed.txt")).unwrap();
    h.settle_and_poll();

    let a2 = h.render_a();
    let b2 = h.render_b();
    assert_ne!(a1, a2, "repo A should change after manual rename");
    assert_eq!(b1, b2, "repo B should be unaffected");
}

#[test]
fn two_repos_git_mv() {
    let mut h = MultiRepoHarness::new();
    h.write_a("mv_me.txt", "rename via git");
    h.git_a(&["add", "mv_me.txt"]);
    h.git_a(&["commit", "-m", "add mv target"]);
    h.settle_and_poll();

    let a1 = h.render_a();
    let b1 = h.render_b();

    h.git_a(&["mv", "mv_me.txt", "mv_done.txt"]);
    h.settle_and_poll();

    let a2 = h.render_a();
    let b2 = h.render_b();
    assert_ne!(a1, a2, "repo A should change after git mv");
    assert_eq!(b1, b2, "repo B should be unaffected");
}

#[test]
fn two_repos_branch_switch() {
    let mut h = MultiRepoHarness::new();
    let a1 = h.render_a();
    let b1 = h.render_b();

    h.git_a(&["checkout", "other"]);
    h.settle_and_poll();

    let a2 = h.render_a();
    let b2 = h.render_b();
    assert_ne!(a1, a2, "repo A should change after branch switch");
    assert_eq!(b1, b2, "repo B should be unaffected");
}

#[test]
fn two_repos_stash_push_pop() {
    let mut h = MultiRepoHarness::new();
    let a1 = h.render_a();
    let b1 = h.render_b();

    h.write_a("a.txt", "modified for stash");
    h.settle_and_poll();
    h.git_a(&["stash", "push"]);
    h.settle_and_poll();

    let a_stashed = h.render_a();
    let b2 = h.render_b();
    assert_ne!(a1, a_stashed, "repo A should change after stash push");
    assert_eq!(b1, b2, "repo B should be unaffected");

    h.git_a(&["stash", "pop"]);
    h.settle_and_poll();

    let a_restored = h.render_a();
    let b3 = h.render_b();
    assert_ne!(a_stashed, a_restored, "repo A should change after stash pop");
    assert_ne!(a1, a_restored, "repo A should show modified after stash pop");
    assert_eq!(b1, b3, "repo B should still be unaffected");
}

#[test]
fn two_repos_merge_conflict() {
    let mut h = MultiRepoHarness::new();
    let a1 = h.render_a();
    let b1 = h.render_b();

    let base_a = common::current_branch(h.a.path());
    h.write_a("conflict.txt", "base");
    h.git_a(&["add", "conflict.txt"]);
    h.git_a(&["commit", "-m", "base"]);
    h.git_a(&["checkout", "-b", "side"]);
    std::fs::write(h.a.path().join("conflict.txt"), "side").unwrap();
    h.git_a(&["add", "conflict.txt"]);
    h.git_a(&["commit", "-m", "side change"]);
    h.git_a(&["checkout", &base_a]);
    std::fs::write(h.a.path().join("conflict.txt"), "main").unwrap();
    h.git_a(&["add", "conflict.txt"]);
    h.git_a(&["commit", "-m", "main change"]);
    h.settle_and_poll();

    let _ = std::process::Command::new("git")
        .arg("-C").arg(h.a.path())
        .args(["merge", "side", "--no-edit"])
        .output();
    h.settle_and_poll();

    let a2 = h.render_a();
    let b2 = h.render_b();
    assert_ne!(a1, a2, "repo A should change after merge conflict");
    assert_eq!(b1, b2, "repo B should be unaffected");
}

#[test]
fn two_repos_ignored_file() {
    let mut h = MultiRepoHarness::new();

    std::fs::write(h.a.path().join(".gitignore"), "ignored_*\n").unwrap();
    h.git_a(&["add", ".gitignore"]);
    h.git_a(&["commit", "-m", "add gitignore"]);
    h.settle_and_poll();

    let a_clean = h.render_a();
    let b1 = h.render_b();

    std::fs::write(h.a.path().join("ignored_file.txt"), "should be ignored").unwrap();
    h.settle_and_poll();

    let a2 = h.render_a();
    let b2 = h.render_b();
    assert_eq!(a_clean, a2, "repo A should not change after creating ignored file");
    assert_eq!(b1, b2, "repo B should be unaffected");
}

#[test]
fn two_repos_subdir_create() {
    let mut h = MultiRepoHarness::new();
    let a1 = h.render_a();
    let b1 = h.render_b();

    let deep = h.a.path().join("deep").join("dir");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("new_subfile.txt"), "subdir content").unwrap();
    h.settle_and_poll();

    let a2 = h.render_a();
    let b2 = h.render_b();
    assert_ne!(a1, a2, "repo A should change after subdir file create");
    assert_eq!(b1, b2, "repo B should be unaffected");
}

#[test]
fn two_repos_independent_ops_both() {
    let mut h = MultiRepoHarness::new();
    let a1 = h.render_a();
    let b1 = h.render_b();

    h.write_a("a_only.txt", "only in A");
    h.write_b("b_only.txt", "only in B");
    h.settle_and_poll();

    let a2 = h.render_a();
    let b2 = h.render_b();
    assert_ne!(a1, a2, "repo A should change after file write in A");
    assert_ne!(b1, b2, "repo B should change after file write in B");

    h.write_a("a.txt", "modified");
    h.settle_and_poll();

    let a3 = h.render_a();
    let b3 = h.render_b();
    assert_ne!(a2, a3, "repo A should change again after modifying tracked file");
    assert_eq!(b2, b3, "repo B should stay at state after first write");
}
