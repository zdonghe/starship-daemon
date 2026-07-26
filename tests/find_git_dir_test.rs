use std::path::{Path, PathBuf};
use std::process::Command;

const DOTFILES: &str = r"C:\Users\Dong\Documents\dotfiles";
const CFG: &str = r"C:\Users\Dong\Documents\dotfiles\configs\starship\starship.toml";

#[test]
fn find_git_dir_at_root() {
    let result = starship_daemon::find_git_dir(&PathBuf::from(DOTFILES));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), PathBuf::from(DOTFILES).join(".git"));
}

#[test]
fn find_git_dir_one_level_deep() {
    let subdir = PathBuf::from(DOTFILES).join("configs");
    let result = starship_daemon::find_git_dir(&subdir);
    assert!(result.is_some());
    assert!(result.unwrap().ends_with(".git"));
}

#[test]
fn find_git_dir_two_levels_deep() {
    let subdir = PathBuf::from(DOTFILES).join("configs").join("starship");
    let result = starship_daemon::find_git_dir(&subdir);
    assert!(result.is_some());
}

#[test]
fn find_git_dir_no_repo() {
    let result = starship_daemon::find_git_dir(&PathBuf::from(r"C:\Users"));
    assert!(result.is_none());
}

#[test]
fn cache_key_changes_on_file_create_in_subdir() {
    use starship_daemon::prompt;
    let subdir = PathBuf::from(DOTFILES).join("configs");
    let test_file = subdir.join("__test_git_create__.txt");

    let _ = std::fs::remove_file(&test_file);
    let key1 = prompt::compute_cache_key(&subdir, 0, "vi", 120, 0, Path::new(CFG), None);

    std::fs::write(&test_file, b"test").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));

    let key2 = prompt::compute_cache_key(&subdir, 0, "vi", 120, 0, Path::new(CFG), None);
    let _ = std::fs::remove_file(&test_file);

    assert_ne!(key1.cwd_mtime, key2.cwd_mtime,
        "cwd_mtime should change after file creation in the same dir");
}

#[test]
fn cache_key_changes_after_git_add_from_subdir() {
    use starship_daemon::prompt;
    let dotfiles = PathBuf::from(DOTFILES);
    let subdir = dotfiles.join("configs");
    let test_file = dotfiles.join("__test_git_add__.txt");

    std::fs::write(&test_file, b"test").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));

    let key1 = prompt::compute_cache_key(&subdir, 0, "vi", 120, 0, Path::new(CFG), None);

    let out = Command::new("git")
        .args(["-C", DOTFILES, "add", test_file.file_name().unwrap().to_str().unwrap()])
        .output().expect("git add");
    assert!(out.status.success(), "git add failed");
    std::thread::sleep(std::time::Duration::from_millis(200));

    let key2 = prompt::compute_cache_key(&subdir, 0, "vi", 120, 0, Path::new(CFG), None);

    let _ = Command::new("git").args(["-C", DOTFILES, "reset", "HEAD", "--", test_file.file_name().unwrap().to_str().unwrap()]).output();
    let _ = std::fs::remove_file(&test_file);

    assert_ne!(key1.index_mtime, key2.index_mtime,
        "index_mtime should change after git add from subdir");
}
