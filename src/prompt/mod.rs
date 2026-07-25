use std::path::{Path, PathBuf};

/// Minimal config wrapper — just tracks path for reload detection.
#[derive(Debug)]
pub struct ModuleConfig {
    pub config_path: PathBuf,
}

/// Per-request context
pub struct RenderContext {
    pub cwd: PathBuf,
    pub terminal_width: usize,
    pub status_code: i32,
    pub keymap: String,
}

/// Validate that a config file exists.
pub fn load_config(path: &Path) -> Option<ModuleConfig> {
    let _content = std::fs::read_to_string(path).ok()?;
    Some(ModuleConfig { config_path: path.to_path_buf() })
}

/// Get default starship config path.
pub fn default_config_path() -> PathBuf {
    if let Ok(cfg) = std::env::var("STARSHIP_CONFIG") {
        let p = PathBuf::from(cfg);
        if p.exists() { return p; }
    }
    let dotfiles = PathBuf::from(r"C:\Users\Dong\Documents\dotfiles");
    let candidates = [
        dotfiles.join("configs").join("starship").join("starship.toml"),
        dotfiles.join("configs").join("starship").join("git.toml"),
    ];
    for c in &candidates { if c.exists() { return c.clone(); } }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(home).join(".config").join("starship.toml");
        if p.exists() { return p; }
    }
    dotfiles.join("configs").join("starship").join("starship.toml")
}

/// Render the full prompt using starship's native pipeline.
///
/// Starship caches git status per `current_dir` in a process-global static
/// (`get_static_repo_status` in git_status.rs). Every call uses a unique
/// subdirectory as `current_dir` and the real cwd as `logical_dir`. The
/// cache key is `current_dir`, so unique paths guarantee a cache miss and
/// fresh git status on every prompt.
pub fn render_prompt(ctx: &RenderContext) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static BUST_COUNTER: AtomicU64 = AtomicU64::new(0);
    // Use .git/bust/ as the unique path  -  it's inside the repo so gix discovers
    // correctly, but it's under .git/ so gix doesn't report it as untracked.
    let bust = ctx.cwd.join(".git").join("bust").join(format!("{}", BUST_COUNTER.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::create_dir_all(&bust);

    let mut properties = starship::context::Properties::default();
    properties.status_code = Some(ctx.status_code.to_string());
    properties.keymap = ctx.keymap.clone();

    let env = starship::context::Env::default();
    let mut sctx = starship::context::Context::new_with_shell_and_path(
        properties, starship::context::Shell::Pwsh, starship::context::Target::Main,
        bust, ctx.cwd.clone(), env,
    );
    sctx.width = ctx.terminal_width;

    let result = starship::print::get_prompt(&sctx);
    let _ = std::fs::remove_dir_all(ctx.cwd.join(".git").join("bust"));
    result.trim_end_matches('\n').to_string()
}
