use std::path::PathBuf;
use std::process::Command;

fn init_temp_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let out = Command::new("git")
        .args(["init"])
        .current_dir(&repo)
        .output()
        .expect("git init failed");
    assert!(out.status.success());
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
    let result = starship_daemon::find_git_dir(dir.path());
    assert!(result.is_none());
}

#[test]
fn cache_key_changes_on_file_create_in_subdir() {
    use starship_daemon::prompt;
    let (_d, repo) = init_temp_repo();
    let test_file = repo.join("__test_git_create__.txt");

    let _ = std::fs::remove_file(&test_file);
    let cfg = repo.join("starship.toml");
    std::fs::write(&cfg, b"").unwrap();
    let key1 = prompt::compute_cache_key(&repo, 0, "vi", 120, 0, &cfg, None);

    std::fs::write(&test_file, b"test").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));

    let key2 = prompt::compute_cache_key(&repo, 0, "vi", 120, 0, &cfg, None);
    let _ = std::fs::remove_file(&test_file);

    assert_ne!(key1.cwd_mtime, key2.cwd_mtime,
        "cwd_mtime should change after file creation in the same dir");
}

#[test]
fn cache_key_changes_after_git_add_from_subdir() {
    use starship_daemon::prompt;
    let (_d, repo) = init_temp_repo();
    let subdir = repo.join("sub");
    std::fs::create_dir(&subdir).unwrap();
    let test_file = repo.join("__test_git_add__.txt");

    std::fs::write(&test_file, b"test").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cfg = repo.join("starship.toml");
    std::fs::write(&cfg, b"").unwrap();

    let key1 = prompt::compute_cache_key(&subdir, 0, "vi", 120, 0, &cfg, None);

    let out = Command::new("git")
        .args([
            "-C",
            repo.to_str().unwrap(),
            "add",
            test_file.file_name().unwrap().to_str().unwrap(),
        ])
        .output()
        .expect("git add");
    assert!(out.status.success(), "git add failed");
    std::thread::sleep(std::time::Duration::from_millis(200));

    let key2 = prompt::compute_cache_key(&subdir, 0, "vi", 120, 0, &cfg, None);

    let _ = Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "reset", "HEAD", "--", test_file.file_name().unwrap().to_str().unwrap()])
        .output();
    let _ = std::fs::remove_file(&test_file);

    assert_ne!(key1.index_mtime, key2.index_mtime,
        "index_mtime should change after git add from subdir");
}
