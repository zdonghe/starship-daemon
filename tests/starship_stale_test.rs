/// Test: does starship's get_prompt() return stale git status
/// when called multiple times in the same process?
///
/// Root cause of the daemon's stale git status.
/// The subprocess (starship prompt) works because each call
/// is a new process. The library (starship::print::get_prompt)
/// caches repo state internally and doesn't re-scan on subsequent calls.

use std::path::PathBuf;
use std::time::Duration;

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
    let cwd = PathBuf::from(r"C:\Users\Dong\Documents\dotfiles");
    let test_file = cwd.join("__test_starship_stale__.txt");

    let _ = std::fs::remove_file(&test_file);

    let r1 = render(&cwd);

    std::fs::write(&test_file, b"test").unwrap();
    std::thread::sleep(Duration::from_millis(500));

    let r2 = render(&cwd);
    let _ = std::fs::remove_file(&test_file);

    eprintln!("First render has git status: {}", has_git_status(&r1));
    eprintln!("Second render has git status: {}", has_git_status(&r2));

    assert!(
        has_git_status(&r2),
        "Second get_prompt() should show new file, but returned same as first.\n\
         Starship library caches repo state in-process.\n\
         r1: {}\n r2: {}",
        r1.trim(),
        r2.trim(),
    );
}
