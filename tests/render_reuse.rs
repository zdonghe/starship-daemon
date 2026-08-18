#[cfg(feature = "fork")]
mod reuse_test {
    use std::sync::Mutex;

    use starship_daemon::cache;

    static CACHE_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn render_cached_reuses_status_at_stable_watcher_version() {
        let _guard = CACHE_LOCK.lock().unwrap();
        let r = tempfile::TempDir::new().unwrap();
        let repo = r.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C").arg(repo)
                .args(args)
                .output()
                .expect("git command failed");
            assert!(out.status.success(), "git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
        };
        git(&["init"]);
        git(&["config", "user.email", "test@test"]);
        git(&["config", "user.name", "test"]);
        std::fs::write(repo.join("a.txt"), "hello").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-m", "initial"]);
        std::thread::sleep(std::time::Duration::from_millis(15));

        let cfg = toml::toml! {
            format = "$git_status"
            add_newline = false
        };
        let git_dir = starship_daemon::find_git_dir(repo);
        let mut lru = lru::LruCache::new(std::num::NonZeroUsize::new(8).unwrap());

        let ctx_a = cache::RenderContext {
            cwd: repo.to_path_buf(),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let key_a = cache::compute_cache_key(repo, "vi", 120, 0, 7);
        let out1 = cache::render_cached(&ctx_a, git_dir.as_deref(), &cfg, &key_a, &mut lru);

        let sub = repo.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(repo.join("untracked.txt"), "new").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));

        let ctx_b = cache::RenderContext {
            cwd: sub.clone(),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let key_b = cache::compute_cache_key(&sub, "vi", 120, 0, 7);
        let out2 = cache::render_cached(&ctx_b, git_dir.as_deref(), &cfg, &key_b, &mut lru);
        assert_eq!(out1, out2, "stable watcher version must reuse cached git status (no rescan)");

        let key_c = cache::compute_cache_key(&sub, "vi", 120, 0, 8);
        let out3 = cache::render_cached(&ctx_b, git_dir.as_deref(), &cfg, &key_c, &mut lru);
        assert_ne!(out1, out3, "watcher version bump must force a fresh scan reflecting the change");
    }

    #[test]
    fn config_change_at_stable_version_forces_fresh_status() {
        let _guard = CACHE_LOCK.lock().unwrap();
        let r = tempfile::TempDir::new().unwrap();
        let repo = r.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C").arg(repo)
                .args(args)
                .output()
                .expect("git command failed");
            assert!(out.status.success(), "git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
        };
        git(&["init"]);
        git(&["config", "user.email", "test@test"]);
        git(&["config", "user.name", "test"]);
        std::fs::write(repo.join("a.txt"), "hello").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-m", "initial"]);
        std::thread::sleep(std::time::Duration::from_millis(15));

        let cfg = toml::toml! {
            format = "$git_status"
            add_newline = false
        };
        let git_dir = starship_daemon::find_git_dir(repo);
        let mut lru = lru::LruCache::new(std::num::NonZeroUsize::new(8).unwrap());

        let ctx = cache::RenderContext {
            cwd: repo.to_path_buf(),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let out1 = cache::render_cached(&ctx, git_dir.as_deref(), &cfg, &cache::compute_cache_key(repo, "vi", 120, 100, 7), &mut lru);

        std::fs::write(repo.join("untracked.txt"), "new").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));

        let out2 = cache::render_cached(&ctx, git_dir.as_deref(), &cfg, &cache::compute_cache_key(repo, "vi", 120, 200, 7), &mut lru);
        assert_ne!(out1, out2, "config change at stable version must force a fresh status scan");
    }
}

#[cfg(not(feature = "fork"))]
mod non_fork_compile_guard {
    #[allow(dead_code)]
    fn _assert_path_is_path(_p: &std::path::Path) {}
}
