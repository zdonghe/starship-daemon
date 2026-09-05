mod common;

use std::sync::Mutex;

use starship_daemon::cache;
use starship_daemon::render;

static CACHE_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn git_repo_bust_tree_cleaned_before_render() {
    let _guard = CACHE_LOCK.lock().unwrap();
    let r = common::TestRepo::new();
    let repo = r.path();
    let git_dir = starship_daemon::find_git_dir(repo);
    let gd = git_dir.as_ref().unwrap();

    std::fs::create_dir_all(gd.join("bust/old/100")).unwrap();
    std::fs::write(gd.join("bust/old/100/file.txt"), "stale").unwrap();
    assert!(gd.join("bust/old/100/file.txt").exists());

    let ctx = render::RenderContext {
        cwd: repo.to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };
    let cfg = toml::toml! { add_newline = false };
    let _out = render::render_prompt_with_config(
        &ctx,
        git_dir.as_deref(),
        &cfg,
        render::BustDir::Reuse {
            version: 1,
            config_mtime: 100,
        },
    );

    assert!(
        !gd.join("bust/old").exists(),
        "stale bust tree must be cleaned"
    );
    assert!(
        !gd.join("bust").exists(),
        "bust root must be cleaned after render"
    );
}

#[test]
fn non_git_no_bust_dirs_created() {
    let _guard = CACHE_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let sentinel = dir.path().join("must_not_delete");
    std::fs::create_dir(&sentinel).unwrap();
    std::fs::write(sentinel.join("file.txt"), "keep me").unwrap();

    let ctx = render::RenderContext {
        cwd: dir.path().to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };
    let cfg = toml::toml! { add_newline = false };
    let out = render::render_prompt_with_config(
        &ctx,
        None,
        &cfg,
        render::BustDir::Reuse {
            version: 1,
            config_mtime: 100,
        },
    );

    assert!(!out.is_empty(), "render must produce output");
    assert!(
        sentinel.exists(),
        "non-git dirs must not be touched"
    );
    assert!(
        sentinel.join("file.txt").exists(),
        "files inside non-git dirs must survive"
    );
    assert!(
        !dir.path().join("bust").exists(),
        "no bust dir should be created for non-git"
    );
}

#[test]
fn different_versions_produce_independent_cleanup() {
    let _guard = CACHE_LOCK.lock().unwrap();
    let r = common::TestRepo::new();
    let repo = r.path();
    let git_dir = starship_daemon::find_git_dir(repo);
    let gd = git_dir.as_ref().unwrap();

    let ctx = render::RenderContext {
        cwd: repo.to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };
    let cfg = toml::toml! { add_newline = false };

    let _out1 = render::render_prompt_with_config(
        &ctx,
        git_dir.as_deref(),
        &cfg,
        render::BustDir::Reuse {
            version: 1,
            config_mtime: 100,
        },
    );
    assert!(
        !gd.join("bust").exists(),
        "bust tree must be cleaned after first render"
    );

    let _out2 = render::render_prompt_with_config(
        &ctx,
        git_dir.as_deref(),
        &cfg,
        render::BustDir::Reuse {
            version: 2,
            config_mtime: 100,
        },
    );
    assert!(
        !gd.join("bust").exists(),
        "bust tree must be cleaned after second render"
    );
}

#[test]
fn fresh_bust_dir_cleaned() {
    let _guard = CACHE_LOCK.lock().unwrap();
    let r = common::TestRepo::new();
    let repo = r.path();
    let git_dir = starship_daemon::find_git_dir(repo);
    let gd = git_dir.as_ref().unwrap();

    let ctx = render::RenderContext {
        cwd: repo.to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };
    let cfg = toml::toml! { add_newline = false };
    let _out = render::render_prompt_with_config(
        &ctx,
        git_dir.as_deref(),
        &cfg,
        render::BustDir::Fresh,
    );

    assert!(
        !gd.join("bust").exists(),
        "bust tree must be cleaned after fresh render"
    );
}

#[test]
fn nothing_outside_bust_is_deleted() {
    let _guard = CACHE_LOCK.lock().unwrap();
    let r = common::TestRepo::new();
    let repo = r.path();
    let git_dir = starship_daemon::find_git_dir(repo);
    let gd = git_dir.as_ref().unwrap();

    let git_sentinels: Vec<(&str, &str)> = vec![
        ("refs/sentinel.txt", "keep-ref"),
        ("hooks/sentinel.txt", "keep-hook"),
    ];

    for (rel, content) in &git_sentinels {
        let path = gd.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    }

    std::fs::write(repo.join("project_sentinel.txt"), "keep-project").unwrap();

    std::fs::create_dir_all(gd.join("bust/old/100")).unwrap();
    std::fs::write(gd.join("bust/old/100/file.txt"), "stale").unwrap();

    let ctx = render::RenderContext {
        cwd: repo.to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };
    let cfg = toml::toml! { add_newline = false };
    let _out = render::render_prompt_with_config(
        &ctx,
        git_dir.as_deref(),
        &cfg,
        render::BustDir::Reuse {
            version: 5,
            config_mtime: 999,
        },
    );

    for (rel, expected_content) in &git_sentinels {
        let path = gd.join(rel);
        assert!(path.exists(), "sentinel must survive: {rel}");
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, *expected_content, "sentinel content must be intact: {rel}");
    }

    assert!(
        repo.join("project_sentinel.txt").exists(),
        "project file must survive"
    );
    assert!(
        !gd.join("bust").exists(),
        "bust tree must be cleaned after render"
    );
}

#[test]
fn render_cached_cleans_bust_dirs() {
    let _guard = CACHE_LOCK.lock().unwrap();
    let r = common::TestRepo::new();
    let repo = r.path();
    let git_dir = starship_daemon::find_git_dir(repo);
    let gd = git_dir.as_ref().unwrap();
    let cfg = toml::toml! { add_newline = false };
    let mut lru = lru::LruCache::new(std::num::NonZeroUsize::new(256).unwrap());

    let ctx = render::RenderContext {
        cwd: repo.to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };
    let key = cache::compute_cache_key(repo, "vi", 120, 0, 5);

    let out = render::render_cached(&ctx, git_dir.as_deref(), &cfg, &key, &mut lru);
    assert!(!out.is_empty(), "render must produce output");

    assert!(
        !gd.join("bust").exists(),
        "bust tree must be cleaned after render_cached"
    );
}

#[test]
fn sequential_renders_both_clean_bust_tree() {
    let _guard = CACHE_LOCK.lock().unwrap();
    let r = common::TestRepo::new();
    let repo = r.path();
    let git_dir = starship_daemon::find_git_dir(repo);
    let gd = git_dir.as_ref().unwrap();

    let ctx = render::RenderContext {
        cwd: repo.to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };
    let cfg = toml::toml! { add_newline = false };

    let _out1 = render::render_prompt_with_config(
        &ctx,
        git_dir.as_deref(),
        &cfg,
        render::BustDir::Reuse {
            version: 4,
            config_mtime: 100,
        },
    );
    assert!(
        !gd.join("bust").exists(),
        "bust tree must be cleaned after first render"
    );

    std::fs::create_dir_all(gd.join("bust/stale/200")).unwrap();
    std::fs::write(gd.join("bust/stale/200/file.txt"), "stale").unwrap();

    let _out2 = render::render_prompt_with_config(
        &ctx,
        git_dir.as_deref(),
        &cfg,
        render::BustDir::Reuse {
            version: 4,
            config_mtime: 200,
        },
    );
    assert!(
        !gd.join("bust").exists(),
        "bust tree must be cleaned after second render"
    );
}

#[test]
fn version_bump_invalidates_cache_renders_fresh_status() {
    let _guard = CACHE_LOCK.lock().unwrap();
    let r = common::TestRepo::new();
    let repo = r.path();
    let git_dir = starship_daemon::find_git_dir(repo);
    let cfg = toml::toml! {
        format = "$git_status"
        add_newline = false
    };
    let mut lru = lru::LruCache::new(std::num::NonZeroUsize::new(256).unwrap());

    let ctx = render::RenderContext {
        cwd: repo.to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };

    let key_v1 = cache::compute_cache_key(repo, "vi", 120, 0, 1);
    let out_clean = render::render_cached(
        &ctx,
        git_dir.as_deref(),
        &cfg,
        &key_v1,
        &mut lru,
    );

    r.write("a.txt", "modified");
    common::settle();

    let out_same = render::render_cached(
        &ctx,
        git_dir.as_deref(),
        &cfg,
        &key_v1,
        &mut lru,
    );
    assert_eq!(
        out_clean, out_same,
        "same watcher version should reuse cache"
    );

    let key_v2 = cache::compute_cache_key(repo, "vi", 120, 0, 2);
    let out_bumped = render::render_cached(
        &ctx,
        git_dir.as_deref(),
        &cfg,
        &key_v2,
        &mut lru,
    );
    assert_ne!(
        out_clean, out_bumped,
        "version bump must invalidate cache and render fresh git status"
    );
}
