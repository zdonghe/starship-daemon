use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use lru::LruCache;

pub const PIPE_NAME: &str = r"\\.\pipe\starship-daemon";

pub fn pipe_name() -> String {
    std::env::var("STARSHIP_DAEMON_PIPE").map(|n| {
        if n.starts_with(r"\\.\pipe\") { n } else { format!(r"\\.\pipe\{n}") }
    }).unwrap_or_else(|_| PIPE_NAME.to_string())
}

pub mod cache;
pub mod daemon;
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
    static CACHE: LazyLock<Mutex<LruCache<PathBuf, Option<PathBuf>>>> =
        LazyLock::new(|| Mutex::new(LruCache::new(NonZeroUsize::new(64).unwrap())));
    let mut cache = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(result) = cache.get(cwd) {
        return result.clone();
    }
    let actual = if cwd.as_os_str().is_empty() { Path::new(".") } else { cwd };
    let result = find_git_dir_uncached(actual);
    cache.put(actual.to_path_buf(), result.clone());
    result
}

// Binary wire protocol.
//
// REQUEST  [u8 version=1][u32 LE total_len][body]     max frame 65536 = MAX_FRAME_LEN
// body     [u32 cwd_len][cwd lossy cap 32768]
//          [i32 status][u16 keymap_len][keymap lossy cap 256, empty -> None]
//          [u32 width][u16 config_len][config lossy cap 4096, empty -> None]
//          [u8 disable cache, non-zero -> true]
// RESPONSE [u32 LE len][prompt utf8]                  len = prompt.len()
//
// The header is 5 bytes and the frame is capped at 65536, so total_len must
// satisfy 5 + total_len <= 65536, i.e. total_len <= 65531. A bad version, an
// over-cap field, or a field reading past the body returns None; the daemon
// then drops the connection and the client falls back to its plain prompt.
pub const PROTO_VERSION: u8 = 1;
pub const HEADER_LEN: usize = 5;
pub const MAX_FRAME_LEN: usize = 65536;
pub const MAX_TOTAL_LEN: usize = MAX_FRAME_LEN - HEADER_LEN; // 65531
pub const MAX_CWD_LEN: usize = 32768;
pub const MAX_KEYMAP_LEN: usize = 256;
pub const MAX_CONFIG_LEN: usize = 4096;

pub struct ClientProps {
    pub status_code: i32,
    pub keymap: Option<String>,
    pub terminal_width: usize,
    pub starship_config: Option<String>,
    pub disable_cache: bool,
}

pub struct ParsedRequest {
    pub cwd: PathBuf,
    pub props: ClientProps,
}

pub fn parse_request(data: &[u8]) -> Option<ParsedRequest> {
    if data.len() < HEADER_LEN { return None; }
    if data[0] != PROTO_VERSION { return None; }
    let total_len = u32::from_le_bytes(data[1..HEADER_LEN].try_into().unwrap()) as usize;
    if total_len > MAX_TOTAL_LEN { return None; }
    if HEADER_LEN + total_len > data.len() { return None; }
    let body = &data[HEADER_LEN..HEADER_LEN + total_len];
    let mut off = 0usize;

    // Every read is bounds checked. A cwd that eats the fixed tail, or an
    // exact-fill keymap or config, must return None - never panic.
    let cwd_len = u32::from_le_bytes(read_slice(body, &mut off, 4)?.try_into().unwrap()) as usize;
    if cwd_len > MAX_CWD_LEN { return None; }
    let cwd = PathBuf::from(String::from_utf8_lossy(read_slice(body, &mut off, cwd_len)?).into_owned());

    let status_code = i32::from_le_bytes(read_slice(body, &mut off, 4)?.try_into().unwrap());

    let keymap_len = u16::from_le_bytes(read_slice(body, &mut off, 2)?.try_into().unwrap()) as usize;
    let keymap = if keymap_len == 0 {
        None
    } else {
        if keymap_len > MAX_KEYMAP_LEN { return None; }
        Some(String::from_utf8_lossy(read_slice(body, &mut off, keymap_len)?).into_owned())
    };

    let terminal_width = u32::from_le_bytes(read_slice(body, &mut off, 4)?.try_into().unwrap()) as usize;

    let config_len = u16::from_le_bytes(read_slice(body, &mut off, 2)?.try_into().unwrap()) as usize;
    let starship_config = if config_len == 0 {
        None
    } else {
        if config_len > MAX_CONFIG_LEN { return None; }
        Some(String::from_utf8_lossy(read_slice(body, &mut off, config_len)?).into_owned())
    };

    let disable = read_slice(body, &mut off, 1)?[0];

    Some(ParsedRequest {
        cwd,
        props: ClientProps { status_code, keymap, terminal_width, starship_config, disable_cache: disable != 0 },
    })
}

fn read_slice<'a>(body: &'a [u8], off: &mut usize, n: usize) -> Option<&'a [u8]> {
    if *off + n > body.len() { return None; }
    let s = &body[*off..*off + n];
    *off += n;
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The fixed body layout with no variable-length data: cwd_len(4) + cwd(0)
    // + status(4) + keymap_len(2) + width(4) + config_len(2) + disable(1) = 17.
    const FIXED_BODY_LEN: usize = 17;

    fn encode_body(cwd: &str, status: i32, keymap: Option<&str>, width: u32, config: Option<&str>, disable: bool) -> Vec<u8> {
        let cwd_b = cwd.as_bytes();
        let keymap_b = keymap.map(|s| s.as_bytes()).unwrap_or_default();
        let config_b = config.map(|s| s.as_bytes()).unwrap_or_default();
        let mut body = Vec::new();
        body.extend_from_slice(&(cwd_b.len() as u32).to_le_bytes());
        body.extend_from_slice(cwd_b);
        body.extend_from_slice(&status.to_le_bytes());
        body.extend_from_slice(&(keymap_b.len() as u16).to_le_bytes());
        body.extend_from_slice(keymap_b);
        body.extend_from_slice(&width.to_le_bytes());
        body.extend_from_slice(&(config_b.len() as u16).to_le_bytes());
        body.extend_from_slice(config_b);
        body.push(if disable { 1 } else { 0 });
        body
    }

    fn encode_request_v1(cwd: &str, status: i32, keymap: Option<&str>, width: u32, config: Option<&str>, disable: bool) -> Vec<u8> {
        let body = encode_body(cwd, status, keymap, width, config, disable);
        let mut data = vec![PROTO_VERSION];
        data.extend_from_slice(&(body.len() as u32).to_le_bytes());
        data.extend_from_slice(&body);
        data
    }

    fn decode(req: &ParsedRequest) -> (i32, Option<String>, usize, Option<String>, bool) {
        (
            req.props.status_code,
            req.props.keymap.clone(),
            req.props.terminal_width,
            req.props.starship_config.clone(),
            req.props.disable_cache,
        )
    }

    #[test]
    fn parse_request_valid_full() {
        let data = encode_request_v1("/home/user/project", 139, Some("vi"), 120, Some(r"C:\config.toml"), true);
        let r = parse_request(&data);
        assert!(r.is_some());
        let req = r.unwrap();
        assert_eq!(req.cwd, PathBuf::from("/home/user/project"));
        assert_eq!(decode(&req), (139, Some("vi".to_string()), 120, Some(r"C:\config.toml".to_string()), true));
    }

    #[test]
    fn parse_request_minimal() {
        // All variable-length fields empty: body = 17 bytes, frame = 22.
        let data = encode_request_v1("", 0, None, 0, None, false);
        assert_eq!(data.len(), HEADER_LEN + FIXED_BODY_LEN);
        let r = parse_request(&data);
        assert!(r.is_some());
        let req = r.unwrap();
        assert_eq!(req.cwd, PathBuf::from(""));
        assert_eq!(decode(&req), (0, None, 0, None, false));
    }

    #[test]
    fn parse_request_empty_keymap_is_none() {
        // keymap_len = 0 -> None -> daemon default ("vi" in daemon.rs).
        let data = encode_request_v1(".", 0, None, 0, None, false);
        let req = parse_request(&data).unwrap();
        assert_eq!(req.props.keymap, None);
    }

    #[test]
    fn parse_request_empty_data() {
        assert!(parse_request(&[]).is_none());
        assert!(parse_request(&[0u8; 5]).is_none());
        assert!(parse_request(&[PROTO_VERSION, 0, 0, 0, 0]).is_none());
    }

    #[test]
    fn parse_request_truncated() {
        let mut data = encode_request_v1("hello", 0, None, 0, None, false);
        data.truncate(data.len() - 1);
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_bad_version() {
        let mut data = encode_request_v1(".", 0, None, 0, None, false);
        data[0] = 2;
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_total_len_over_cap() {
        // Declared total_len exceeds MAX_TOTAL_LEN (65531): rejected even though
        // no body is present.
        let mut data = vec![PROTO_VERSION];
        data.extend_from_slice(&(MAX_TOTAL_LEN as u32 + 1).to_le_bytes());
        assert!(parse_request(&data).is_none());

        // At the cap with no body it is still truncated (data.len() < 5 + total_len).
        let mut data = vec![PROTO_VERSION];
        data.extend_from_slice(&(MAX_TOTAL_LEN as u32).to_le_bytes());
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_trailing_bytes_tolerated() {
        // Forward-compat: fields appended after the disable byte are outside
        // the v1 layout and must be ignored, not rejected.
        let mut data = encode_request_v1(".", 0, Some("vi"), 120, None, true);
        data.push(0); // one extra trailing byte
        data.push(0xde); // more trailing bytes
        let req = parse_request(&data);
        assert!(req.is_some(), "trailing body bytes must be tolerated");
        let req = req.unwrap();
        assert_eq!(decode(&req), (0, Some("vi".to_string()), 120, None, true));
    }

    #[test]
    fn parse_request_disable_nonzero_is_true() {
        // disable_cache treats any non-zero byte as true (lenient decode).
        let mut data = encode_request_v1(".", 0, None, 0, None, false);
        let last = data.len() - 1;
        data[last] = 0xff;
        let req = parse_request(&data).unwrap();
        assert!(req.props.disable_cache, "disable byte 0xff must decode to disable_cache = true");
    }

    #[test]
    fn parse_request_cwd_len_overrun() {
        // cwd_len claims 100 bytes but the body only carries 17.
        let mut data = vec![PROTO_VERSION];
        data.extend_from_slice(&(FIXED_BODY_LEN as u32).to_le_bytes());
        data.extend_from_slice(&100u32.to_le_bytes());
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.push(0);
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_cwd_len_too_big() {
        // cwd_len exceeds MAX_CWD_LEN even though the frame is well-formed.
        let body_len = FIXED_BODY_LEN + MAX_CWD_LEN + 1;
        let mut data = vec![PROTO_VERSION];
        data.extend_from_slice(&(body_len as u32).to_le_bytes());
        data.extend_from_slice(&((MAX_CWD_LEN + 1) as u32).to_le_bytes());
        data.resize(HEADER_LEN + body_len, 0);
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_keymap_overrun() {
        // keymap_len claims 100 bytes but the body ends after the disable byte.
        let mut data = vec![PROTO_VERSION];
        data.extend_from_slice(&(FIXED_BODY_LEN as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes()); // cwd_len = 0
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&100u16.to_le_bytes()); // keymap_len > remaining
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.push(0);
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_config_overrun() {
        let mut data = vec![PROTO_VERSION];
        data.extend_from_slice(&(FIXED_BODY_LEN as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&100u16.to_le_bytes()); // config_len > remaining
        data.push(0);
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_cwd_eats_tail_never_panics() {
        // In-range cwd lengths that consume the fixed tail bytes must return
        // None, never panic. This sweeps the panic band over a 17-byte body
        // (cwd_len >= 10 used to slice body[17..21] and abort).
        for cwd_len in 1..=13usize {
            let mut data = vec![PROTO_VERSION];
            data.extend_from_slice(&(FIXED_BODY_LEN as u32).to_le_bytes());
            data.extend_from_slice(&(cwd_len as u32).to_le_bytes());
            data.resize(HEADER_LEN + FIXED_BODY_LEN, 0);
            assert!(parse_request(&data).is_none(), "cwd_len={cwd_len} must be rejected");
        }

        // keymap_len = 3 fills the body exactly after width: the config_len
        // read used to slice body[17..19] on a 17-byte body and panic.
        let mut data = vec![PROTO_VERSION];
        data.extend_from_slice(&(FIXED_BODY_LEN as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes()); // cwd_len = 0
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&3u16.to_le_bytes()); // keymap_len = 3
        data.resize(HEADER_LEN + FIXED_BODY_LEN, 0);
        assert!(parse_request(&data).is_none());

        // config_len = 3 with a 19-byte body: config fills the body exactly
        // and the unguarded disable read used to panic at body[19].
        let mut data = vec![PROTO_VERSION];
        data.extend_from_slice(&19u32.to_le_bytes()); // total_len = 19
        data.extend_from_slice(&0u32.to_le_bytes()); // cwd_len = 0
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes()); // keymap_len = 0
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&3u16.to_le_bytes()); // config_len = 3
        data.resize(HEADER_LEN + 19, 0);
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_exact_boundary_caps() {
        // Max-length fields must be accepted; one over must be rejected.
        let ok = encode_request_v1(&"a".repeat(MAX_CWD_LEN), 0, None, 0, None, false);
        assert!(parse_request(&ok).is_some(), "cwd_len == MAX_CWD_LEN must fit");
        let over = encode_request_v1(&"a".repeat(MAX_CWD_LEN + 1), 0, None, 0, None, false);
        assert!(parse_request(&over).is_none(), "cwd_len == MAX_CWD_LEN + 1 must be rejected");

        let ok = encode_request_v1(".", 0, Some(&"k".repeat(MAX_KEYMAP_LEN)), 0, None, false);
        assert!(parse_request(&ok).is_some(), "keymap_len == MAX_KEYMAP_LEN must fit");
        let over = encode_request_v1(".", 0, Some(&"k".repeat(MAX_KEYMAP_LEN + 1)), 0, None, false);
        assert!(parse_request(&over).is_none(), "keymap_len == MAX_KEYMAP_LEN + 1 must be rejected");

        let ok = encode_request_v1(".", 0, None, 0, Some(&"c".repeat(MAX_CONFIG_LEN)), false);
        assert!(parse_request(&ok).is_some(), "config_len == MAX_CONFIG_LEN must fit");
        let over = encode_request_v1(".", 0, None, 0, Some(&"c".repeat(MAX_CONFIG_LEN + 1)), false);
        assert!(parse_request(&over).is_none(), "config_len == MAX_CONFIG_LEN + 1 must be rejected");
    }

    #[test]
    fn parse_request_non_utf8_cwd_is_lossy() {
        let mut data = encode_request_v1(".", 0, None, 0, None, false);
        // cwd_len = 1, so the cwd byte is body[4]; corrupt it.
        data[HEADER_LEN + 4] = 0xff;
        let req = parse_request(&data);
        assert!(req.is_some());
        // from_utf8_lossy replaces invalid sequences with U+FFFD
        assert!(!req.unwrap().cwd.as_os_str().is_empty());
    }

    #[test]
    fn parse_request_non_utf8_keymap_and_config_are_lossy() {
        // keymap/config decode with the same lossy rule as the cwd.
        let keymap_b = [0xffu8, 0xfe];
        let config_b = [0x80u8, 0x81];
        let body_len = FIXED_BODY_LEN + keymap_b.len() + config_b.len();
        let mut data = vec![PROTO_VERSION];
        data.extend_from_slice(&(body_len as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes()); // cwd_len = 0
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&(keymap_b.len() as u16).to_le_bytes());
        data.extend_from_slice(&keymap_b);
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&(config_b.len() as u16).to_le_bytes());
        data.extend_from_slice(&config_b);
        data.push(0);
        let req = parse_request(&data).unwrap();
        assert!(req.props.keymap.is_some());
        assert!(req.props.starship_config.is_some());
    }
}
