use std::path::Path;
use std::process::Command;

use starship_daemon::render;

mod common;
use common::*;

#[test]
fn render_output_reflects_git_status_changes() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();

    let git_dir = starship_daemon::find_git_dir(r.path());
    let out1 = render::render_prompt_with_config(
        &render_ctx(r.path()),
        git_dir.as_deref(),
        &cfg,
        render::BustDir::Fresh,
    );
    assert!(!out1.is_empty(), "first render should produce output");

    r.write("untracked.txt", "new");
    settle();
    let out2 = render::render_prompt_with_config(
        &render_ctx(r.path()),
        git_dir.as_deref(),
        &cfg,
        render::BustDir::Fresh,
    );
    assert!(!out2.is_empty());
    assert_ne!(out1, out2, "render output should change after file create");

    r.git(&["add", "untracked.txt"]);
    settle();
    let out3 = render::render_prompt_with_config(
        &render_ctx(r.path()),
        git_dir.as_deref(),
        &cfg,
        render::BustDir::Fresh,
    );
    assert!(!out3.is_empty());
    assert_ne!(out2, out3, "render output should change after git add");

    r.git(&["commit", "-m", "add untracked.txt"]);
    settle();
    let out4 = render::render_prompt_with_config(
        &render_ctx(r.path()),
        git_dir.as_deref(),
        &cfg,
        render::BustDir::Fresh,
    );
    assert!(!out4.is_empty());
}

#[test]
fn render_output_is_deterministic() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    let ctx = render::RenderContext {
        cwd: r.path().to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };

    let git_dir = starship_daemon::find_git_dir(r.path());
    let out1 =
        render::render_prompt_with_config(&ctx, git_dir.as_deref(), &cfg, render::BustDir::Fresh);
    let out2 =
        render::render_prompt_with_config(&ctx, git_dir.as_deref(), &cfg, render::BustDir::Fresh);
    assert_eq!(out1, out2, "same inputs should produce same render output");
}

#[test]
fn render_auto_finds_git_dir_when_none_passed() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    let ctx = render::RenderContext {
        cwd: r.path().to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };

    let git_dir = starship_daemon::find_git_dir(r.path());
    let explicit =
        render::render_prompt_with_config(&ctx, git_dir.as_deref(), &cfg, render::BustDir::Fresh);
    let auto = render::render_prompt_with_config(&ctx, None, &cfg, render::BustDir::Fresh);
    assert!(!auto.is_empty(), "auto-find fallback should produce output");
    assert_eq!(
        auto, explicit,
        "auto-find and explicit git_dir should render identically"
    );
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

    let ctx_main = render::RenderContext {
        cwd: r.path().to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };
    let ctx_sub = render::RenderContext {
        cwd: nested.clone(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };

    let git_dir_main = starship_daemon::find_git_dir(r.path());
    let git_dir_sub = starship_daemon::find_git_dir(&nested);
    let out_main = render::render_prompt_with_config(
        &ctx_main,
        git_dir_main.as_deref(),
        &cfg,
        render::BustDir::Fresh,
    );
    let out_sub = render::render_prompt_with_config(
        &ctx_sub,
        git_dir_sub.as_deref(),
        &cfg,
        render::BustDir::Fresh,
    );
    assert_ne!(
        out_main, out_sub,
        "render output should differ for different git repos"
    );
}

fn render_ctx(repo: &Path) -> render::RenderContext {
    render::RenderContext {
        cwd: repo.to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    }
}

fn render(repo: &Path, git_dir: Option<&Path>, cfg: &toml::Table) -> String {
    render::render_prompt_with_config(&render_ctx(repo), git_dir, cfg, render::BustDir::Fresh)
}

#[test]
fn render_modified_tracked_file() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    r.write("mod.txt", "original");
    r.git(&["add", "mod.txt"]);
    r.git(&["commit", "-m", "add mod.txt"]);
    settle();
    let git_dir = starship_daemon::find_git_dir(r.path());
    let before = render(r.path(), git_dir.as_deref(), &cfg);
    r.write("mod.txt", "modified content");
    settle();
    let after = render(r.path(), git_dir.as_deref(), &cfg);
    assert_ne!(
        before, after,
        "render should change after tracked file modified"
    );
}

#[test]
fn render_deleted_tracked_file() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    r.write("del.txt", "delete me");
    r.git(&["add", "del.txt"]);
    r.git(&["commit", "-m", "add del.txt"]);
    settle();
    let git_dir = starship_daemon::find_git_dir(r.path());
    let before = render(r.path(), git_dir.as_deref(), &cfg);
    r.remove("del.txt");
    settle();
    let after = render(r.path(), git_dir.as_deref(), &cfg);
    assert_ne!(
        before, after,
        "render should change after tracked file deleted"
    );
}

#[test]
fn render_renamed_with_git_mv() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    r.write("old.txt", "rename me");
    r.git(&["add", "old.txt"]);
    r.git(&["commit", "-m", "add old.txt"]);
    settle();
    let git_dir = starship_daemon::find_git_dir(r.path());
    let before = render(r.path(), git_dir.as_deref(), &cfg);
    r.git(&["mv", "old.txt", "new.txt"]);
    settle();
    let after = render(r.path(), git_dir.as_deref(), &cfg);
    assert_ne!(before, after, "render should change after git mv");
}

#[test]
fn render_manual_rename_without_git() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    r.write("manual.txt", "rename manually");
    r.git(&["add", "manual.txt"]);
    r.git(&["commit", "-m", "add manual.txt"]);
    settle();
    let git_dir = starship_daemon::find_git_dir(r.path());
    let before = render(r.path(), git_dir.as_deref(), &cfg);
    std::fs::rename(
        r.path().join("manual.txt"),
        r.path().join("manual_renamed.txt"),
    )
    .unwrap();
    settle();
    let after = render(r.path(), git_dir.as_deref(), &cfg);
    assert_ne!(
        before, after,
        "render should change after manual file rename"
    );
}

#[test]
fn render_merge_conflict() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    let base_branch = current_branch(r.path());
    r.write("conflict.txt", "base content");
    r.git(&["add", "conflict.txt"]);
    r.git(&["commit", "-m", "base"]);
    r.git(&["checkout", "-b", "side"]);
    std::fs::write(r.path().join("conflict.txt"), "side content").unwrap();
    r.git(&["add", "conflict.txt"]);
    r.git(&["commit", "-m", "side change"]);
    r.git(&["checkout", &base_branch]);
    std::fs::write(r.path().join("conflict.txt"), "main content").unwrap();
    r.git(&["add", "conflict.txt"]);
    r.git(&["commit", "-m", "main change"]);
    settle();
    let git_dir = starship_daemon::find_git_dir(r.path());
    let before = render(r.path(), git_dir.as_deref(), &cfg);
    let _ = Command::new("git")
        .arg("-C")
        .arg(r.path())
        .args(["merge", "side", "--no-edit"])
        .output();
    settle();
    let after = render(r.path(), git_dir.as_deref(), &cfg);
    assert_ne!(before, after, "render should change after merge conflict");
}

#[test]
fn render_stash_push_pop_deterministic() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    r.write("stash_me.txt", "original");
    r.git(&["add", "stash_me.txt"]);
    r.git(&["commit", "-m", "init"]);
    r.write("stash_me.txt", "modified for stash");
    settle();
    let git_dir = starship_daemon::find_git_dir(r.path());
    let dirty = render(r.path(), git_dir.as_deref(), &cfg);
    r.git(&["stash", "push"]);
    settle();
    let clean = render(r.path(), git_dir.as_deref(), &cfg);
    assert_ne!(dirty, clean, "render should change after stash push");
    r.git(&["stash", "pop"]);
    settle();
    let restored = render(r.path(), git_dir.as_deref(), &cfg);
    assert_eq!(
        dirty, restored,
        "render should return to original after stash pop"
    );
}

#[test]
fn render_ignored_file_unchanged() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    r.write("tracked.txt", "tracked content");
    r.git(&["add", "tracked.txt"]);
    r.git(&["commit", "-m", "add tracked"]);
    std::fs::write(r.path().join(".gitignore"), "ignored_*\n").unwrap();
    r.git(&["add", ".gitignore"]);
    r.git(&["commit", "-m", "add gitignore"]);
    settle();
    let git_dir = starship_daemon::find_git_dir(r.path());
    let before = render(r.path(), git_dir.as_deref(), &cfg);
    r.write("ignored_file.txt", "should be ignored");
    settle();
    let after = render(r.path(), git_dir.as_deref(), &cfg);
    assert_eq!(
        before, after,
        "render should NOT change after creating ignored file"
    );
}

#[test]
fn render_bare_repo_does_not_crash() {
    let bare = tempfile::TempDir::new().unwrap();
    let bare_path = bare.path().join("repo.git");
    std::fs::create_dir_all(&bare_path).unwrap();
    git(&bare_path, &["init", "--bare"]);
    settle();
    let cfg = toml::Table::new();
    let ctx = render::RenderContext {
        cwd: bare_path.clone(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };
    let git_dir = starship_daemon::find_git_dir(&bare_path);
    let out =
        render::render_prompt_with_config(&ctx, git_dir.as_deref(), &cfg, render::BustDir::Fresh);
    assert!(!out.is_empty(), "bare repo render should produce output");
}

#[test]
fn render_subdir_file_create() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    let deep = r.path().join("deep").join("dir");
    std::fs::create_dir_all(&deep).unwrap();
    settle();
    let git_dir = starship_daemon::find_git_dir(r.path());
    let before = render(r.path(), git_dir.as_deref(), &cfg);
    r.write("deep/dir/new_file.txt", "new in subdir");
    settle();
    let after = render(r.path(), git_dir.as_deref(), &cfg);
    assert_ne!(
        before, after,
        "render should change after file creation in subdirectory"
    );
}

#[test]
fn render_subdir_file_modify() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    let deep = r.path().join("deep").join("dir");
    std::fs::create_dir_all(&deep).unwrap();
    r.write("deep/dir/tracked.txt", "original");
    r.git(&["add", "deep/dir/tracked.txt"]);
    r.git(&["commit", "-m", "add deep tracked file"]);
    settle();
    let git_dir = starship_daemon::find_git_dir(r.path());
    let before = render(r.path(), git_dir.as_deref(), &cfg);
    r.write("deep/dir/tracked.txt", "modified in subdir");
    settle();
    let after = render(r.path(), git_dir.as_deref(), &cfg);
    assert_ne!(
        before, after,
        "render should change after modifying tracked file in subdirectory"
    );
}

#[test]
fn render_subdir_file_delete() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    let deep = r.path().join("deep").join("dir");
    std::fs::create_dir_all(&deep).unwrap();
    r.write("deep/dir/todelete.txt", "delete me");
    r.git(&["add", "deep/dir/todelete.txt"]);
    r.git(&["commit", "-m", "add deep file to delete"]);
    settle();
    let git_dir = starship_daemon::find_git_dir(r.path());
    let before = render(r.path(), git_dir.as_deref(), &cfg);
    r.remove("deep/dir/todelete.txt");
    settle();
    let after = render(r.path(), git_dir.as_deref(), &cfg);
    assert_ne!(
        before, after,
        "render should change after deleting tracked file in subdirectory"
    );
}

#[test]
fn render_subdir_manual_rename_without_git() {
    let r = TestRepo::new();
    let cfg = toml::Table::new();
    let deep = r.path().join("deep").join("dir");
    std::fs::create_dir_all(&deep).unwrap();
    r.write("deep/dir/original.txt", "original");
    r.git(&["add", "deep/dir/original.txt"]);
    r.git(&["commit", "-m", "add subdir file"]);
    settle();
    let git_dir = starship_daemon::find_git_dir(r.path());
    let before = render(r.path(), git_dir.as_deref(), &cfg);
    std::fs::rename(
        r.path().join("deep/dir/original.txt"),
        r.path().join("deep/dir/renamed.txt"),
    )
    .unwrap();
    settle();
    let after = render(r.path(), git_dir.as_deref(), &cfg);
    assert_ne!(
        before, after,
        "render should change after manual rename in subdirectory"
    );
}
