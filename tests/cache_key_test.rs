/// Test: what does starship use as the cache key for REPO_STATUS?
/// If it's just the git workdir (canonicalized), then using a different
/// path won't help — the cache will match anyway.
use std::path::PathBuf;

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
    let dotfiles = PathBuf::from(r"C:\Users\Dong\Documents\dotfiles");
    let subdir = dotfiles.join("configs").join("starship");
    let test_file = dotfiles.join("__test_cache_key__.txt");

    let _ = std::fs::remove_file(&test_file);

    // Render from subdir (no file yet)
    let r1 = render(&subdir);
    eprintln!("R1 (subdir, no file): has_status={}", has_git_status(&r1));

    // Create file in repo root
    std::fs::write(&test_file, b"test").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Render from repo root (file exists)
    let r2 = render(&dotfiles);
    eprintln!("R2 (root, file exists): has_status={}", has_git_status(&r2));
    let _ = std::fs::remove_file(&test_file);

    // If cache key is the workdir (canonicalized), both render the same workdir,
    // so R2 would hit the cache from R1 — stale!
    // If cache key is current_dir (not canonicalized to workdir), they'd differ.
    if has_git_status(&r2) {
        eprintln!("FRESH — cache key is current_dir (different from workdir)");
    } else {
        eprintln!("STALE — cache key is the workdir (both resolved to same repo)");
    }

    // We already know it's stale from the same-path test.
    // This test just determines WHICH path is used as cache key.
}

#[test]
fn existent_path_buster_works() {
    let dotfiles = PathBuf::from(r"C:\Users\Dong\Documents\dotfiles");
    let test_file = dotfiles.join("__test_cache_key2__.txt");
    let _ = std::fs::remove_file(&test_file);

    // First render — clean
    let r1 = render(&dotfiles);
    eprintln!("R1 (root, no file): has_status={}", has_git_status(&r1));

    // Create file
    std::fs::write(&test_file, b"test").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Buster: render from a subdirectory inside the same repo
    let subdir = dotfiles.join("configs").join("starship");
    let r_bust = render(&subdir);
    eprintln!("R_bust (subdir): has_status={}", has_git_status(&r_bust));

    // Real render from root — if cache key is current_dir, this should miss
    let r2 = render(&dotfiles);
    eprintln!("R2 (root, file exists): has_status={}", has_git_status(&r2));

    let _ = std::fs::remove_file(&test_file);

    if has_git_status(&r2) {
        eprintln!("SUCCESS: subdir render busted the root cache");
    } else {
        eprintln!("FAILED: subdir render did NOT bust the root cache");
    }
}
