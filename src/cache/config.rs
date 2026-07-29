use std::path::{Path, PathBuf};

pub fn load_config(path: &Path) -> Option<PathBuf> {
    if path.is_file() { Some(path.to_path_buf()) } else { None }
}

pub fn default_config_path() -> PathBuf {
    if let Ok(cfg) = std::env::var("STARSHIP_CONFIG") {
        return PathBuf::from(cfg);
    }
    std::env::var("USERPROFILE")
        .map(|h| PathBuf::from(h).join(".config").join("starship.toml"))
        .unwrap_or_else(|_| PathBuf::from(".config/starship.toml"))
}

pub fn read_config(path: &Path) -> toml::Table {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.parse::<toml::Table>().ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_config_returns_none_for_missing() {
        assert!(load_config(Path::new("__nonexistent__")).is_none());
    }

    #[test]
    fn load_config_returns_path_for_existing() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("starship.toml");
        fs::write(&p, "key = 'val'").unwrap();
        assert_eq!(load_config(&p), Some(p.clone()));
    }

    #[test]
    fn read_config_returns_table_for_valid_toml() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("cfg.toml");
        fs::write(&p, "format = 'test'\nsymbol = '$'\n").unwrap();
        let t = read_config(&p);
        assert_eq!(t.get("format").and_then(|v| v.as_str()), Some("test"));
        assert_eq!(t.get("symbol").and_then(|v| v.as_str()), Some("$"));
    }

    #[test]
    fn read_config_invalid_toml_returns_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("bad.toml");
        fs::write(&p, "key = = value").unwrap();
        let t = read_config(&p);
        assert!(t.is_empty());
    }

    #[test]
    fn read_config_missing_returns_empty_table() {
        let t = read_config(Path::new("__nonexistent__"));
        assert!(t.is_empty());
    }

    #[test]
    fn default_config_path_uses_starship_config_env_var() {
        let prev = std::env::var("STARSHIP_CONFIG").ok();
        unsafe { std::env::set_var("STARSHIP_CONFIG", "C:\\custom\\starship.toml"); }
        let p = default_config_path();
        assert_eq!(p, PathBuf::from("C:\\custom\\starship.toml"));
        match prev {
            Some(v) => unsafe { std::env::set_var("STARSHIP_CONFIG", v); },
            None => unsafe { std::env::remove_var("STARSHIP_CONFIG"); },
        }
    }

    #[test]
    fn load_config_returns_none_for_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(load_config(dir.path()).is_none());
    }

    #[test]
    fn read_config_empty_file_returns_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("empty.toml");
        fs::write(&p, "").unwrap();
        let t = read_config(&p);
        assert!(t.is_empty());
    }
}
