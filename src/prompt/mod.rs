use std::path::{Path, PathBuf};

use toml_edit::DocumentMut;

use starship::context::{Context as StarshipContext, Properties, Shell, Target};
use starship::print;

/// Deserialized starship.toml configuration
#[derive(Debug)]
pub struct ModuleConfig {
    pub format: String,
    pub doc: DocumentMut,
    pub scan_timeout: u64,
}

/// Per-request context
pub struct RenderContext {
    pub cwd: PathBuf,
    pub terminal_width: usize,
    pub status_code: i32,
    pub keymap: String,
    pub cmd_duration: Option<u128>,
    pub jobs: i64,
    pub shlvl: Option<i64>,
}

/// Load and parse starship.toml at the given path.
pub fn load_config(path: &Path) -> Option<ModuleConfig> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to read config '{}': {e}", path.display());
            return None;
        }
    };

    let doc: DocumentMut = match content.parse() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("TOML parse error: {e}");
            return None;
        }
    };

    let format = doc["format"].as_str().unwrap_or("$character").to_string();
    let scan_timeout = doc["scan_timeout"].as_integer().unwrap_or(20) as u64;

    Some(ModuleConfig { format, doc, scan_timeout })
}

/// Get default starship config path.
pub fn default_config_path() -> PathBuf {
    if let Ok(cfg) = std::env::var("STARSHIP_CONFIG") {
        let p = PathBuf::from(cfg);
        if p.exists() {
            return p;
        }
    }

    let dotfiles = PathBuf::from(r"C:\Users\Dong\Documents\dotfiles");
    let candidates = [
        dotfiles.join("configs").join("starship").join("starship.toml"),
        dotfiles.join("configs").join("starship").join("git.toml"),
    ];
    for c in &candidates {
        if c.exists() {
            return c.clone();
        }
    }

    if let Ok(home) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(home).join(".config").join("starship.toml");
        if p.exists() {
            return p;
        }
    }

    dotfiles.join("configs").join("starship").join("starship.toml")
}

/// Render the full prompt using starship's native rendering pipeline.
/// This gives 100% visual parity including fill alignment, colors, and spacing.
pub fn render_prompt(ctx: &RenderContext) -> String {
    let mut properties = Properties::default();
    properties.status_code = Some(ctx.status_code.to_string());
    properties.keymap = ctx.keymap.clone();
    properties.cmd_duration = ctx.cmd_duration.map(|d| d.to_string());
    properties.jobs = ctx.jobs;
    properties.shlvl = ctx.shlvl;

    let env = starship::context::Env::default();
    let mut sctx = StarshipContext::new_with_shell_and_path(
        properties,
        Shell::Pwsh,
        Target::Main,
        ctx.cwd.clone(),
        ctx.cwd.clone(),
        env,
    );
    sctx.width = ctx.terminal_width;

    let result = print::get_prompt(&sctx);
    // Trim trailing newline if present
    result.trim_end_matches('\n').to_string()
}
