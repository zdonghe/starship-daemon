mod common;

use starship_daemon::cache;
use starship_daemon::render;

#[test]
fn zero_watcher_version_stays_fresh_across_cds() {
    let r = tempfile::TempDir::new().unwrap();
    let repo = r.path();
    let git = |args: &[&str]| common::git(repo, args);
    git(&["init"]);
    git(&["config", "user.email", "test@test"]);
    git(&["config", "user.name", "test"]);
    std::fs::write(repo.join("a.txt"), "hello").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-m", "initial"]);

    let cfg = toml::toml! {
        format = "$git_status"
        add_newline = false
    };
    let git_dir = starship_daemon::find_git_dir(repo);
    let mut lru = lru::LruCache::new(std::num::NonZeroUsize::new(8).unwrap());

    let ctx = |cwd: std::path::PathBuf| render::RenderContext {
        cwd,
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };
    let key = |cwd: &std::path::Path| cache::compute_cache_key(cwd, "vi", 120, 0, 0);

    let out1 = render::render_cached(
        &ctx(repo.to_path_buf()),
        git_dir.as_deref(),
        &cfg,
        &key(repo),
        &mut lru,
    );

    std::fs::write(repo.join("untracked.txt"), "new").unwrap();
    common::settle();

    let sub = repo.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let out2 = render::render_cached(
        &ctx(sub.clone()),
        git_dir.as_deref(),
        &cfg,
        &key(&sub),
        &mut lru,
    );
    assert_ne!(
        out1, out2,
        "watcher_version 0 must never pin stale git status across renders"
    );
}
