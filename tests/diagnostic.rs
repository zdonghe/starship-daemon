use std::path::Path;
use std::time::Instant;

fn make_context<'a>(
    cwd: &'a Path,
    config: &'a toml::Table,
    bust_suffix: &'a str,
) -> starship_daemon::starship::context::Context<'a> {
    let git_dir = starship_daemon::find_git_dir(cwd);
    let current_dir: std::path::PathBuf = match git_dir {
        Some(ref gd) => {
            let bust = gd.join("diag_bust").join(bust_suffix);
            let _ = std::fs::create_dir_all(&bust);
            bust
        }
        None => cwd.to_path_buf(),
    };

    let mut props = starship_daemon::starship::context::Properties::default();
    props.status_code = Some("0".into());
    props.keymap = "viins".into();
    let env = starship_daemon::starship::context::Env::default();

    let mut sctx = starship_daemon::starship::context::Context::new_with_shell_and_path(
        props,
        starship_daemon::starship::context::Shell::Pwsh,
        starship_daemon::starship::context::Target::Main,
        current_dir,
        cwd.to_path_buf(),
        env,
    );
    sctx.width = 120;
    sctx = sctx.set_config(config.clone());
    sctx
}

#[test]
fn diag_app_launchers_breakdown() {
    let config = starship_daemon::prompt::read_config(&starship_daemon::prompt::default_config_path());
    let full_fmt = config
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("$all");
    let cwd = Path::new(r"C:\Users\Dong\Documents\Code\app-launchers");

    let ctx = make_context(cwd, &config, "modules");
    let modules = starship_daemon::starship::print::compute_modules(&ctx);

    let mut total_module = 0.0f64;
    println!("\n=== Per-module timing (app-launchers, cold) ===");
    for m in &modules {
        let ms = m.duration.as_secs_f64() * 1000.0;
        total_module += ms;
        println!("  {:<20} {:>8.1}ms", m.get_name(), ms);
    }
    println!("  {:<20} {:>8.1}ms", "--------------------", total_module);

    let ctx2 = make_context(cwd, &config, "prompt");
    let t0 = Instant::now();
    let _ = starship_daemon::starship::print::get_prompt(&ctx2);
    let total_prompt = t0.elapsed().as_secs_f64() * 1000.0;

    println!("\n  {:<20} {:>8.1}ms", "get_prompt total", total_prompt);
    println!("  {:<20} {:>8.1}ms", "format overhead", total_prompt - total_module);
    println!();
}
