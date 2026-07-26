use std::time::Instant;
use starship_daemon::prompt::{self, RenderContext};
use starship_daemon::find_git_dir;

fn main() {
    let cwd = std::env::current_dir().unwrap();
    let git_dir = find_git_dir(&cwd);
    let config = prompt::default_config_path();
    let _config = prompt::load_config(&config);
    println!("cwd:     {:?}", cwd);
    println!("git_dir: {:?}", git_dir);
    println!("config:  {:?}", config);

    // --- Cold render (no pre-warm) ---
    let ctx = RenderContext {
        cwd: cwd.clone(),
        terminal_width: 120,
        status_code: 0,
        keymap: "vi".to_string(),
    };
    let start = Instant::now();
    let cold_out = prompt::render_prompt(&ctx, git_dir.as_deref());
    let cold_dur = start.elapsed();
    println!("\ncold first render:  {:>8.1} ms", cold_dur.as_secs_f64() * 1000.0);
    println!("  output len: {}", cold_out.len());

    // --- Warm render (pre-warmed by cold render above) ---
    let start = Instant::now();
    let warm_out = prompt::render_prompt(&ctx, git_dir.as_deref());
    let warm_dur = start.elapsed();
    println!("warm (2nd) render: {:>8.1} ms", warm_dur.as_secs_f64() * 1000.0);
    println!("  output len: {}", warm_out.len());

    // --- Multiple renders to see if further improvement ---
    let mut total = 0.0f64;
    for i in 0..10 {
        let start = Instant::now();
        let _ = prompt::render_prompt(&ctx, git_dir.as_deref());
        let d = start.elapsed();
        total += d.as_secs_f64() * 1000.0;
        println!("  render {:>2}: {:>8.1} ms", i + 1, d.as_secs_f64() * 1000.0);
    }
    println!("avg renders 1-10 warm: {:>8.1} ms", total / 10.0);

    // --- Comparison ---
    println!("\n---");
    println!("prewarm saves: {:>8.1} ms (first render)", cold_dur.as_secs_f64() * 1000.0 - warm_dur.as_secs_f64() * 1000.0);
}
