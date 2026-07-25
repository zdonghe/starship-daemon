use std::path::{Path, PathBuf};

pub const PIPE_NAME: &str = r"\\.\pipe\starship-daemon";

pub mod prompt;

pub fn find_git_dir(cwd: &Path) -> Option<PathBuf> {
    let dot_git = cwd.join(".git");
    if dot_git.is_dir() {
        Some(dot_git)
    } else if dot_git.is_file() {
        let content = std::fs::read_to_string(&dot_git).ok()?;
        let path = content.strip_prefix("gitdir: ")?.trim();
        let abs = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            dot_git.parent()?.join(path)
        };
        Some(abs)
    } else {
        None
    }
}
