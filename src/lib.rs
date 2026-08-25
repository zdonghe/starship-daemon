use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use lru::LruCache;

#[cfg(feature = "fork")]
extern crate starship_fork as starship;

#[cfg(all(feature = "stock", feature = "fork"))]
compile_error!(
    "features `stock` and `fork` are mutually exclusive; build the fork variant with `cargo build --no-default-features --features fork`"
);

#[cfg(not(any(feature = "stock", feature = "fork")))]
compile_error!(
    "one of the `stock` or `fork` features is required (default is `fork`); build the stock variant with `cargo build --no-default-features --features stock`"
);

pub const PIPE_NAME: &str = r"\\.\pipe\starship-daemon";

pub fn pipe_name() -> String {
    std::env::var("STARSHIP_DAEMON_PIPE")
        .map(|n| {
            if n.starts_with(r"\\.\pipe\") {
                n
            } else {
                format!(r"\\.\pipe\{n}")
            }
        })
        .unwrap_or_else(|_| PIPE_NAME.to_string())
}

pub fn debug_enabled() -> bool {
    static ENABLED: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var("STARSHIP_DAEMON_DEBUG").as_deref() == Ok("1"));
    *ENABLED
}

pub mod cache;
pub mod daemon;
pub mod ffi;
pub mod gitignore;
pub mod render;
pub mod timings;
pub mod watch;

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
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(result) = cache.get(cwd) {
        return result.clone();
    }
    let actual = if cwd.as_os_str().is_empty() {
        Path::new(".")
    } else {
        cwd
    };
    let result = find_git_dir_uncached(actual);
    cache.put(actual.to_path_buf(), result.clone());
    result
}

// Binary wire protocol.
//
// REQUEST  [u8 request type][u32 LE total_len][body]  max frame 65536 = MAX_FRAME_LEN
//          type 1 = prompt (REQ_PROMPT), type 2 = timings report (REQ_TIMINGS);
//          both share the same body layout, only the response differs:
//          type 1 answers with the prompt, type 2 with a timings report.
// body     [u32 cwd_len][cwd lossy cap 32768]
//          [i32 status][u16 keymap_len][keymap lossy cap 256, empty -> None]
//          [u32 width][u16 config_len][config lossy cap 4096, empty -> None]
//          [u8 disable cache, non-zero -> true]
// RESPONSE [u32 LE len][utf8 payload]                 len = payload.len()
//
// The header is 5 bytes and the frame is capped at 65536, so total_len must
// satisfy 5 + total_len <= 65536, i.e. total_len <= 65531. An unknown request
// type, an over-cap field, or a field reading past the body returns None; the
// daemon then drops the connection and the client falls back to its plain prompt.
// Request type bytes. Deliberately independent constants: there is no
// protocol version negotiation anywhere - byte 0 of every request is a
// request TYPE. Coupling these to a "version" would silently break old
// clients if the values were ever bumped.
pub const REQ_PROMPT: u8 = 1;
pub const REQ_TIMINGS: u8 = 2;
pub const HEADER_LEN: usize = 5;
pub const MAX_FRAME_LEN: usize = 65536;
pub const MAX_TOTAL_LEN: usize = MAX_FRAME_LEN - HEADER_LEN;
pub const MAX_CWD_LEN: usize = 32768;
pub const MAX_KEYMAP_LEN: usize = 256;
pub const MAX_CONFIG_LEN: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Prompt,
    Timings,
}

pub fn valid_request_type(b: u8) -> bool {
    b == REQ_PROMPT || b == REQ_TIMINGS
}

pub struct ClientProps {
    pub status_code: i32,
    pub keymap: Option<String>,
    pub terminal_width: usize,
    pub starship_config: Option<String>,
    pub disable_cache: bool,
}

pub struct ParsedRequest {
    pub kind: RequestKind,
    pub cwd: PathBuf,
    pub props: ClientProps,
}

pub fn parse_request(data: &[u8]) -> Option<ParsedRequest> {
    if data.len() < HEADER_LEN {
        return None;
    }
    let kind = if data[0] == REQ_PROMPT {
        RequestKind::Prompt
    } else if data[0] == REQ_TIMINGS {
        RequestKind::Timings
    } else {
        return None;
    };
    let total_len = u32::from_le_bytes(data[1..HEADER_LEN].try_into().unwrap()) as usize;
    if total_len > MAX_TOTAL_LEN {
        return None;
    }
    if HEADER_LEN + total_len > data.len() {
        return None;
    }
    let body = &data[HEADER_LEN..HEADER_LEN + total_len];
    let mut off = 0usize;

    let cwd_len = read_u32(body, &mut off)? as usize;
    if cwd_len > MAX_CWD_LEN {
        return None;
    }
    let cwd =
        PathBuf::from(String::from_utf8_lossy(read_slice(body, &mut off, cwd_len)?).into_owned());

    let status_code = read_i32(body, &mut off)?;

    let keymap_len = read_u16(body, &mut off)? as usize;
    let keymap = if keymap_len == 0 {
        None
    } else {
        if keymap_len > MAX_KEYMAP_LEN {
            return None;
        }
        Some(String::from_utf8_lossy(read_slice(body, &mut off, keymap_len)?).into_owned())
    };

    let terminal_width = read_u32(body, &mut off)? as usize;

    let config_len = read_u16(body, &mut off)? as usize;
    let starship_config = if config_len == 0 {
        None
    } else {
        if config_len > MAX_CONFIG_LEN {
            return None;
        }
        Some(String::from_utf8_lossy(read_slice(body, &mut off, config_len)?).into_owned())
    };

    let disable = read_slice(body, &mut off, 1)?[0];

    Some(ParsedRequest {
        kind,
        cwd,
        props: ClientProps {
            status_code,
            keymap,
            terminal_width,
            starship_config,
            disable_cache: disable != 0,
        },
    })
}

pub fn encode_request(
    kind: u8,
    cwd: &str,
    status: i32,
    keymap: &str,
    width: u32,
    config: Option<&str>,
    disable: bool,
) -> Vec<u8> {
    let mut body = Vec::new();
    let cwd_b = cwd.as_bytes();
    body.extend_from_slice(&(cwd_b.len() as u32).to_le_bytes());
    body.extend_from_slice(cwd_b);
    body.extend_from_slice(&status.to_le_bytes());
    let keymap_b = keymap.as_bytes();
    body.extend_from_slice(&(keymap_b.len() as u16).to_le_bytes());
    body.extend_from_slice(keymap_b);
    body.extend_from_slice(&width.to_le_bytes());
    match config {
        Some(c) => {
            let config_b = c.as_bytes();
            body.extend_from_slice(&(config_b.len() as u16).to_le_bytes());
            body.extend_from_slice(config_b);
        }
        None => body.extend_from_slice(&0u16.to_le_bytes()),
    }
    body.push(disable as u8);

    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.push(kind);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn read_slice<'a>(body: &'a [u8], off: &mut usize, n: usize) -> Option<&'a [u8]> {
    if *off + n > body.len() {
        return None;
    }
    let s = &body[*off..*off + n];
    *off += n;
    Some(s)
}

fn read_u32(body: &[u8], off: &mut usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        read_slice(body, off, 4)?.try_into().unwrap(),
    ))
}

fn read_i32(body: &[u8], off: &mut usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        read_slice(body, off, 4)?.try_into().unwrap(),
    ))
}

fn read_u16(body: &[u8], off: &mut usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        read_slice(body, off, 2)?.try_into().unwrap(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXED_BODY_LEN: usize = 17;

    fn encode_body(
        cwd: &str,
        status: i32,
        keymap: Option<&str>,
        width: u32,
        config: Option<&str>,
        disable: bool,
    ) -> Vec<u8> {
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

    fn encode_request_v1(
        cwd: &str,
        status: i32,
        keymap: Option<&str>,
        width: u32,
        config: Option<&str>,
        disable: bool,
    ) -> Vec<u8> {
        let body = encode_body(cwd, status, keymap, width, config, disable);
        let mut data = vec![REQ_PROMPT];
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
        let data = encode_request_v1(
            "/home/user/project",
            139,
            Some("vi"),
            120,
            Some(r"C:\config.toml"),
            true,
        );
        let r = parse_request(&data);
        assert!(r.is_some());
        let req = r.unwrap();
        assert_eq!(req.cwd, PathBuf::from("/home/user/project"));
        assert_eq!(
            decode(&req),
            (
                139,
                Some("vi".to_string()),
                120,
                Some(r"C:\config.toml".to_string()),
                true
            )
        );
    }

    #[test]
    fn parse_request_minimal() {
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
        let data = encode_request_v1(".", 0, None, 0, None, false);
        let req = parse_request(&data).unwrap();
        assert_eq!(req.props.keymap, None);
    }

    #[test]
    fn parse_request_empty_data() {
        assert!(parse_request(&[]).is_none());
        assert!(parse_request(&[0u8; 5]).is_none());
        assert!(parse_request(&[REQ_PROMPT, 0, 0, 0, 0]).is_none());
    }

    #[test]
    fn parse_request_truncated() {
        let mut data = encode_request_v1("hello", 0, None, 0, None, false);
        data.truncate(data.len() - 1);
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_bad_type() {
        let mut data = encode_request_v1(".", 0, None, 0, None, false);
        data[0] = 0xFF;
        assert!(parse_request(&data).is_none());
        let mut data = encode_request_v1(".", 0, None, 0, None, false);
        data[0] = 0;
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_timings_type() {
        let mut data = encode_request_v1("/repo", 7, Some("vi"), 120, None, false);
        data[0] = REQ_TIMINGS;
        let r = parse_request(&data);
        assert!(r.is_some());
        let req = r.unwrap();
        assert_eq!(req.kind, RequestKind::Timings);
        assert_eq!(req.cwd, PathBuf::from("/repo"));
        assert_eq!(decode(&req), (7, Some("vi".to_string()), 120, None, false));
    }

    #[test]
    fn parse_request_total_len_over_cap() {
        let mut data = vec![REQ_PROMPT];
        data.extend_from_slice(&(MAX_TOTAL_LEN as u32 + 1).to_le_bytes());
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_total_len_exact_cap_missing_body() {
        let mut data = vec![REQ_PROMPT];
        data.extend_from_slice(&(MAX_TOTAL_LEN as u32).to_le_bytes());
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_trailing_bytes_tolerated() {
        let mut data = encode_request_v1(".", 0, Some("vi"), 120, None, true);
        data.push(0);
        data.push(0xde);
        let req = parse_request(&data);
        assert!(req.is_some(), "trailing body bytes must be tolerated");
        let req = req.unwrap();
        assert_eq!(decode(&req), (0, Some("vi".to_string()), 120, None, true));
    }

    #[test]
    fn parse_request_disable_nonzero_is_true() {
        let mut data = encode_request_v1(".", 0, None, 0, None, false);
        let last = data.len() - 1;
        data[last] = 0xff;
        let req = parse_request(&data).unwrap();
        assert!(
            req.props.disable_cache,
            "disable byte 0xff must decode to disable_cache = true"
        );
    }

    #[test]
    fn parse_request_cwd_len_overrun() {
        let mut data = vec![REQ_PROMPT];
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
        let body_len = FIXED_BODY_LEN + MAX_CWD_LEN + 1;
        let mut data = vec![REQ_PROMPT];
        data.extend_from_slice(&(body_len as u32).to_le_bytes());
        data.extend_from_slice(&((MAX_CWD_LEN + 1) as u32).to_le_bytes());
        data.resize(HEADER_LEN + body_len, 0);
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_keymap_overrun() {
        let mut data = vec![REQ_PROMPT];
        data.extend_from_slice(&(FIXED_BODY_LEN as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&100u16.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.push(0);
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_config_overrun() {
        let mut data = vec![REQ_PROMPT];
        data.extend_from_slice(&(FIXED_BODY_LEN as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&100u16.to_le_bytes());
        data.push(0);
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_cwd_eats_tail_never_panics() {
        for cwd_len in 1..=13usize {
            let mut data = vec![REQ_PROMPT];
            data.extend_from_slice(&(FIXED_BODY_LEN as u32).to_le_bytes());
            data.extend_from_slice(&(cwd_len as u32).to_le_bytes());
            data.resize(HEADER_LEN + FIXED_BODY_LEN, 0);
            assert!(
                parse_request(&data).is_none(),
                "cwd_len={cwd_len} must be rejected"
            );
        }

        let mut data = vec![REQ_PROMPT];
        data.extend_from_slice(&(FIXED_BODY_LEN as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&3u16.to_le_bytes());
        data.resize(HEADER_LEN + FIXED_BODY_LEN, 0);
        assert!(parse_request(&data).is_none());

        let mut data = vec![REQ_PROMPT];
        data.extend_from_slice(&19u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&3u16.to_le_bytes());
        data.resize(HEADER_LEN + 19, 0);
        assert!(parse_request(&data).is_none());
    }

    #[test]
    fn parse_request_exact_boundary_caps() {
        let ok = encode_request_v1(&"a".repeat(MAX_CWD_LEN), 0, None, 0, None, false);
        assert!(
            parse_request(&ok).is_some(),
            "cwd_len == MAX_CWD_LEN must fit"
        );
        let over = encode_request_v1(&"a".repeat(MAX_CWD_LEN + 1), 0, None, 0, None, false);
        assert!(
            parse_request(&over).is_none(),
            "cwd_len == MAX_CWD_LEN + 1 must be rejected"
        );

        let ok = encode_request_v1(".", 0, Some(&"k".repeat(MAX_KEYMAP_LEN)), 0, None, false);
        assert!(
            parse_request(&ok).is_some(),
            "keymap_len == MAX_KEYMAP_LEN must fit"
        );
        let over = encode_request_v1(
            ".",
            0,
            Some(&"k".repeat(MAX_KEYMAP_LEN + 1)),
            0,
            None,
            false,
        );
        assert!(
            parse_request(&over).is_none(),
            "keymap_len == MAX_KEYMAP_LEN + 1 must be rejected"
        );

        let ok = encode_request_v1(".", 0, None, 0, Some(&"c".repeat(MAX_CONFIG_LEN)), false);
        assert!(
            parse_request(&ok).is_some(),
            "config_len == MAX_CONFIG_LEN must fit"
        );
        let over = encode_request_v1(
            ".",
            0,
            None,
            0,
            Some(&"c".repeat(MAX_CONFIG_LEN + 1)),
            false,
        );
        assert!(
            parse_request(&over).is_none(),
            "config_len == MAX_CONFIG_LEN + 1 must be rejected"
        );
    }

    #[test]
    fn parse_request_non_utf8_cwd_is_lossy() {
        let mut data = encode_request_v1(".", 0, None, 0, None, false);

        data[HEADER_LEN + 4] = 0xff;
        let req = parse_request(&data);
        assert!(req.is_some());

        assert!(!req.unwrap().cwd.as_os_str().is_empty());
    }

    #[test]
    fn parse_request_non_utf8_keymap_and_config_are_lossy() {
        let keymap_b = [0xffu8, 0xfe];
        let config_b = [0x80u8, 0x81];
        let body_len = FIXED_BODY_LEN + keymap_b.len() + config_b.len();
        let mut data = vec![REQ_PROMPT];
        data.extend_from_slice(&(body_len as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
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

    #[test]
    fn encode_request_matches_reference_and_roundtrips() {
        assert_eq!(
            encode_request(REQ_PROMPT, "/repo", -3, "vi", 120, Some("cfg"), true),
            encode_request_v1("/repo", -3, Some("vi"), 120, Some("cfg"), true)
        );
        assert_eq!(
            encode_request(REQ_PROMPT, "", 0, "", 0, None, false),
            encode_request_v1("", 0, None, 0, None, false)
        );
        assert_eq!(
            encode_request(REQ_PROMPT, "/repo/日本語", 0, "", 0, None, false),
            encode_request_v1("/repo/日本語", 0, None, 0, None, false)
        );

        for (kind, expected) in [
            (REQ_PROMPT, RequestKind::Prompt),
            (REQ_TIMINGS, RequestKind::Timings),
        ] {
            let built = encode_request(kind, "/repo/日本語", -3, "vi", 120, Some("cfg"), true);
            assert_eq!(built[0], kind);
            let req = parse_request(&built).unwrap();
            assert_eq!(req.kind, expected);
            assert_eq!(req.cwd, PathBuf::from("/repo/日本語"));
            assert_eq!(
                decode(&req),
                (
                    -3,
                    Some("vi".to_string()),
                    120,
                    Some("cfg".to_string()),
                    true
                )
            );
        }
    }
}
