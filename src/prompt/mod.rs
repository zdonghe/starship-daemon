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
    std::env::var("USERPROFILE")
        .map(|h| PathBuf::from(h).join(".config").join("starship.toml"))
        .unwrap_or_else(|_| PathBuf::from(".config/starship.toml"))
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

    let (current_dir, cleanup) = match crate::find_git_dir(&ctx.cwd) {
        Some(git_dir) => {
            let bust = git_dir.join("bust").join(format!("{}", BUST_COUNTER.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::create_dir_all(&bust);
            (bust, Some(git_dir))
        }
        None => (ctx.cwd.clone(), None),
    };

    let mut properties = starship::context::Properties::default();
    properties.status_code = Some(ctx.status_code.to_string());
    properties.keymap = ctx.keymap.clone();

    let env = starship::context::Env::default();
    let mut sctx = starship::context::Context::new_with_shell_and_path(
        properties, starship::context::Shell::Pwsh, starship::context::Target::Main,
        current_dir, ctx.cwd.clone(), env,
    );
    sctx.width = ctx.terminal_width;

    let result = starship::print::get_prompt(&sctx);
    if let Some(git_dir) = cleanup {
        let _ = std::fs::remove_dir_all(git_dir.join("bust"));
    }
    result.trim_end_matches('\n').to_string()
}
