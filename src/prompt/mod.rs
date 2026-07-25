use std::path::{Path, PathBuf};


use starship::context::{Context as StarshipContext, Properties, Shell, Target};
use starship::print;

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
/// git-fast watchers keep gix caches warm so the internal gix scan is fast.
/// Full prompt caching per (cwd, status_code, keymap, time_bucket) avoids
/// re-rendering entirely on the hot path.
pub fn render_prompt(ctx: &RenderContext) -> String {
    let mut properties = Properties::default();
    let t0_props = std::time::Instant::now();
    properties.status_code = Some(ctx.status_code.to_string());
    properties.keymap = ctx.keymap.clone();

    let env = starship::context::Env::default();
    let mut sctx = StarshipContext::new_with_shell_and_path(
        properties, Shell::Pwsh, Target::Main,
        ctx.cwd.clone(), ctx.cwd.clone(), env,
    );
    sctx.width = ctx.terminal_width;
    let t1_ctx = std::time::Instant::now();

    let result = print::get_prompt(&sctx);
    let t2_render = std::time::Instant::now();
    eprintln!("PROMPT_PROFILE: props_setup={:?}  ctx_init={:?}  get_prompt={:?}", t1_ctx.duration_since(t0_props), t2_render.duration_since(t1_ctx), t2_render.duration_since(t0_props));
    result.trim_end_matches('\n').to_string()
}
