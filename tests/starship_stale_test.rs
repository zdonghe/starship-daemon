use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

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
fn starship_get_prompt_is_stale_in_process() {
    let (_d, repo) = init_repo_with_commit();
    let test_file = repo.join("__test_starship_stale__.txt");

    let _ = std::fs::remove_file(&test_file);

    let r1 = render(&repo);

    std::fs::write(&test_file, b"test").unwrap();
    std::thread::sleep(Duration::from_millis(500));

    let r2 = render(&repo);
    let _ = std::fs::remove_file(&test_file);

    eprintln!("r1 has_status={}", has_git_status(&r1));
    eprintln!("r2 has_status={}", has_git_status(&r2));

    assert!(
        has_git_status(&r2),
        "Second get_prompt() should show new file, but returned same as first.\n\
         Starship library caches repo state in-process.\n\
         r1: {}\n r2: {}",
        r1.trim(),
        r2.trim(),
    );
}
