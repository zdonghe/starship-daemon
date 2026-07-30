use std::collections::HashMap;
use std::ffi::c_void;
use std::mem;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::ffi::{self, HANDLE, DWORD, LPVOID};
use crate::gitignore::{GitignoreFilter, load_gitignore, is_ignored_str};

const FILE_LIST_DIRECTORY: DWORD = 1;
const FILE_SHARE_READ: DWORD = 1;
const FILE_SHARE_WRITE: DWORD = 2;
const FILE_SHARE_DELETE: DWORD = 4;
const OPEN_EXISTING: DWORD = 3;
const FILE_FLAG_OVERLAPPED: DWORD = 0x40000000;
const FILE_FLAG_BACKUP_SEMANTICS: DWORD = 0x02000000;
const FILE_NOTIFY_CHANGE_FILE_NAME: DWORD = 1;
const FILE_NOTIFY_CHANGE_DIR_NAME: DWORD = 2;
const FILE_NOTIFY_CHANGE_LAST_WRITE: DWORD = 0x10;
const DIRTY_COOLDOWN_MS: u64 = 100;
const CHANGE_BUF_SIZE: u32 = 65536;

fn is_git_internal(path: &str) -> bool {
    let trimmed = path.trim_start_matches('/');
    if let Some(rest) = trimmed.strip_prefix(".git/") {
        rest != "index" && rest != "HEAD" && !rest.starts_with("refs/heads/") && !rest.starts_with("refs/remotes/") && rest != "refs/stash" && rest != "packed-refs"
    } else {
        trimmed == ".git"
    }
}

fn extract_watcher_paths(buf: &[u8]) -> Vec<(String, u32)> {
    if buf.len() < 12 { return vec![]; }
    let mut paths = Vec::new();
    let mut offset = 0usize;
    loop {
        if offset + 12 > buf.len() { break; }
        let next = u32::from_le_bytes(buf[offset..offset+4].try_into().unwrap()) as usize;
        let action = u32::from_le_bytes(buf[offset+4..offset+8].try_into().unwrap());
        let name_len = u32::from_le_bytes(buf[offset+8..offset+12].try_into().unwrap()) as usize;
        if offset + 12 + name_len > buf.len() || name_len < 2 { break; }
        let name_slice = &buf[offset+12..offset+12+name_len];
        let name_wide: Vec<u16> = name_slice.chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        let name = String::from_utf16(&name_wide).unwrap_or_default();
        let normalized = name.replace('\\', "/").trim_start_matches('/').to_string();
        paths.push((normalized, action));
        if next == 0 { break; }
        offset += next;
    }
    paths
}

pub struct WatchEntry {
    repo_root: PathBuf,
    dir_handle: HANDLE,
    change_buf: Vec<u8>,
    pub change_event: HANDLE,
    overlapped: ffi::OVERLAPPED,
    dirty: bool,
    dirty_cooldown: Instant,
    generation: u64,
    ignore: Option<GitignoreFilter>,
}

impl Drop for WatchEntry {
    fn drop(&mut self) {
        unsafe {
            if self.dir_handle != ffi::INVALID_HANDLE_VALUE {
                if ffi::CancelIoEx(self.dir_handle, &mut self.overlapped as *mut _ as *mut c_void) != 0 {
                    let mut bytes: DWORD = 0;
                    let _ = ffi::GetOverlappedResult(self.dir_handle, &mut self.overlapped as *mut _ as *mut c_void, &mut bytes, 1);
                }
                ffi::CloseHandle(self.dir_handle);
            }
            ffi::CloseHandle(self.change_event);
        }
    }
}

pub struct WatcherState {
    pub entries: Vec<WatchEntry>,
    generations: HashMap<PathBuf, u64>,
}

impl WatcherState {
    pub fn new() -> Self {
        WatcherState { entries: Vec::new(), generations: HashMap::new() }
    }

    pub fn generation(&self, repo_root: &Path) -> u64 {
        self.generations.get(repo_root).copied().unwrap_or(0)
    }

    pub fn ensure(&mut self, repo_root: &Path) {
        if self.generations.contains_key(repo_root) { return; }
        let dir_handle;
        let change_event;
        unsafe {
            let wide = ffi::to_wide_path(repo_root);
            dir_handle = ffi::CreateFileW(wide.as_ptr(), FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(), OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut());
            if dir_handle == ffi::INVALID_HANDLE_VALUE {
                self.generations.insert(repo_root.to_path_buf(), 0);
                return;
            }
            change_event = ffi::CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
            if change_event.is_null() {
                ffi::CloseHandle(dir_handle);
                self.generations.insert(repo_root.to_path_buf(), 0);
                return;
            }
        }
        let ignore = load_gitignore(repo_root);
        self.entries.push(WatchEntry {
            repo_root: repo_root.to_path_buf(),
            dir_handle,
            change_buf: vec![0u8; CHANGE_BUF_SIZE as usize],
            change_event,
            overlapped: unsafe { mem::zeroed() },
            dirty: false,
            generation: 1,
            dirty_cooldown: Instant::now(),
            ignore,
        });
        let idx = self.entries.len() - 1;
        start_watch(&mut self.entries[idx]);
        self.generations.insert(repo_root.to_path_buf(), 1);
    }

    pub fn handle_event(&mut self, idx: usize) {
        if idx >= self.entries.len() { return; }
        let rw = &mut self.entries[idx];
        unsafe {
            let mut bytes: DWORD = 0;
            let ok = ffi::GetOverlappedResult(rw.dir_handle, &mut rw.overlapped as *mut _ as *mut c_void, &mut bytes, 1);
            if ok != 0 {
                if bytes == 0 {
                    rw.dirty = true;
                    rw.dirty_cooldown = Instant::now();
                } else {
                    let len = (bytes as usize).min(rw.change_buf.len());
                    let paths = extract_watcher_paths(&rw.change_buf[..len]);
                    for (path, _action) in paths {
                        if is_git_internal(&path) { continue; }
                        if let Some(ref ig) = rw.ignore {
                            if is_ignored_str(ig, &path) { continue; }
                        }
                        rw.dirty = true;
                        rw.dirty_cooldown = Instant::now();
                    }
                }
            }
            start_watch(rw);
        }
    }

    pub fn poll(&mut self) {
        for i in 0..self.entries.len() {
            let rc = unsafe { ffi::WaitForSingleObject(self.entries[i].change_event, 0) };
            if rc == ffi::WAIT_OBJECT_0 {
                self.handle_event(i);
            }
        }
    }

    pub fn process_dirty(&mut self) {
        let now = Instant::now();
        for rw in &mut self.entries {
            if !rw.dirty { continue; }
            if now.duration_since(rw.dirty_cooldown) < Duration::from_millis(DIRTY_COOLDOWN_MS) { continue; }
            rw.generation += 1;
            rw.dirty = false;
            self.generations.insert(rw.repo_root.clone(), rw.generation);
        }
    }
}

fn start_watch(rw: &mut WatchEntry) {
    unsafe {
        ffi::ResetEvent(rw.change_event);
        rw.overlapped = mem::zeroed();
        rw.overlapped.h_event = rw.change_event;
        let mut bytes: DWORD = 0;
        let ok = ffi::ReadDirectoryChangesW(rw.dir_handle, rw.change_buf.as_mut_ptr() as LPVOID, CHANGE_BUF_SIZE, 1,
            FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE,
            &mut bytes, &mut rw.overlapped as *mut _ as *mut c_void, std::ptr::null());
        if ok == 0 {
            rw.dirty = true;
            rw.dirty_cooldown = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_internal(path: &str) -> bool { is_git_internal(path) }

    #[test]
    fn git_index_is_not_internal() {
        assert!(!is_internal(".git/index"));
    }

    #[test]
    fn git_head_is_not_internal() {
        assert!(!is_internal(".git/HEAD"));
    }

    #[test]
    fn git_refs_heads_is_not_internal() {
        assert!(!is_internal(".git/refs/heads/main"));
        assert!(!is_internal(".git/refs/heads/feature"));
    }

    #[test]
    fn git_stash_is_not_internal() {
        assert!(!is_internal(".git/refs/stash"));
    }

    #[test]
    fn git_objects_is_internal() {
        assert!(is_internal(".git/objects/ab/cdef1234"));
    }

    #[test]
    fn git_logs_is_internal() {
        assert!(is_internal(".git/logs/HEAD"));
    }

    #[test]
    fn git_config_is_internal() {
        assert!(is_internal(".git/config"));
    }

    #[test]
    fn git_description_is_internal() {
        assert!(is_internal(".git/description"));
    }

    #[test]
    fn git_remotes_is_not_internal() {
        assert!(!is_internal(".git/refs/remotes/origin/main"));
    }

    #[test]
    fn git_tags_is_internal() {
        assert!(is_internal(".git/refs/tags/v1.0"));
    }

    #[test]
    fn git_hooks_is_internal() {
        assert!(is_internal(".git/hooks/pre-commit"));
    }

    #[test]
    fn dot_git_dir_is_internal() {
        assert!(is_internal(".git"));
    }

    #[test]
    fn non_git_files_are_not_internal() {
        assert!(!is_internal("somefile.txt"));
        assert!(!is_internal("src/main.rs"));
        assert!(!is_internal(".gitignore"));
    }

    #[test]
    fn leading_slash_is_stripped() {
        assert!(!is_internal("/.git/index"));
        assert!(!is_internal("/.git/HEAD"));
        assert!(is_internal("/.git/objects/ab/cdef1234"));
    }

    #[test]
    fn extract_empty_buffer() {
        let r = extract_watcher_paths(&[]);
        assert!(r.is_empty());
    }

    #[test]
    fn extract_short_buffer() {
        let r = extract_watcher_paths(&[0u8; 11]);
        assert!(r.is_empty());
    }

    #[test]
    fn extract_single_record() {
        let name_bytes: Vec<u16> = "hello.txt\0".encode_utf16().collect();
        let name_byte_len = (name_bytes.len() * 2) as u32;
        let mut buf = Vec::new();
        let rec_len = 12 + name_byte_len;
        let next = if rec_len % 4 == 0 { rec_len } else { rec_len + 4 - (rec_len % 4) };
        buf.extend_from_slice(&next.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&name_byte_len.to_le_bytes());
        for c in &name_bytes { buf.extend_from_slice(&c.to_le_bytes()); }
        while buf.len() < next as usize { buf.push(0); }

        let paths = extract_watcher_paths(&buf);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].0, "hello.txt");
        assert_eq!(paths[0].1, 1);
    }

    #[test]
    fn extract_with_trailing_null_terminates() {
        let name_bytes: Vec<u16> = "a.txt\0extra\0".encode_utf16().collect();
        let name_byte_len = (name_bytes.len() * 2) as u32;
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&name_byte_len.to_le_bytes());
        for c in &name_bytes { buf.extend_from_slice(&c.to_le_bytes()); }

        let paths = extract_watcher_paths(&buf);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].0, "a.txt");
    }

    #[test]
    fn extract_backslash_normalized() {
        let name_bytes: Vec<u16> = "sub\\dir\\f.txt\0".encode_utf16().collect();
        let name_byte_len = (name_bytes.len() * 2) as u32;
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&3u32.to_le_bytes());
        buf.extend_from_slice(&name_byte_len.to_le_bytes());
        for c in &name_bytes { buf.extend_from_slice(&c.to_le_bytes()); }

        let paths = extract_watcher_paths(&buf);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].0, "sub/dir/f.txt");
    }

    #[test]
    fn extract_multiple_records() {
        let names = ["first.txt\0", "second.txt\0"];
        let mut buf = Vec::new();
        for name in &names {
            let name_bytes: Vec<u16> = name.encode_utf16().collect();
            let name_byte_len = (name_bytes.len() * 2) as u32;
            let record_len = 12 + name_byte_len;
            let aligned = if record_len % 4 == 0 { record_len } else { record_len + 4 - (record_len % 4) };
            buf.extend_from_slice(&aligned.to_le_bytes());
            buf.extend_from_slice(&4u32.to_le_bytes());
            buf.extend_from_slice(&name_byte_len.to_le_bytes());
            for c in &name_bytes { buf.extend_from_slice(&c.to_le_bytes()); }
            while buf.len() < aligned as usize { buf.push(0); }
        }

        let paths = extract_watcher_paths(&buf);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].0, "first.txt");
        assert_eq!(paths[1].0, "second.txt");
    }

    #[test]
    fn ensure_inserts_generation_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub");
        std::fs::create_dir_all(&p).unwrap();
        let out = std::process::Command::new("git")
            .args(["init"]).current_dir(&p).output();
        if out.map_or(true, |o| !o.status.success()) { return; }

        let mut w = WatcherState::new();
        w.ensure(&p);
        assert!(w.generations.contains_key(&p));
        assert_eq!(w.generation(&p), 1);
    }

    #[test]
    fn poll_detects_file_creation() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub");
        std::fs::create_dir_all(&p).unwrap();
        let out = std::process::Command::new("git")
            .args(["init"]).current_dir(&p).output();
        if out.map_or(true, |o| !o.status.success()) { return; }

        let mut w = WatcherState::new();
        w.ensure(&p);
        assert_eq!(w.entries.len(), 1, "watcher should have 1 entry");
        assert_eq!(w.generation(&p), 1);

        std::thread::sleep(std::time::Duration::from_millis(50));

        std::fs::write(p.join("newfile.txt"), b"hello").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));

        let ev = w.entries[0].change_event;
        let rc = unsafe { ffi::WaitForSingleObject(ev, 0) };
        assert_eq!(rc, ffi::WAIT_OBJECT_0, "event should be signaled after file create");

        w.poll();
        assert!(w.entries[0].dirty, "entry should be dirty after poll");

        std::thread::sleep(std::time::Duration::from_millis(150));
        w.process_dirty();
        let g1 = w.generation(&p);
        assert!(g1 > 1, "gen should have been bumped from event, got {g1}");
        assert!(!w.entries[0].dirty, "entry should not be dirty after process_dirty");

        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(p.join("another_file.txt"), b"world").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));

        w.poll();
        assert!(w.entries[0].dirty, "entry should be dirty after second poll");

        std::thread::sleep(std::time::Duration::from_millis(150));
        w.process_dirty();
        let g2 = w.generation(&p);
        assert!(g2 > g1, "gen should have been bumped from second event, got {g2} vs {g1}");
        assert!(!w.entries[0].dirty, "entry should not be dirty after second process_dirty");
    }

    #[test]
    fn process_dirty_respects_cooldown() {
        let mut w = WatcherState::new();
        w.entries.push(WatchEntry {
            repo_root: PathBuf::from("/test"),
            dir_handle: ffi::INVALID_HANDLE_VALUE,
            change_buf: Vec::new(),
            change_event: std::ptr::null_mut(),
            overlapped: unsafe { std::mem::zeroed() },
            dirty: true,
            dirty_cooldown: Instant::now(),
            generation: 5,
            ignore: None,
        });
        w.generations.insert(PathBuf::from("/test"), 5);

        // Cooldown hasn't elapsed yet - no bump
        w.process_dirty();
        assert_eq!(w.generation(Path::new("/test")), 5);
        assert!(w.entries[0].dirty);
    }

    #[test]
    fn process_dirty_skips_non_dirty() {
        let mut w = WatcherState::new();
        w.entries.push(WatchEntry {
            repo_root: PathBuf::from("/a"),
            dir_handle: ffi::INVALID_HANDLE_VALUE,
            change_buf: Vec::new(),
            change_event: std::ptr::null_mut(),
            overlapped: unsafe { std::mem::zeroed() },
            dirty: true,
            dirty_cooldown: Instant::now() - Duration::from_millis(200),
            generation: 1,
            ignore: None,
        });
        w.entries.push(WatchEntry {
            repo_root: PathBuf::from("/b"),
            dir_handle: ffi::INVALID_HANDLE_VALUE,
            change_buf: Vec::new(),
            change_event: std::ptr::null_mut(),
            overlapped: unsafe { std::mem::zeroed() },
            dirty: false,
            dirty_cooldown: Instant::now(),
            generation: 10,
            ignore: None,
        });
        w.generations.insert(PathBuf::from("/a"), 1);
        w.generations.insert(PathBuf::from("/b"), 10);

        w.process_dirty();
        assert_eq!(w.generation(Path::new("/a")), 2, "dirty entry should bump");
        assert_eq!(w.generation(Path::new("/b")), 10, "non-dirty entry should not bump");
    }

    #[test]
    fn extract_alignment_padding() {
        let name_bytes: Vec<u16> = "a.txt\0".encode_utf16().collect();
        let name_byte_len = (name_bytes.len() * 2) as u32;
        let mut buf = Vec::new();
        let rec_len = 12 + name_byte_len;
        let aligned = if rec_len % 4 == 0 { rec_len } else { rec_len + 4 - (rec_len % 4) };
        buf.extend_from_slice(&aligned.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&name_byte_len.to_le_bytes());
        for c in &name_bytes { buf.extend_from_slice(&c.to_le_bytes()); }
        while buf.len() < aligned as usize { buf.push(0xAA); }

        let paths = extract_watcher_paths(&buf);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].0, "a.txt");
    }

    #[test]
    fn extract_action_codes_preserved() {
        for action in [1u32, 2, 3, 4, 5] {
            let name_bytes: Vec<u16> = "f.txt\0".encode_utf16().collect();
            let name_byte_len = (name_bytes.len() * 2) as u32;
            let mut buf = Vec::new();
            buf.extend_from_slice(&0u32.to_le_bytes());
            buf.extend_from_slice(&action.to_le_bytes());
            buf.extend_from_slice(&name_byte_len.to_le_bytes());
            for c in &name_bytes { buf.extend_from_slice(&c.to_le_bytes()); }

            let paths = extract_watcher_paths(&buf);
            assert_eq!(paths.len(), 1, "action={action}");
            assert_eq!(paths[0].1, action, "action={action}");
        }
    }

    #[test]
    fn extract_zero_length_name_breaks() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        let paths = extract_watcher_paths(&buf);
        assert_eq!(paths.len(), 0);
    }
}
