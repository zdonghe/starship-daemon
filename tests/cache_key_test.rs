use std::path::PathBuf;
use std::process::Command;

fn init_repo_with_commit() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    Command::new("git").args(["init"]).current_dir(&repo).output().unwrap();
    Command::new("git").args(["config", "user.email", "test@test"]).current_dir(&repo).output().unwrap();
    Command::new("git").args(["config", "user.name", "test"]).current_dir(&repo).output().unwrap();
    std::fs::write(repo.join("initial"), b"").unwrap();
    Command::new("git").args(["add", "."]).current_dir(&repo).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial"]).current_dir(&repo).output().unwrap();
    (dir, repo)
}

fn render(cwd: &PathBuf) -> String {
    use starship::context::{Context as StarshipContext, Properties, Shell, Target};
    use starship::print;

    let mut props = Properties::default();
    props.status_code = Some("0".to_string());
    props.keymap = "vi".to_string();

    let env = starship::context::Env::default();
    let mut sctx = StarshipContext::new_with_shell_and_path(
        props, Shell::Pwsh, Target::Main,
        cwd.clone(), cwd.clone(), env,
    );
    sctx.width = 120;

    let result = print::get_prompt(&sctx);
    result.trim_end_matches('\n').to_string()
}

fn has_git_status(s: &str) -> bool {
    s.contains('?') || s.contains('!') || s.contains('+') || s.contains('\u{2718}')
}

#[test]
fn cache_key_is_directory_not_workdir() {
    let (_d, repo) = init_repo_with_commit();
    let subdir = repo.join("sub");
    std::fs::create_dir(&subdir).unwrap();
    let test_file = repo.join("__test_cache_key__.txt");

    let _ = std::fs::remove_file(&test_file);

    let r1 = render(&subdir);
    eprintln!("R1 (subdir, no file): has_status={}", has_git_status(&r1));

    std::fs::write(&test_file, b"test").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));

    let r2 = render(&repo);
    eprintln!("R2 (root, file exists): has_status={}", has_git_status(&r2));
    let _ = std::fs::remove_file(&test_file);

    if has_git_status(&r2) {
        eprintln!("FRESH");
    } else {
        eprintln!("STALE");
    }
}

#[test]
fn existent_path_buster_works() {
    let (_d, repo) = init_repo_with_commit();
    let subdir = repo.join("sub");
    std::fs::create_dir(&subdir).unwrap();
    let test_file = repo.join("__test_cache_key2__.txt");
    let _ = std::fs::remove_file(&test_file);

    let r1 = render(&repo);
    eprintln!("R1 (root, no file): has_status={}", has_git_status(&r1));

    std::fs::write(&test_file, b"test").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));

    let r_bust = render(&subdir);
    eprintln!("R_bust (subdir): has_status={}", has_git_status(&r_bust));

    let r2 = render(&repo);
    eprintln!("R2 (root, file exists): has_status={}", has_git_status(&r2));

    let _ = std::fs::remove_file(&test_file);

    if has_git_status(&r2) {
        eprintln!("SUCCESS");
    } else {
        eprintln!("FAILED");
    }
}
