use std::path::{Path, PathBuf};

pub mod config;
pub mod prompt;

pub use config::*;
pub use prompt::*;

pub fn get_mtime_ns(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub struct CacheKey {
    pub cwd: PathBuf,
    pub status_code: i32,
    pub keymap: String,
    pub terminal_width: usize,
    pub config_mtime: u64,
    pub watcher_gen: u64,
}

pub fn compute_cache_key(cwd: &Path, status_code: i32, keymap: &str, terminal_width: usize, config_path: &Path, watcher_gen: u64) -> CacheKey {
    CacheKey {
        cwd: cwd.to_path_buf(), status_code, keymap: keymap.to_string(), terminal_width,
        config_mtime: get_mtime_ns(config_path),
        watcher_gen,
    }
}

pub fn current_minute() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn get_mtime_ns_missing_returns_zero() {
        assert_eq!(get_mtime_ns(Path::new("__nonexistent_path_xyz__")), 0);
    }

    #[test]
    fn get_mtime_ns_existing_returns_nonzero() {
        let dir = tempfile::TempDir::new().unwrap();
        let f = dir.path().join("t");
        fs::write(&f, "x").unwrap();
        assert!(get_mtime_ns(&f) > 0);
    }

    #[test]
    fn cache_key_equality_reflexive() {
        let k = CacheKey {
            cwd: PathBuf::from("/a"), status_code: 0, keymap: "vi".into(), terminal_width: 120,
            config_mtime: 5, watcher_gen: 0,
        };
        assert_eq!(k, k);
    }

    #[test]
    fn cache_key_equality_different_cwd_not_equal() {
        let k1 = CacheKey {
            cwd: PathBuf::from("/a"), status_code: 0, keymap: "vi".into(), terminal_width: 120,
            config_mtime: 5, watcher_gen: 0,
        };
        let k2 = CacheKey {
            cwd: PathBuf::from("/b"), status_code: 0, keymap: "vi".into(), terminal_width: 120,
            config_mtime: 5, watcher_gen: 0,
        };
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_cache_key_no_git_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let k = compute_cache_key(dir.path(), 0, "vi", 120, Path::new("__nonexistent_config__"),  0);
        assert_eq!(k.status_code, 0);
        assert_eq!(k.keymap, "vi");
        assert_eq!(k.terminal_width, 120);
        assert_eq!(k.config_mtime, 0);
        assert_eq!(k.watcher_gen, 0);
    }

    #[test]
    fn compute_cache_key_different_status_code_differentiates() {
        let dir = tempfile::TempDir::new().unwrap();
        let k1 = compute_cache_key(dir.path(), 0, "vi", 120, Path::new("__nonexistent_config__"),  0);
        let k2 = compute_cache_key(dir.path(), 1, "vi", 120, Path::new("__nonexistent_config__"),  0);
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_cache_key_different_watcher_gen_differentiates() {
        let dir = tempfile::TempDir::new().unwrap();
        let k1 = compute_cache_key(dir.path(), 0, "vi", 120, Path::new("__nonexistent_config__"),  0);
        let k2 = compute_cache_key(dir.path(), 0, "vi", 120, Path::new("__nonexistent_config__"),  5);
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_cache_key_with_nonexistent_cwd_still_works() {
        let bad_cwd = Path::new("__nonexistent_cwd__");
        let k = compute_cache_key(bad_cwd, 0, "vi", 120, Path::new("__nonexistent_config__"),  0);
        assert_eq!(k.config_mtime, 0);
    }
}
