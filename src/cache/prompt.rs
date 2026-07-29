use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::cache::get_mtime_ns;

pub struct RenderContext {
    pub cwd: PathBuf,
    pub terminal_width: usize,
    pub status_code: i32,
    pub keymap: String,
}

struct RepoCache {
    git_dir: PathBuf,
    index_mtime: u64,
    ctx: Option<starship::context::Context<'static>>,
}

static REPO_CACHE: Mutex<Option<RepoCache>> = Mutex::new(None);

fn get_or_create_ctx(
    git_dir: Option<&Path>,
    current_dir: &Path,
    logical_dir: &Path,
    status_code: i32,
    keymap: &str,
    terminal_width: usize,
    config: &toml::Table,
) -> starship::context::Context<'static> {
    if let Some(gd) = git_dir {
        let index_mtime = get_mtime_ns(&gd.join("index"));
        let mut cache = REPO_CACHE.lock().unwrap();
        if let Some(ref mut cached) = *cache {
            if cached.git_dir == gd && cached.index_mtime == index_mtime {
                if let Some(sctx) = cached.ctx.take() {
                    return sctx;
                }
            }
        }
    }
    let mut properties = starship::context::Properties::default();
    properties.status_code = Some(status_code.to_string());
    properties.keymap = keymap.to_string();
    let env = starship::context::Env::default();
    let mut sctx = starship::context::Context::new_with_shell_and_path(
        properties, starship::context::Shell::Pwsh, starship::context::Target::Main,
        current_dir.to_path_buf(), logical_dir.to_path_buf(), env,
    );
    sctx.width = terminal_width;
    sctx = sctx.set_config(config.clone());
    sctx
}

pub fn render_prompt(ctx: &RenderContext, git_dir: Option<&Path>) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static BUST_COUNTER: AtomicU64 = AtomicU64::new(0);

    let (current_dir, bust_dir) = match git_dir.map(Path::to_path_buf).or_else(|| crate::find_git_dir(&ctx.cwd)) {
        Some(git_dir) => {
            let bust = git_dir.join("bust").join(format!("{}", BUST_COUNTER.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::create_dir_all(&bust);
            (bust.clone(), Some(bust))
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
    if let Some(dir) = bust_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
    result.trim_end_matches('\n').to_string()
}

pub fn render_prompt_with_config(ctx: &RenderContext, git_dir: Option<&Path>, config: &toml::Table) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static BUST_COUNTER: AtomicU64 = AtomicU64::new(0);

    let (current_dir, bust_dir, resolved_git_dir) = match git_dir.map(Path::to_path_buf).or_else(|| crate::find_git_dir(&ctx.cwd)) {
        Some(git_dir) => {
            let bust = git_dir.join("bust").join(format!("{}", BUST_COUNTER.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::create_dir_all(&bust);
            (bust.clone(), Some(bust), Some(git_dir))
        }
        None => (ctx.cwd.clone(), None, None),
    };

    let mut sctx = get_or_create_ctx(
        resolved_git_dir.as_deref(),
        &current_dir,
        &ctx.cwd,
        ctx.status_code,
        &ctx.keymap,
        ctx.terminal_width,
        config,
    );

    sctx.current_dir = current_dir.clone();
    sctx.logical_dir = ctx.cwd.clone();
    sctx.width = ctx.terminal_width;
    sctx.properties.status_code = Some(ctx.status_code.to_string());
    sctx.properties.keymap = ctx.keymap.clone();
    sctx = sctx.set_config(config.clone());

    let result = starship::print::get_prompt(&sctx);

    if let Some(ref gd) = resolved_git_dir {
        let index_mtime = get_mtime_ns(&gd.join("index"));
        let mut cache = REPO_CACHE.lock().unwrap();
        *cache = Some(RepoCache {
            git_dir: gd.clone(),
            index_mtime,
            ctx: Some(sctx),
        });
    }

    if let Some(dir) = bust_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
    result.trim_end_matches('\n').to_string()
}
