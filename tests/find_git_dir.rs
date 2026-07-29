use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::settle;

fn init_temp_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let out = Command::new("git")
        .args(["init"]).current_dir(&repo).output()
        .expect("git init failed");
    assert!(out.status.success());
    settle();
    (dir, repo)
}

#[test]
fn find_git_dir_at_root() {
    let (_d, repo) = init_temp_repo();
    let result = starship_daemon::find_git_dir(&repo);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), repo.join(".git"));
}

#[test]
fn find_git_dir_one_level_deep() {
    let (_d, repo) = init_temp_repo();
    let subdir = repo.join("sub");
    std::fs::create_dir(&subdir).unwrap();
    let result = starship_daemon::find_git_dir(&subdir);
    assert!(result.is_some());
    assert!(result.unwrap().ends_with(".git"));
}

#[test]
fn find_git_dir_two_levels_deep() {
    let (_d, repo) = init_temp_repo();
    let subdir = repo.join("a").join("b");
    std::fs::create_dir_all(&subdir).unwrap();
    let result = starship_daemon::find_git_dir(&subdir);
    assert!(result.is_some());
}

#[test]
fn find_git_dir_no_repo() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    if has_dot_git_ancestor(p) {
        return;
    }
    let result = starship_daemon::find_git_dir(p);
    assert!(result.is_none());
}

fn has_dot_git_ancestor(mut p: &Path) -> bool {
    for _ in 0..32 {
        if p.join(".git").exists() { return true; }
        p = match p.parent() { Some(parent) => parent, None => return false };
    }
    false
}
