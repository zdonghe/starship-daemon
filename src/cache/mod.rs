use std::path::{Path, PathBuf};

pub mod config;

pub use config::*;

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
    pub keymap: String,
    pub terminal_width: usize,
    pub config_mtime: u64,
    pub watcher_version: u64,
}

pub fn compute_cache_key(cwd: &Path, keymap: &str, terminal_width: usize, config_mtime: u64, watcher_version: u64) -> CacheKey {
    CacheKey {
        cwd: cwd.to_path_buf(), keymap: keymap.to_string(), terminal_width,
        config_mtime, watcher_version,
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
    fn compute_cache_key_all_fields() {
        let k = compute_cache_key(Path::new("/home/user"), "emacs", 100, 7, 3);
        let expected = CacheKey {
            cwd: PathBuf::from("/home/user"), keymap: "emacs".into(),
            terminal_width: 100, config_mtime: 7, watcher_version: 3,
        };
        assert_eq!(k, expected, "struct comparison catches swapped-field bugs");
    }

    #[test]
    fn compute_cache_key_different_watcher_version_differentiates() {
        let k1 = compute_cache_key(Path::new("/a"), "vi", 120, 0, 0);
        let k2 = compute_cache_key(Path::new("/a"), "vi", 120, 0, 5);
        assert_ne!(k1, k2);
    }
}
