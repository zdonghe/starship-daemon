use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const PIPE_NAME: &str = r"\\.\pipe\starship-daemon";

pub mod ffi;
pub mod prompt;
pub mod watcher;

fn find_git_dir_uncached(cwd: &Path) -> Option<PathBuf> {
    for d in cwd.ancestors() {
        let dot_git = d.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        if dot_git.is_file() {
            let content = std::fs::read_to_string(&dot_git).ok()?;
            let path = content.strip_prefix("gitdir: ")?.trim();
            let abs = if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                dot_git.parent()?.join(path)
            };
            return Some(abs);
        }
    }
    None
}

pub fn find_git_dir(cwd: &Path) -> Option<PathBuf> {
    use std::sync::LazyLock;
    static CACHE: LazyLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let key = if cwd.as_os_str().is_empty() { PathBuf::from(".") } else { cwd.to_path_buf() };
    let mut cache = CACHE.lock().unwrap();
    if let Some(result) = cache.get(&key) {
        return result.clone();
    }
    let result = find_git_dir_uncached(&key);
    if cache.len() >= 64 {
        cache.clear();
    }
    cache.insert(key, result.clone());
    result
}
