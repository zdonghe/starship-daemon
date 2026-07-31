use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const PIPE_NAME: &str = r"\\.\pipe\starship-daemon";

pub fn pipe_name() -> String {
    std::env::var("STARSHIP_DAEMON_PIPE").map(|n| {
        if n.starts_with(r"\\.\pipe\") { n } else { format!(r"\\.\pipe\{n}") }
    }).unwrap_or_else(|_| PIPE_NAME.to_string())
}

pub mod cache;
pub mod ffi;
pub mod watch;
pub mod gitignore;

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
    let mut cache = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(result) = cache.get(cwd) {
        return result.clone();
    }
    let actual = if cwd.as_os_str().is_empty() { Path::new(".") } else { cwd };
    let result = find_git_dir_uncached(actual);
    if cache.len() >= 64 {
        cache.clear();
    }
    cache.insert(actual.to_path_buf(), result.clone());
    result
}

pub struct ClientProps {
    pub status_code: Option<i32>,
    pub keymap: Option<String>,
    pub terminal_width: Option<usize>,
    pub starship_config: Option<String>,
    pub disable_cache: Option<bool>,
}

impl ClientProps {
    pub fn parse_json(data: &[u8]) -> Option<Self> {
        let s = std::str::from_utf8(data).ok()?;
        let s = s.trim().trim_start_matches('{').trim_end_matches('}');
        let mut status_code = None;
        let mut keymap = None;
        let mut terminal_width = None;
        let mut starship_config = None;
        let mut disable_cache = None;
        let mut i = 0;
        let bytes = s.as_bytes();
        while i < bytes.len() {
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b',' || bytes[i] == b'\t' || bytes[i] == b'\n') { i += 1; }
            if i >= bytes.len() { break; }
            if bytes[i] != b'"' { break; }
            i += 1;
            let ks = i; while i < bytes.len() && bytes[i] != b'"' { i += 1; }
            if i >= bytes.len() { break; }
            let key = std::str::from_utf8(&bytes[ks..i]).ok()?; i += 1;
            while i < bytes.len() && bytes[i] != b':' { i += 1; }
            if i >= bytes.len() { break; }
            i += 1;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') { i += 1; }
            if i >= bytes.len() { break; }
            if bytes[i] == b'"' {
                i += 1; let vs = i;
                while i < bytes.len() && bytes[i] != b'"' { i += 1; }
                let val = std::str::from_utf8(&bytes[vs..i]).ok()?.to_string(); i += 1;
                match key { "keymap" => keymap = Some(val), "starship_config" => starship_config = Some(val), _ => {} }
            } else if i + 3 < bytes.len() && &bytes[i..i+4] == b"null" { i += 4; }
            else if i + 3 < bytes.len() && &bytes[i..i+4] == b"true" { i += 4; match key { "disable_cache" => disable_cache = Some(true), _ => {} } }
            else if i + 4 < bytes.len() && &bytes[i..i+5] == b"false" { i += 5; match key { "disable_cache" => disable_cache = Some(false), _ => {} } }
            else {
                let vs = i; while i < bytes.len() && bytes[i] != b',' && bytes[i] != b'}' && bytes[i] != b' ' && bytes[i] != b'\n' && bytes[i] != b'\t' { i += 1; }
                let val = std::str::from_utf8(&bytes[vs..i]).ok()?;
                match key { "status_code" => status_code = val.parse::<i32>().ok(), "terminal_width" => terminal_width = val.parse::<usize>().ok(), _ => {} }
            }
        }
        Some(ClientProps { status_code, keymap, terminal_width, starship_config, disable_cache })
    }
}

pub struct ParsedRequest {
    pub cwd: PathBuf,
    pub props: ClientProps,
}

pub fn parse_request(data: &[u8]) -> Option<ParsedRequest> {
    if data.len() < 8 { return None; }
    let cwd_len = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
    if cwd_len > 32768 || 4 + cwd_len + 4 > data.len() { return None; }
    let cwd = PathBuf::from(String::from_utf8_lossy(&data[4..4 + cwd_len]).as_ref());
    let props_start = 4 + cwd_len;
    let props_len = u32::from_le_bytes(data[props_start..props_start + 4].try_into().unwrap()) as usize;
    if props_len > 4096 || props_start + 4 + props_len > data.len() { return None; }
    let props = ClientProps::parse_json(&data[props_start + 4..props_start + 4 + props_len])?;
    Some(ParsedRequest { cwd, props })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_empty_object() {
        let r = ClientProps::parse_json(b"{}");
        assert!(r.is_some());
        let p = r.unwrap();
        assert!(p.status_code.is_none());
        assert!(p.keymap.is_none());
        assert!(p.terminal_width.is_none());
        assert!(p.starship_config.is_none());
        assert!(p.disable_cache.is_none());
    }

    #[test]
    fn parse_json_all_fields() {
        let r = ClientProps::parse_json(
            b"{\"status_code\":0,\"keymap\":\"vi\",\"terminal_width\":120,\"starship_config\":\"C:\\config.toml\",\"disable_cache\":false}"
        );
        assert!(r.is_some());
        let p = r.unwrap();
        assert_eq!(p.status_code, Some(0));
        assert_eq!(p.keymap, Some("vi".to_string()));
        assert_eq!(p.terminal_width, Some(120));
        assert_eq!(p.starship_config, Some("C:\\config.toml".to_string()));
        assert_eq!(p.disable_cache, Some(false));
    }

    #[test]
    fn parse_json_status_code_139() {
        let r = ClientProps::parse_json(b"{\"status_code\":139}");
        assert_eq!(r.unwrap().status_code, Some(139));
    }

    #[test]
    fn parse_json_status_code_negative() {
        let r = ClientProps::parse_json(b"{\"status_code\":-1}");
        assert_eq!(r.unwrap().status_code, Some(-1));
    }

    #[test]
    fn parse_json_keymap_emacs() {
        let r = ClientProps::parse_json(b"{\"keymap\":\"emacs\"}");
        assert_eq!(r.unwrap().keymap, Some("emacs".to_string()));
    }

    #[test]
    fn parse_json_disable_cache_true() {
        let r = ClientProps::parse_json(b"{\"disable_cache\":true}");
        assert_eq!(r.unwrap().disable_cache, Some(true));
    }

    #[test]
    fn parse_json_disable_cache_false() {
        let r = ClientProps::parse_json(b"{\"disable_cache\":false}");
        assert_eq!(r.unwrap().disable_cache, Some(false));
    }

    #[test]
    fn parse_json_unknown_keys_ignored() {
        let r = ClientProps::parse_json(b"{\"unknown\":\"value\",\"another\":42}");
        assert!(r.is_some());
        let p = r.unwrap();
        assert!(p.status_code.is_none());
        assert!(p.keymap.is_none());
    }

    #[test]
    fn parse_json_status_code_string_ignored() {
        let r = ClientProps::parse_json(b"{\"status_code\":\"not_a_number\"}");
        assert!(r.is_some());
        assert!(r.unwrap().status_code.is_none());
    }

    #[test]
    fn parse_json_null_value() {
        let r = ClientProps::parse_json(b"{\"starship_config\":null}");
        assert!(r.is_some());
        assert!(r.unwrap().starship_config.is_none());
    }

    #[test]
    fn parse_json_whitespace_insensitive() {
        let r = ClientProps::parse_json(b"{  \"status_code\"  :  0  ,  \"keymap\"  :  \"vi\"  }");
        assert!(r.is_some());
        let p = r.unwrap();
        assert_eq!(p.status_code, Some(0));
        assert_eq!(p.keymap, Some("vi".to_string()));
    }

    #[test]
    fn parse_json_newlines_and_tabs() {
        let r = ClientProps::parse_json(b"{\n\t\"status_code\": 0\n}");
        assert_eq!(r.unwrap().status_code, Some(0));
    }

    #[test]
    fn parse_json_malformed_truncated_allows_partial() {
        // Parser is lenient: incomplete key-value pairs are just skipped
        let r = ClientProps::parse_json(b"{\"key");
        assert!(r.is_some());
        assert!(r.unwrap().keymap.is_none());
    }

    #[test]
    fn parse_json_empty_input() {
        assert!(ClientProps::parse_json(b"").is_some());
        let p = ClientProps::parse_json(b"").unwrap();
        assert!(p.status_code.is_none());
    }

    #[test]
    fn parse_json_not_utf8() {
        assert!(ClientProps::parse_json(b"\xff\xfe\x00").is_none());
    }

    #[test]
    fn parse_json_full_with_cache_disabled() {
        let r = ClientProps::parse_json(
            b"{\"status_code\":1,\"keymap\":\"emacs\",\"terminal_width\":80,\"disable_cache\":true}"
        );
        let p = r.unwrap();
        assert_eq!(p.status_code, Some(1));
        assert_eq!(p.keymap, Some("emacs".to_string()));
        assert_eq!(p.terminal_width, Some(80));
        assert_eq!(p.disable_cache, Some(true));
    }

    fn encode_request(cwd: &str, props: &str) -> Vec<u8> {
        let cwd_b = cwd.as_bytes();
        let props_b = props.as_bytes();
        let mut data = Vec::new();
        data.extend_from_slice(&(cwd_b.len() as u32).to_le_bytes());
        data.extend_from_slice(cwd_b);
        data.extend_from_slice(&(props_b.len() as u32).to_le_bytes());
        data.extend_from_slice(props_b);
        data
    }

    #[test]
    fn parse_request_valid() {
        let data = encode_request("/home/user/project", "{\"status_code\":0}");
        let r = parse_request(&data);
        assert!(r.is_some());
        let req = r.unwrap();
        assert_eq!(req.cwd, PathBuf::from("/home/user/project"));
        assert_eq!(req.props.status_code, Some(0));
    }

    #[test]
    fn parse_request_empty() {
        assert!(parse_request(&[]).is_none());
        assert!(parse_request(&[0u8; 4]).is_none());
    }

    #[test]
    fn parse_request_truncated() {
        let mut data = encode_request("hello", "{}");
        data.truncate(data.len() - 1);
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_cwd_len_overflow() {
        let mut data = Vec::new();
        data.extend_from_slice(&40000u32.to_le_bytes()); // > 32768
        data.extend_from_slice(b"irrelevant");
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_props_len_overflow() {
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_le_bytes()); // cwd_len = 1
        data.push(b'.');                              // cwd = "."
        data.extend_from_slice(&5000u32.to_le_bytes()); // props_len > 4096
        data.extend_from_slice(b"{}");
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_zero_cwd() {
        let data = encode_request("", "{}");
        let r = parse_request(&data);
        assert!(r.is_some());
        assert_eq!(r.unwrap().cwd, PathBuf::from(""));
    }

    #[test]
    fn parse_request_zero_props() {
        let data = encode_request(".", "");
        let r = parse_request(&data);
        assert!(r.is_some());
    }

    #[test]
    fn parse_request_bad_json_value() {
        let data = encode_request(".", "{bad json}");
        let r = parse_request(&data);
        assert!(r.is_some());
        let req = r.unwrap();
        // Parser treats unknown content as empty; all fields should be None
        assert!(req.props.status_code.is_none());
        assert!(req.props.keymap.is_none());
        assert!(req.props.terminal_width.is_none());
    }

    #[test]
    fn parse_request_non_utf8_cwd_does_not_crash() {
        let cwd_b = b"\xff\xfe";
        let props_b = b"{}";
        let mut data = Vec::new();
        data.extend_from_slice(&(cwd_b.len() as u32).to_le_bytes());
        data.extend_from_slice(cwd_b);
        data.extend_from_slice(&(props_b.len() as u32).to_le_bytes());
        data.extend_from_slice(props_b);
        let r = parse_request(&data);
        assert!(r.is_some());
        let req = r.unwrap();
        // from_utf8_lossy replaces invalid sequences with U+FFFD
        assert!(!req.cwd.as_os_str().is_empty());
    }
}
