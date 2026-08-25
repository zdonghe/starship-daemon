use std::ffi::c_void;
use std::mem;
use std::path::{Path, PathBuf};
#[cfg(debug_assertions)]
use std::sync::Mutex;
#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(debug_assertions)]
use std::time::Instant;

use crate::ffi::{self, DWORD, HANDLE, LPVOID};
use crate::gitignore::{GitignoreFilter, is_ignored_str, load_gitignore};

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
const CHANGE_BUF_SIZE: u32 = 65536;

pub const MAX_WATCHED_REPOS: usize = 16;

fn ol_ptr(ol: &mut ffi::OVERLAPPED) -> *mut c_void {
    ol as *mut _ as *mut c_void
}

fn is_git_internal(path: &str) -> bool {
    let trimmed = path.trim_start_matches('/');
    if let Some(rest) = trimmed.strip_prefix(".git/") {
        rest != "index"
            && rest != "HEAD"
            && !rest.starts_with("refs/heads/")
            && !rest.starts_with("refs/remotes/")
            && rest != "refs/stash"
            && rest != "packed-refs"
    } else {
        trimmed == ".git"
    }
}

fn extract_watcher_paths(buf: &[u8]) -> Vec<(String, u32)> {
    if buf.len() < 12 {
        return vec![];
    }
    let mut paths = Vec::new();
    let mut offset = 0usize;
    loop {
        if offset + 12 > buf.len() {
            break;
        }
        let next = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as usize;
        let action = u32::from_le_bytes(buf[offset + 4..offset + 8].try_into().unwrap());
        let name_len =
            u32::from_le_bytes(buf[offset + 8..offset + 12].try_into().unwrap()) as usize;
        if offset + 12 + name_len > buf.len() || name_len < 2 {
            break;
        }
        let name_slice = &buf[offset + 12..offset + 12 + name_len];
        let name_wide: Vec<u16> = name_slice
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|&c| c != 0)
            .collect();
        let name = String::from_utf16(&name_wide).unwrap_or_default();
        let normalized = name.replace('\\', "/").trim_start_matches('/').to_string();
        paths.push((normalized, action));
        if next == 0 {
            break;
        }
        offset += next;
    }
    paths
}

pub struct WatchEntry {
    repo_root: PathBuf,
    dir_handle: HANDLE,
    change_buf: Vec<u8>,
    pub(crate) change_event: HANDLE,
    overlapped: ffi::OVERLAPPED,
    ignore: Option<GitignoreFilter>,
    pending: bool,
    armed: bool,
    last_touch: u64,
    version: u64,
}

impl Drop for WatchEntry {
    fn drop(&mut self) {
        unsafe {
            if self.dir_handle != ffi::INVALID_HANDLE_VALUE {
                if ffi::CancelIoEx(self.dir_handle, ol_ptr(&mut self.overlapped)) != 0
                    || ffi::GetLastError() != ffi::ERROR_NOT_FOUND
                {
                    let mut bytes: DWORD = 0;
                    let _ = ffi::GetOverlappedResult(
                        self.dir_handle,
                        ol_ptr(&mut self.overlapped),
                        &mut bytes,
                        1,
                    );
                }
                ffi::CloseHandle(self.dir_handle);
            }
            ffi::CloseHandle(self.change_event);
        }
    }
}

// box_collection suppressed: entries must stay heap-pinned. start_watch hands
// the OS raw pointers into change_buf/overlapped that remain valid until the
// overlapped I/O completes or is cancelled, and entries keep moving while new
// repos are pushed.
#[allow(clippy::vec_box)]
pub struct WatcherState {
    pub(crate) entries: Vec<Box<WatchEntry>>,

    epoch: u64,

    touch: u64,
}

impl Default for WatcherState {
    fn default() -> Self {
        Self::new()
    }
}

impl WatcherState {
    pub fn new() -> Self {
        WatcherState {
            entries: Vec::new(),
            epoch: 1,
            touch: 0,
        }
    }

    pub fn version(&self, repo_root: &Path) -> u64 {
        #[cfg(debug_assertions)]
        let _g = StatGuard::new(&STATS.nanos_version, None);
        self.version_inner(repo_root)
    }

    fn version_inner(&self, repo_root: &Path) -> u64 {
        for e in &self.entries {
            if e.repo_root == repo_root {
                return e.version;
            }
        }
        0
    }

    pub fn ensure(&mut self, repo_root: &Path) {
        #[cfg(debug_assertions)]
        let _g = StatGuard::new(&STATS.nanos_ensure, Some(&STATS.ensures));
        self.ensure_inner(repo_root);
    }

    fn ensure_inner(&mut self, repo_root: &Path) {
        for i in 0..self.entries.len() {
            if self.entries[i].repo_root == repo_root {
                self.entries[i].last_touch = self.touch;
                self.touch += 1;
                if !self.entries[i].armed {
                    let start_ok = start_watch(&mut self.entries[i]);
                    self.entries[i].armed = start_ok;
                    self.entries[i].pending = true;
                }
                return;
            }
        }
        if self.entries.len() >= MAX_WATCHED_REPOS {
            self.evict_lru();
        }
        let dir_handle;
        let change_event;
        unsafe {
            let wide = ffi::to_wide_path(repo_root);
            dir_handle = ffi::CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            );
            if dir_handle == ffi::INVALID_HANDLE_VALUE {
                return;
            }
            change_event = ffi::CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
            if change_event.is_null() {
                ffi::CloseHandle(dir_handle);
                return;
            }
        }
        let ignore = load_gitignore(repo_root);
        let mut entry = Box::new(WatchEntry {
            repo_root: repo_root.to_path_buf(),
            dir_handle,
            change_buf: vec![0u8; CHANGE_BUF_SIZE as usize],
            change_event,
            overlapped: unsafe { mem::zeroed() },
            ignore,
            pending: false,
            armed: false,
            last_touch: self.touch,
            version: self.epoch,
        });
        self.epoch += 1;
        self.touch += 1;

        entry.armed = start_watch(&mut entry);
        if !entry.armed {
            entry.pending = true;
        }
        self.entries.push(entry);
    }

    fn evict_lru(&mut self) {
        let mut victim = 0usize;
        let mut oldest = self.entries[0].last_touch;
        for (i, e) in self.entries.iter().enumerate().skip(1) {
            if e.last_touch < oldest {
                oldest = e.last_touch;
                victim = i;
            }
        }
        self.entries.swap_remove(victim);
        #[cfg(debug_assertions)]
        {
            count(&STATS.evictions);
        }
    }

    pub fn handle_event(&mut self, idx: usize) {
        if idx >= self.entries.len() {
            return;
        }
        #[cfg(debug_assertions)]
        let _gt = StatGuard::new(&STATS.nanos_handle_event, Some(&STATS.completions));
        let changed = {
            let we = &mut self.entries[idx];
            let mut bytes: DWORD = 0;
            let ok = unsafe {
                ffi::GetOverlappedResult(we.dir_handle, ol_ptr(&mut we.overlapped), &mut bytes, 1)
            };
            let changed = if ok != 0 {
                if bytes == 0 {
                    true
                } else {
                    let len = (bytes as usize).min(we.change_buf.len());
                    let paths = {
                        #[cfg(debug_assertions)]
                        let _gp = StatGuard::new(&STATS.nanos_parse, None);
                        extract_watcher_paths(&we.change_buf[..len])
                    };
                    #[cfg(debug_assertions)]
                    {
                        if stats_on() {
                            STATS
                                .records
                                .fetch_add(paths.len() as u64, Ordering::Relaxed);
                        }
                    }

                    let mut reload = false;
                    for (path, _) in &paths {
                        if path == ".gitignore" {
                            reload = true;
                            break;
                        }
                    }
                    if reload {
                        #[cfg(debug_assertions)]
                        let _gr = StatGuard::new(&STATS.nanos_reload, Some(&STATS.reloads));
                        we.ignore = load_gitignore(&we.repo_root);
                    }
                    let visible = {
                        #[cfg(debug_assertions)]
                        let _gf = StatGuard::new(&STATS.nanos_filter, None);
                        let mut visible = false;
                        #[cfg(debug_assertions)]
                        let mut internal = 0u64;
                        #[cfg(debug_assertions)]
                        let mut ignored = 0u64;

                        for (path, _) in &paths {
                            if is_git_internal(path) {
                                #[cfg(debug_assertions)]
                                {
                                    internal += 1;
                                }
                            } else if let Some(ref ig) = we.ignore {
                                if is_ignored_str(ig, path) {
                                    #[cfg(debug_assertions)]
                                    {
                                        ignored += 1;
                                    }
                                    #[cfg(debug_assertions)]
                                    {
                                        if stats_on() {
                                            let mut sp = STATS.dropped_samples.lock().unwrap();
                                            if sp.len() < 5 {
                                                sp.push(path.clone());
                                            }
                                        }
                                    }
                                } else {
                                    visible = true;
                                }
                            } else {
                                visible = true;
                            }
                        }
                        #[cfg(debug_assertions)]
                        {
                            if stats_on() {
                                repo_stats(&we.repo_root, paths.len() as u64, internal, ignored);
                            }
                        }
                        visible
                    };
                    reload || visible
                }
            } else {
                true
            };
            #[cfg(debug_assertions)]
            let _gr2 = StatGuard::new(&STATS.nanos_rearm, None);
            let start_ok = start_watch(we);
            #[cfg(debug_assertions)]
            {
                if start_ok {
                    count(&STATS.re_arms);
                }
            }
            we.armed = start_ok;
            changed || !start_ok
        };
        if changed {
            self.entries[idx].pending = true;
        }
    }

    pub fn flush(&mut self) {
        #[cfg(debug_assertions)]
        let _g = StatGuard::new(&STATS.nanos_flush, None);
        for e in &mut self.entries {
            if e.pending {
                e.pending = false;
                e.version = self.epoch;
                self.epoch += 1;
                #[cfg(debug_assertions)]
                {
                    count(&STATS.bumps);
                }
            }
        }
    }

    pub fn poll(&mut self) {
        self.process_signaled();
        self.flush();
    }

    pub fn process_signaled(&mut self) {
        #[cfg(debug_assertions)]
        let _g = StatGuard::new(&STATS.nanos_sweep, None);
        for i in 0..self.entries.len() {
            #[cfg(debug_assertions)]
            {
                count(&STATS.sweep_checks);
            }
            if unsafe { ffi::WaitForSingleObject(self.entries[i].change_event, 0) }
                == ffi::WAIT_OBJECT_0
            {
                #[cfg(debug_assertions)]
                {
                    count(&STATS.wakes);
                }
                self.handle_event(i);
            }
        }
    }

    pub fn num_entries(&self) -> usize {
        self.entries.len()
    }

    pub fn change_event(&self, idx: usize) -> HANDLE {
        self.entries[idx].change_event
    }
}

fn start_watch(we: &mut WatchEntry) -> bool {
    unsafe {
        ffi::ResetEvent(we.change_event);
        we.overlapped = mem::zeroed();
        we.overlapped.h_event = we.change_event;
        let mut bytes: DWORD = 0;
        ffi::ReadDirectoryChangesW(
            we.dir_handle,
            we.change_buf.as_mut_ptr() as LPVOID,
            CHANGE_BUF_SIZE,
            1,
            FILE_NOTIFY_CHANGE_FILE_NAME
                | FILE_NOTIFY_CHANGE_DIR_NAME
                | FILE_NOTIFY_CHANGE_LAST_WRITE,
            &mut bytes,
            ol_ptr(&mut we.overlapped),
            std::ptr::null(),
        ) != 0
    }
}

// ---- debug-only (all cfg(debug_assertions)) ----
#[cfg(debug_assertions)]
pub struct RepoStat {
    pub repo: PathBuf,
    pub completions: u64,
    pub dropped_git_internal: u64,
    pub dropped_gitignored: u64,
}

#[cfg(debug_assertions)]
pub struct WatcherStats {
    pub wakes: AtomicU64,
    pub sweep_checks: AtomicU64,
    pub completions: AtomicU64,
    pub records: AtomicU64,
    pub reloads: AtomicU64,
    pub re_arms: AtomicU64,
    pub bumps: AtomicU64,
    pub ensures: AtomicU64,
    pub evictions: AtomicU64,
    pub nanos_sweep: AtomicU64,
    pub nanos_handle_event: AtomicU64,
    pub nanos_parse: AtomicU64,
    pub nanos_filter: AtomicU64,
    pub nanos_rearm: AtomicU64,
    pub nanos_reload: AtomicU64,
    pub nanos_flush: AtomicU64,
    pub nanos_ensure: AtomicU64,
    pub nanos_version: AtomicU64,
    pub per_repo: Mutex<Vec<RepoStat>>,
    pub dropped_samples: Mutex<Vec<String>>,
}

#[cfg(debug_assertions)]
impl Default for WatcherStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(debug_assertions)]
impl WatcherStats {
    pub const fn new() -> Self {
        WatcherStats {
            wakes: AtomicU64::new(0),
            sweep_checks: AtomicU64::new(0),
            completions: AtomicU64::new(0),
            records: AtomicU64::new(0),
            reloads: AtomicU64::new(0),
            re_arms: AtomicU64::new(0),
            bumps: AtomicU64::new(0),
            ensures: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            nanos_sweep: AtomicU64::new(0),
            nanos_handle_event: AtomicU64::new(0),
            nanos_parse: AtomicU64::new(0),
            nanos_filter: AtomicU64::new(0),
            nanos_rearm: AtomicU64::new(0),
            nanos_reload: AtomicU64::new(0),
            nanos_flush: AtomicU64::new(0),
            nanos_ensure: AtomicU64::new(0),
            nanos_version: AtomicU64::new(0),
            per_repo: Mutex::new(Vec::new()),
            dropped_samples: Mutex::new(Vec::new()),
        }
    }
}

#[cfg(debug_assertions)]
pub static STATS: WatcherStats = WatcherStats::new();

#[cfg(debug_assertions)]
pub static STATS_ENABLED: AtomicBool = AtomicBool::new(false);

#[cfg(debug_assertions)]
fn stats_on() -> bool {
    STATS_ENABLED.load(Ordering::Relaxed)
}

#[cfg(debug_assertions)]
struct StatGuard {
    timer: Option<(Instant, &'static AtomicU64)>,
    counter: Option<&'static AtomicU64>,
}

#[cfg(debug_assertions)]
impl StatGuard {
    fn new(elapsed: &'static AtomicU64, counter: Option<&'static AtomicU64>) -> Self {
        if stats_on() {
            StatGuard {
                timer: Some((Instant::now(), elapsed)),
                counter,
            }
        } else {
            StatGuard {
                timer: None,
                counter: None,
            }
        }
    }
}

#[cfg(debug_assertions)]
impl Drop for StatGuard {
    fn drop(&mut self) {
        if let Some((start, elapsed)) = self.timer.take() {
            if let Some(c) = self.counter.take() {
                c.fetch_add(1, Ordering::Relaxed);
            }
            elapsed.fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
    }
}

#[cfg(debug_assertions)]
fn count(c: &AtomicU64) {
    if stats_on() {
        c.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(debug_assertions)]
fn repo_stats(repo: &Path, completions: u64, internal: u64, ignored: u64) {
    let mut m = STATS.per_repo.lock().unwrap();
    if let Some(s) = m.iter_mut().find(|s| s.repo == repo) {
        s.completions += completions;
        s.dropped_git_internal += internal;
        s.dropped_gitignored += ignored;
    } else {
        m.push(RepoStat {
            repo: repo.to_path_buf(),
            completions,
            dropped_git_internal: internal,
            dropped_gitignored: ignored,
        });
        if m.len() > 64 {
            m.remove(0);
        }
    }
}

#[cfg(debug_assertions)]
pub fn drain_stats() -> String {
    fn take(a: &AtomicU64) -> u64 {
        a.swap(0, Ordering::Relaxed)
    }
    let mut out = String::new();
    out.push_str(&format!(
        "counters: wakes={} sweep_checks={} completions={} records={} reloads={} rearms={} bumps={} ensures={} evictions={}\n",
        take(&STATS.wakes), take(&STATS.sweep_checks), take(&STATS.completions),
        take(&STATS.records), take(&STATS.reloads), take(&STATS.re_arms),
        take(&STATS.bumps), take(&STATS.ensures), take(&STATS.evictions),
    ));
    out.push_str(&format!(
        "us: sweep={:.1} handle_event={:.1} parse={:.1} filter={:.1} rearm={:.1} reload={:.1} flush={:.1} ensure={:.1} version={:.1}\n",
        take(&STATS.nanos_sweep) as f64 / 1000.0,
        take(&STATS.nanos_handle_event) as f64 / 1000.0,
        take(&STATS.nanos_parse) as f64 / 1000.0,
        take(&STATS.nanos_filter) as f64 / 1000.0,
        take(&STATS.nanos_rearm) as f64 / 1000.0,
        take(&STATS.nanos_reload) as f64 / 1000.0,
        take(&STATS.nanos_flush) as f64 / 1000.0,
        take(&STATS.nanos_ensure) as f64 / 1000.0,
        take(&STATS.nanos_version) as f64 / 1000.0,
    ));
    let per_repo = {
        let mut m = STATS.per_repo.lock().unwrap();
        std::mem::take(&mut *m)
    };
    for r in per_repo {
        out.push_str(&format!(
            "  repo {:?}: completions={} dropped_internal={} dropped_ignored={}\n",
            r.repo, r.completions, r.dropped_git_internal, r.dropped_gitignored,
        ));
    }
    let samples = {
        let mut m = STATS.dropped_samples.lock().unwrap();
        std::mem::take(&mut *m)
    };
    for s in samples {
        out.push_str(&format!("  dropped: {s}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_internal(path: &str) -> bool {
        is_git_internal(path)
    }

    fn wait_for_version_bump(w: &mut WatcherState, repo: &std::path::Path, before: u64) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            w.poll();
            if w.version(repo) > before {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            if std::time::Instant::now() > deadline {
                panic!("version did not increase within 5s (before={before})");
            }
        }
    }

    fn wait_for_version_stable(w: &mut WatcherState, repo: &std::path::Path) -> u64 {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            w.poll();
            let v = w.version(repo);
            std::thread::sleep(std::time::Duration::from_millis(150));
            w.poll();
            if w.version(repo) == v {
                return v;
            }
            if std::time::Instant::now() > deadline {
                panic!("version did not become stable within 5s (v={v})");
            }
        }
    }

    #[test]
    fn boxed_arm_filters_ignored_writes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("repo");
        std::fs::create_dir_all(p.join("a").join("x")).unwrap();
        std::fs::write(p.join(".gitignore"), "a/**/b\n").unwrap();
        let mut w = WatcherState::new();
        w.ensure(&p);
        let v0 = w.version(&p);
        std::fs::write(p.join("a").join("x").join("b"), b"hello").unwrap();
        for _ in 0..10 {
            w.poll();
            assert_eq!(
                w.version(&p),
                v0,
                "a/x/b must match anchored a/**/b and not bump"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        std::fs::write(p.join("visible.txt"), b"hello").unwrap();
        wait_for_version_bump(&mut w, &p, v0);
    }

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
        let next = if rec_len.is_multiple_of(4) {
            rec_len
        } else {
            rec_len + 4 - (rec_len % 4)
        };
        buf.extend_from_slice(&next.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&name_byte_len.to_le_bytes());
        for c in &name_bytes {
            buf.extend_from_slice(&c.to_le_bytes());
        }
        while buf.len() < next as usize {
            buf.push(0);
        }

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
        for c in &name_bytes {
            buf.extend_from_slice(&c.to_le_bytes());
        }

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
        for c in &name_bytes {
            buf.extend_from_slice(&c.to_le_bytes());
        }

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
            let aligned = if record_len.is_multiple_of(4) {
                record_len
            } else {
                record_len + 4 - (record_len % 4)
            };
            buf.extend_from_slice(&aligned.to_le_bytes());
            buf.extend_from_slice(&4u32.to_le_bytes());
            buf.extend_from_slice(&name_byte_len.to_le_bytes());
            for c in &name_bytes {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            while buf.len() < aligned as usize {
                buf.push(0);
            }
        }

        let paths = extract_watcher_paths(&buf);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].0, "first.txt");
        assert_eq!(paths[1].0, "second.txt");
    }

    #[test]
    fn poll_detects_file_creation() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub");
        std::fs::create_dir_all(&p).unwrap();

        let mut w = WatcherState::new();
        w.ensure(&p);
        assert_eq!(w.entries.len(), 1);

        let v0 = w.version(&p);

        std::fs::write(p.join("newfile.txt"), b"hello").unwrap();
        wait_for_version_bump(&mut w, &p, v0);

        let v1 = wait_for_version_stable(&mut w, &p);

        std::fs::write(p.join("another_file.txt"), b"world").unwrap();
        wait_for_version_bump(&mut w, &p, v1);
    }

    #[test]
    fn extract_alignment_padding() {
        let name_bytes: Vec<u16> = "a.txt\0".encode_utf16().collect();
        let name_byte_len = (name_bytes.len() * 2) as u32;
        let mut buf = Vec::new();
        let rec_len = 12 + name_byte_len;
        let aligned = if rec_len.is_multiple_of(4) {
            rec_len
        } else {
            rec_len + 4 - (rec_len % 4)
        };
        buf.extend_from_slice(&aligned.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&name_byte_len.to_le_bytes());
        for c in &name_bytes {
            buf.extend_from_slice(&c.to_le_bytes());
        }
        while buf.len() < aligned as usize {
            buf.push(0xAA);
        }

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
            for c in &name_bytes {
                buf.extend_from_slice(&c.to_le_bytes());
            }

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

    #[test]
    fn burst_coalesces_real_fs_events() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub");
        std::fs::create_dir_all(&p).unwrap();

        let mut w = WatcherState::new();
        w.ensure(&p);
        let v0 = w.version(&p);

        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(p.join("a.txt"), b"1").unwrap();
        std::fs::write(p.join("b.txt"), b"2").unwrap();
        std::fs::write(p.join("c.txt"), b"3").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));

        w.poll();
        let bumps = w.version(&p) - v0;
        assert!(bumps >= 1, "version must increase after burst");
        assert_eq!(
            bumps, 1,
            "burst must produce exactly 1 version bump, got {bumps}"
        );
    }

    #[test]
    fn flush_clears_pending_so_idle_poll_does_not_bump() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub");
        std::fs::create_dir_all(&p).unwrap();

        let mut w = WatcherState::new();
        w.ensure(&p);
        for _ in 0..5 {
            w.poll();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let v0 = w.version(&p);

        w.entries[0].pending = true;
        w.poll();
        let bumped = w.version(&p);
        assert!(bumped > v0, "pending flag must flush into a bump");
        w.poll();
        assert_eq!(w.version(&p), bumped, "idle poll must not bump again");
    }

    #[test]
    fn ensure_on_initial_arm_failure_bumps_once() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("file.txt");
        std::fs::write(&f, b"x").unwrap();
        let mut w = WatcherState::new();
        w.ensure(&f);
        assert_eq!(
            w.entries.len(),
            1,
            "entry created => CreateFileW succeeded, RDWC attempted"
        );
        assert!(w.entries[0].pending, "failed initial arm must set pending");
        let v0 = w.version(&f);
        w.poll();
        let v1 = w.version(&f);
        assert!(
            v1 > v0,
            "failed initial arm must bump once, got {v0} -> {v1}"
        );
        w.poll();
        assert_eq!(w.version(&f), v1, "second poll must not bump again");
    }

    #[test]
    fn many_repos_all_bump_after_reallocs() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = WatcherState::new();
        let mut repos = Vec::new();
        for i in 0..9 {
            let p = dir.path().join(format!("r{i}"));
            std::fs::create_dir_all(&p).unwrap();
            w.ensure(&p);
            repos.push(p);
        }
        assert_eq!(w.entries.len(), 9);
        for (i, p) in repos.iter().enumerate() {
            let v0 = w.version(p);
            std::fs::write(p.join(format!("f{i}.txt")), b"x").unwrap();
            wait_for_version_bump(&mut w, p, v0);
        }
    }

    #[test]
    fn lru_eviction_caps_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = WatcherState::new();
        for i in 0..49 {
            let p = dir.path().join(format!("r{i}"));
            std::fs::create_dir_all(&p).unwrap();
            w.ensure(&p);
        }
        assert_eq!(w.num_entries(), MAX_WATCHED_REPOS);
        let p0 = dir.path().join("r0");
        assert_eq!(w.version(&p0), 0, "oldest repo must be evicted at the cap");
        w.ensure(&p0);
        assert!(
            w.version(&p0) > 0,
            "re-ensured repo must get a fresh version"
        );
    }

    #[test]
    fn ensure_rearms_a_dead_entry() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("file.txt");
        std::fs::write(&f, b"x").unwrap();
        let mut w = WatcherState::new();
        w.ensure(&f);
        assert_eq!(w.entries.len(), 1);
        assert!(
            !w.entries[0].armed,
            "RDWC on a regular file must fail to arm"
        );
        w.ensure(&f);
        assert!(
            !w.entries[0].armed,
            "re-arm on a regular file must still fail"
        );
        assert!(w.entries[0].pending, "re-arm attempt must set pending");
        let v0 = w.version(&f);
        w.poll();
        assert!(
            w.version(&f) > v0,
            "pending from the re-arm must flush into a bump"
        );
    }

    #[test]
    fn reensure_version_exceeds_all_prior_versions() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        std::fs::create_dir_all(&a).unwrap();
        let mut w = WatcherState::new();
        w.ensure(&a);
        for _ in 0..60 {
            w.entries[0].pending = true;
            w.flush();
        }
        let pre = w.version(&a);
        assert!(
            pre > 60,
            "60 flushes must push the version past 60, got {pre}"
        );
        for i in 0..MAX_WATCHED_REPOS {
            let r = dir.path().join(format!("r{i}"));
            std::fs::create_dir_all(&r).unwrap();
            w.ensure(&r);
        }
        assert_eq!(
            w.version(&a),
            0,
            "repo must be evicted after MAX_WATCHED_REPOS more repos"
        );
        w.ensure(&a);
        assert!(
            w.version(&a) > pre,
            "re-ensured version must exceed every prior version"
        );
    }

    #[test]
    fn gitignore_reload_picks_up_rule_changes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("repo");
        std::fs::create_dir_all(&p).unwrap();
        let mut w = WatcherState::new();
        w.ensure(&p);

        let v_start = w.version(&p);
        std::fs::write(p.join(".gitignore"), "target.txt\n").unwrap();
        wait_for_version_bump(&mut w, &p, v_start);
        let reload_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            w.poll();
            let live = w.entries[0]
                .ignore
                .as_ref()
                .is_some_and(|ig| ig.rules.iter().any(|r| r.parts == ["target.txt"]));
            if live {
                break;
            }
            assert!(
                std::time::Instant::now() < reload_deadline,
                "filter never reloaded"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let v = w.version(&p);
        std::fs::write(p.join("target.txt"), b"x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        w.poll();
        assert_eq!(
            w.version(&p),
            v,
            "file ignored by the current .gitignore must not bump"
        );

        std::fs::write(p.join(".gitignore"), "\n").unwrap();
        wait_for_version_bump(&mut w, &p, v);

        let v2 = w.version(&p);
        std::fs::write(p.join("target.txt"), b"y").unwrap();
        wait_for_version_bump(&mut w, &p, v2);
    }

    #[test]
    fn gitignore_self_ignore_still_bumps_reload() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("repo");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join(".gitignore"), "").unwrap();
        let mut w = WatcherState::new();
        w.ensure(&p);

        let v_start = w.version(&p);
        std::fs::write(p.join(".gitignore"), "*\n").unwrap();
        wait_for_version_bump(&mut w, &p, v_start);

        let live = w.entries[0]
            .ignore
            .as_ref()
            .is_some_and(|ig| ig.rules.iter().any(|r| r.parts == ["*"]));
        assert!(
            live,
            "the `*` rule must be live even though it ignores .gitignore itself"
        );
    }
}
