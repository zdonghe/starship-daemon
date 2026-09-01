mod common;

use std::sync::Mutex;

use starship_daemon::render;

static CACHE_LOCK: Mutex<()> = Mutex::new(());

fn lock_cache() -> std::sync::MutexGuard<'static, ()> {
    CACHE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn default_cfg() -> toml::Table {
    toml::toml! { add_newline = false }
}

fn rctx(cwd: &std::path::Path, status_code: i32) -> render::RenderContext {
    render::RenderContext {
        cwd: cwd.to_path_buf(),
        terminal_width: 120,
        status_code,
        keymap: "vi".to_string(),
    }
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git must be available");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn repo_with_commit() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("tracked.txt"), "a\n").unwrap();
    git(dir.path(), &["init"]);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "init"]);
    dir
}

// ---- Test 1: Git repo - bust dir leaf AND parent cleaned ----
#[test]
fn git_repo_bust_dir_fully_cleaned() {
    let _guard = lock_cache();
    let dir = repo_with_commit();
    let git_dir = starship_daemon::find_git_dir(dir.path());
    let gd = git_dir.as_ref().unwrap();

    render::clear_repo_cache();

    let _out = render::render_prompt_with_config(
        &rctx(dir.path(), 0),
        git_dir.as_deref(),
        &default_cfg(),
        render::BustDir::Reuse {
            version: 1,
            config_mtime: 100,
        },
    );

    let leaf = gd.join("bust/1/100");
    let parent = gd.join("bust/1");
    let root = gd.join("bust");

    assert!(!leaf.exists(), "leaf bust dir must be removed: {leaf:?}");
    assert!(
        !parent.exists(),
        "empty parent bust dir must be removed: {parent:?}"
    );
    assert!(
        root.exists(),
        "bust root should still exist (not attempted): {root:?}"
    );
}

// ---- Test 2: Non-git path - no bust dirs created, nothing deleted ----
#[test]
fn non_git_no_bust_dirs_created() {
    let _guard = lock_cache();
    let dir = tempfile::TempDir::new().unwrap();
    let sentinel = dir.path().join("must_not_delete");
    std::fs::create_dir(&sentinel).unwrap();
    std::fs::write(sentinel.join("file.txt"), "keep me").unwrap();

    render::clear_repo_cache();

    let out = render::render_prompt_with_config(
        &rctx(dir.path(), 0),
        None,
        &default_cfg(),
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

// ---- Test 3: Cache invalidation - different versions produce different paths ----
#[test]
fn different_versions_produce_independent_cleanup() {
    let _guard = lock_cache();
    let dir = repo_with_commit();
    let git_dir = starship_daemon::find_git_dir(dir.path());
    let gd = git_dir.as_ref().unwrap();

    render::clear_repo_cache();

    let _out1 = render::render_prompt_with_config(
        &rctx(dir.path(), 0),
        git_dir.as_deref(),
        &default_cfg(),
        render::BustDir::Reuse {
            version: 1,
            config_mtime: 100,
        },
    );
    assert!(
        !gd.join("bust/1").exists(),
        "version 1 parent must be cleaned"
    );

    let _out2 = render::render_prompt_with_config(
        &rctx(dir.path(), 0),
        git_dir.as_deref(),
        &default_cfg(),
        render::BustDir::Reuse {
            version: 2,
            config_mtime: 100,
        },
    );
    assert!(
        !gd.join("bust/2").exists(),
        "version 2 parent must be cleaned"
    );
    assert!(
        !gd.join("bust/1").exists(),
        "version 1 parent must still be gone"
    );
}

// ---- Test 4: Sibling safety - same version, different mtime ----
#[test]
fn same_version_different_mtime_siblings_are_safe() {
    let _guard = lock_cache();
    let dir = repo_with_commit();
    let git_dir = starship_daemon::find_git_dir(dir.path());
    let gd = git_dir.as_ref().unwrap();

    // Pre-create sibling bust dirs
    std::fs::create_dir_all(gd.join("bust/3/7")).unwrap();
    std::fs::create_dir_all(gd.join("bust/3/8")).unwrap();
    assert!(gd.join("bust/3/7").exists());
    assert!(gd.join("bust/3/8").exists());

    render::clear_repo_cache();

    let _out = render::render_prompt_with_config(
        &rctx(dir.path(), 0),
        git_dir.as_deref(),
        &default_cfg(),
        render::BustDir::Reuse {
            version: 3,
            config_mtime: 7,
        },
    );

    assert!(
        !gd.join("bust/3/7").exists(),
        "target leaf must be removed"
    );
    assert!(
        gd.join("bust/3/8").exists(),
        "sibling leaf must survive"
    );
    assert!(
        gd.join("bust/3").exists(),
        "parent must survive (sibling present)"
    );
}

// ---- Test 5: BustDir::Fresh cleanup ----
#[test]
fn fresh_bust_dir_cleaned() {
    let _guard = lock_cache();
    let dir = repo_with_commit();
    let git_dir = starship_daemon::find_git_dir(dir.path());
    let gd = git_dir.as_ref().unwrap();

    render::clear_repo_cache();

    let _out = render::render_prompt_with_config(
        &rctx(dir.path(), 0),
        git_dir.as_deref(),
        &default_cfg(),
        render::BustDir::Fresh,
    );

    // Leaf must be cleaned
    let bust_disable = gd.join("bust/disable");
    let children: Vec<_> = std::fs::read_dir(&bust_disable)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.metadata().map(|m| m.is_dir()).unwrap_or(false))
        .collect();
    assert!(
        children.is_empty(),
        "no leaf dirs should remain under bust/disable, found: {children:?}"
    );
}

// ---- Test 6: Pre-existing bust dir with unrelated child survives ----
#[test]
fn pre_existing_sibling_child_survives() {
    let _guard = lock_cache();
    let dir = repo_with_commit();
    let git_dir = starship_daemon::find_git_dir(dir.path());
    let gd = git_dir.as_ref().unwrap();

    // Pre-create an unrelated bust dir
    let old_child = gd.join("bust/1/old_child");
    std::fs::create_dir_all(&old_child).unwrap();
    std::fs::write(old_child.join("file.txt"), "keep me").unwrap();

    render::clear_repo_cache();

    let _out = render::render_prompt_with_config(
        &rctx(dir.path(), 0),
        git_dir.as_deref(),
        &default_cfg(),
        render::BustDir::Reuse {
            version: 1,
            config_mtime: 100,
        },
    );

    assert!(
        !gd.join("bust/1/100").exists(),
        "target leaf must be removed"
    );
    assert!(
        old_child.join("file.txt").exists(),
        "pre-existing sibling's files must survive"
    );
    assert!(
        gd.join("bust/1").exists(),
        "parent must survive (old_child present)"
    );
}

// ---- Test 7: Two sequential renders clean parent after last child ----
#[test]
fn sequential_renders_clean_parent_after_last_child() {
    let _guard = lock_cache();
    let dir = repo_with_commit();
    let git_dir = starship_daemon::find_git_dir(dir.path());
    let gd = git_dir.as_ref().unwrap();

    // Pre-create two bust dirs under same parent
    std::fs::create_dir_all(gd.join("bust/4/100")).unwrap();
    std::fs::create_dir_all(gd.join("bust/4/200")).unwrap();

    render::clear_repo_cache();

    let _out1 = render::render_prompt_with_config(
        &rctx(dir.path(), 0),
        git_dir.as_deref(),
        &default_cfg(),
        render::BustDir::Reuse {
            version: 4,
            config_mtime: 100,
        },
    );
    assert!(
        !gd.join("bust/4/100").exists(),
        "first leaf must be removed"
    );
    assert!(
        gd.join("bust/4/200").exists(),
        "second leaf must survive"
    );
    assert!(
        gd.join("bust/4").exists(),
        "parent must survive (sibling present)"
    );

    let _out2 = render::render_prompt_with_config(
        &rctx(dir.path(), 0),
        git_dir.as_deref(),
        &default_cfg(),
        render::BustDir::Reuse {
            version: 4,
            config_mtime: 200,
        },
    );
    assert!(
        !gd.join("bust/4/200").exists(),
        "second leaf must be removed"
    );
    assert!(
        !gd.join("bust/4").exists(),
        "parent must be removed after last child"
    );
}

// ---- Test 8: Cache invalidation renders different git status ----
#[test]
fn version_bump_invalidates_cache_renders_fresh_status() {
    let _guard = lock_cache();
    let dir = repo_with_commit();
    let git_dir = starship_daemon::find_git_dir(dir.path());
    let cfg = toml::toml! {
        format = "$git_status"
        add_newline = false
    };
    let mut lru = lru::LruCache::new(std::num::NonZeroUsize::new(8).unwrap());

    render::clear_repo_cache();

    let ctx = rctx(dir.path(), 0);

    // Clean repo at version 1
    let key_v1 = starship_daemon::cache::compute_cache_key(dir.path(), "vi", 120, 0, 1);
    let out_clean = render::render_cached(
        &ctx,
        git_dir.as_deref(),
        &cfg,
        &key_v1,
        &mut lru,
    );

    // Dirty the repo
    std::fs::write(dir.path().join("tracked.txt"), "b\n").unwrap();
    common::settle();

    // Same version -> should reuse cached (no change detected)
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

    // Bump version -> should render fresh status
    let key_v2 = starship_daemon::cache::compute_cache_key(dir.path(), "vi", 120, 0, 2);
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

// ---- Test 9: render_cached cleans up bust dirs (fork path) ----
#[test]
fn render_cached_cleans_bust_dirs() {
    let _guard = lock_cache();
    let dir = repo_with_commit();
    let git_dir = starship_daemon::find_git_dir(dir.path());
    let gd = git_dir.as_ref().unwrap();
    let cfg = default_cfg();
    let mut lru = lru::LruCache::new(std::num::NonZeroUsize::new(8).unwrap());

    render::clear_repo_cache();

    let ctx = rctx(dir.path(), 0);
    let key = starship_daemon::cache::compute_cache_key(dir.path(), "vi", 120, 0, 5);

    let out = render::render_cached(&ctx, git_dir.as_deref(), &cfg, &key, &mut lru);
    assert!(!out.is_empty(), "render must produce output");

    // All bust dirs under this git_dir should be cleaned
    let bust = gd.join("bust");
    if bust.exists() {
        let remaining: Vec<_> = std::fs::read_dir(&bust)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.metadata().map(|m| m.is_dir()).unwrap_or(false))
            .collect();
        for dir_entry in &remaining {
            let children: Vec<_> = std::fs::read_dir(dir_entry.path())
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .collect();
            assert!(
                children.is_empty(),
                "directory under bust/ should be empty after render: {:?}",
                dir_entry.path()
            );
        }
    }
}

// ---- Test 10: Sentinel test - nothing outside bust leaf/parent is ever deleted ----
#[test]
fn nothing_outside_bust_is_deleted() {
    let _guard = lock_cache();
    let dir = repo_with_commit();
    let git_dir = starship_daemon::find_git_dir(dir.path());
    let gd = git_dir.as_ref().unwrap();

    // Place sentinels inside .git but outside bust leaf/parent
    let git_sentinels: Vec<(&str, &str)> = vec![
        ("refs/sentinel.txt", "keep-ref"),
        ("hooks/sentinel.txt", "keep-hook"),
        ("bust/sentinel.txt", "keep-bust-root"),
        ("bust/99/sentinel.txt", "keep-version-peer"),
        ("bust/98/sentinel.txt", "keep-other-version"),
        ("bust/disable/sentinel.txt", "keep-disable"),
    ];

    for (rel, content) in &git_sentinels {
        let path = gd.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    }

    // Place project sentinel OUTSIDE .git
    std::fs::write(dir.path().join("project_sentinel.txt"), "keep-project").unwrap();

    // Also create a sibling bust dir that shares our target's parent
    std::fs::create_dir_all(gd.join("bust/5/sibling")).unwrap();
    std::fs::write(gd.join("bust/5/sibling/file.txt"), "keep-sibling").unwrap();

    render::clear_repo_cache();

    let _out = render::render_prompt_with_config(
        &rctx(dir.path(), 0),
        git_dir.as_deref(),
        &default_cfg(),
        render::BustDir::Reuse {
            version: 5,
            config_mtime: 999,
        },
    );

    // Verify ALL sentinels survived
    for (rel, expected_content) in &git_sentinels {
        let path = gd.join(rel);
        assert!(path.exists(), "sentinel must survive: {rel}");
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, *expected_content, "sentinel content must be intact: {rel}");
    }

    // Verify sibling bust dir survived
    assert!(
        gd.join("bust/5/sibling/file.txt").exists(),
        "sibling bust dir must survive"
    );

    // Verify project files survived
    assert!(
        dir.path().join("project_sentinel.txt").exists(),
        "project file must survive"
    );
    assert!(
        dir.path().join("tracked.txt").exists(),
        "tracked file must survive"
    );

    // Verify only the target leaf was deleted (parent survives due to sibling)
    assert!(
        !gd.join("bust/5/999").exists(),
        "target leaf must be removed"
    );
    assert!(
        gd.join("bust/5").exists(),
        "parent must survive (sibling present)"
    );
}

// ---- Test 11: Only bust dir leaf+parent deleted, everything else untouched (Fresh path) ----
#[test]
fn fresh_path_only_bust_disable_leaf_deleted() {
    let _guard = lock_cache();
    let dir = repo_with_commit();
    let git_dir = starship_daemon::find_git_dir(dir.path());
    let gd = git_dir.as_ref().unwrap();

    // Place sentinels everywhere
    std::fs::create_dir_all(gd.join("refs")).unwrap();
    std::fs::write(gd.join("refs/sentinel.txt"), "keep-ref").unwrap();
    std::fs::create_dir_all(gd.join("bust")).unwrap();
    std::fs::write(gd.join("bust/sentinel.txt"), "keep-bust-root").unwrap();
    std::fs::create_dir_all(gd.join("bust/disable")).unwrap();
    std::fs::write(gd.join("bust/disable/sentinel.txt"), "keep-disable").unwrap();
    std::fs::write(dir.path().join("project_sentinel.txt"), "keep-project").unwrap();

    render::clear_repo_cache();

    let _out = render::render_prompt_with_config(
        &rctx(dir.path(), 0),
        git_dir.as_deref(),
        &default_cfg(),
        render::BustDir::Fresh,
    );

    // All sentinels must survive
    assert!(gd.join("refs/sentinel.txt").exists(), "git refs sentinel must survive");
    assert!(gd.join("bust/sentinel.txt").exists(), "bust root sentinel must survive");
    assert!(gd.join("bust/disable/sentinel.txt").exists(), "bust/disable sentinel must survive");
    assert!(dir.path().join("project_sentinel.txt").exists(), "project sentinel must survive");

    // Only the leaf bust/disable/<counter> should be gone
    let disable_children: Vec<_> = std::fs::read_dir(gd.join("bust/disable"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.metadata().map(|m| m.is_dir()).unwrap_or(false))
        .collect();
    assert!(
        disable_children.is_empty(),
        "bust/disable should have no directory children after render"
    );
}

// ---- Test 12: Recursive snapshot - every file/dir before render vs after ----
#[test]
fn full_snapshot_before_after_only_bust_leaf_and_parent_change() {
    let _guard = lock_cache();
    let dir = repo_with_commit();
    let git_dir = starship_daemon::find_git_dir(dir.path());
    let gd = git_dir.as_ref().unwrap();

    // Create known bust structure + sentinels
    std::fs::create_dir_all(gd.join("bust/10/100")).unwrap();
    std::fs::write(gd.join("bust/10/100/file.txt"), "target").unwrap();
    std::fs::create_dir_all(gd.join("bust/10/200")).unwrap();
    std::fs::write(gd.join("bust/10/200/file.txt"), "sibling").unwrap();
    std::fs::write(gd.join("bust/sentinel.txt"), "keep").unwrap();
    std::fs::write(gd.join("refs/sentinel.txt"), "keep").unwrap();

    // Verify setup
    assert!(gd.join("bust/10/100/file.txt").exists(), "setup: target file");
    assert!(gd.join("bust/10/200/file.txt").exists(), "setup: sibling file");
    assert!(gd.join("bust/sentinel.txt").exists(), "setup: bust sentinel");
    assert!(gd.join("refs/sentinel.txt").exists(), "setup: refs sentinel");

    render::clear_repo_cache();

    let _out = render::render_prompt_with_config(
        &rctx(dir.path(), 0),
        git_dir.as_deref(),
        &default_cfg(),
        render::BustDir::Reuse {
            version: 10,
            config_mtime: 100,
        },
    );

    // Verify the target leaf was removed
    assert!(!gd.join("bust/10/100/file.txt").exists(), "target file must be gone");
    assert!(!gd.join("bust/10/100").exists(), "target dir must be gone");

    // Verify everything else survived
    assert!(gd.join("bust/10/200/file.txt").exists(), "sibling file must survive");
    assert!(gd.join("bust/10/200").exists(), "sibling dir must survive");
    assert!(gd.join("bust/10").exists(), "version dir must survive (sibling present)");
    assert!(gd.join("bust/sentinel.txt").exists(), "bust sentinel must survive");
    assert!(gd.join("refs/sentinel.txt").exists(), "refs sentinel must survive");
    assert!(dir.path().join("tracked.txt").exists(), "tracked file must survive");
}
