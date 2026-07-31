# Watcher design: LRU-capped per-repo watchers, event-driven (no polling)

Status: v5, IMPLEMENTED. Canonical design.
Supersedes the 8-item LRU-capped draft (conversation-only, v1), the reviewed
draft (v2), and `watcher-design.md` (shared-event Approach C, v5), which is
archived as superseded. v5 reflects the architectural review cycle (Agents 3-6)
that produced the final implementation.

## 1. Context and goal

The daemon watches each git repo with ReadDirectoryChangesW (RDWC). Each
WatchEntry owns a directory handle, a 64KB change buffer, a manual-reset
event, an OVERLAPPED, a gitignore filter, a pending flag, an armed flag, LRU
bookkeeping, and a version (watch.rs:63-74).

Since commit 253794e the per-entry events were removed from the main loop's
WaitForMultipleObjects (WFMO) wait set (unbounded set could exceed
MAXIMUM_WAIT_OBJECTS=64 -> WAIT_FAILED). Freshness relied on a 100ms WFMO
timeout: the loop woke every 100ms and scanned events (process_signaled). This
was the polling design.

Requirement (user): NO polling-based design. The daemon must sleep until an
event wakes it. v5 puts every WatchEntry's event back in the wait set, but the
entry count is hard-capped by an LRU policy (48), so the set is bounded: 8
sessions + 48 entries = 56 < 64. Event-driven delivery; no freshness timer.

Honest note: process_signaled runs unconditionally BEFORE the session loop, so
completions delivered before WFMO returns are applied to that iteration's
requests. A completion landing DURING service_session is picked up by the NEXT
iteration - one iteration of staleness, bounded and identical to the poll
design's window. And while any session is active the loop wakes every
IDLE_SWEEP_MS for the idle-reaper; the change event still wakes WFMO
immediately, so freshness stays event-driven, but the "sleeps until an event"
ideal holds strictly only with zero active sessions. The real value of this
swap is the bug fixes (two UAFs, dead-watch self-heal) PLUS the removal of the
100ms freshness timer. The user has chosen event-driven; the tradeoff is accepted.

## 2. Revision history

v1 (conversation-only draft) -> v2 (reviewed draft, Agent 2): applied B1, M1-M4,
L2, L4, L5. The v2 review table and findings are preserved in git history.

v2 -> v3 (Agent 3, architecture SIMPLIFY):
- CUT the ARM_RETRIES loop (ARM_RETRIES/ARM_RETRY_SLEEP_MS/1022 handling).
  Redundant with ensure()'s self-heal: a dead entry is re-armed on every touch.
- CUT the ungated scan (4.5 v2): with per-entry events back in the wait set,
  process_signaled returns to the GATED per-entry WFS form it had before
  253794e. No misprobe of unarmed entries because a zeroed OVERLAPPED is never
  probed - the gate is the event state, which is set only by a real completion.
- Replaced `last_used: Instant` with a `touch: u64` stamp dispenser (an LRU
  comparison, not a clock; immune to wall-clock skew).
- Kept the conditional INFINITE timeout: no active session -> WFMO blocks
  forever (hard no-polling guarantee); active session -> 1000ms tick for idle
  sweeps only.
- Added gitignore reload on .gitignore batch (was open item 7.1).

v3 -> v4 (Agent 4, cycle repeat):
- A1 blocking: the earlier design shared one epoch for both creation and
  touches; absolute-delta assertions in burst_coalesces_real_fs_events could
  break. Split into TWO counters: `epoch` (creation + flush) and `touch`
  (ensure touches). Verified fix.
- B3: revival must bump. ensure() sets pending=true on a re-arm regardless of
  success (a dead window may have dropped changes).
- B9: capacity/accessor. Confirmed existing `num_entries()` and
  `change_event(idx)` suffice for the wait set; no new accessor needed.
- B7: revert the Drop wait=0 spin to wait=1. With per-entry events the
  shared-event hazard is gone; the canonical MSDN cancel pattern (CancelIoEx +
  GetOverlappedResult(wait=1), skipping only on ERROR_NOT_FOUND) is safe.

v4 -> v5 (Agent 5 verified all findings; Agent 6 perf review APPROVE with one
correctness edge):
- Gitignore reload must run on the RAW extracted batch BEFORE the old filter
  is applied, else an old rule that ignores .gitignore would suppress the
  reload and bump, caching stale rules forever. Implemented in handle_event.
- Verified empirically in implementation: arming RDWC on a stack-local
  WatchEntry and then boxing it is a live UAF (see 4.2b). Fixed by arming
  through the Box.

v5 -> v5.1 (independent critique + verification):
- M1: an errored completion (GetOverlappedResult returns 0, e.g. the kernel
  reports overflow as ERROR_NOTIFY_ENUM_DIR instead of bytes==0) now counts as
  changed=true. Before, a successful re-arm after an errored completion
  silently dropped the whole buffered batch with no bump and no self-heal.
- M2: a reload is itself a visible change - if the new rules ignore .gitignore
  itself (e.g. `*`), the bump still fires. Before, the new filter suppressed
  the .gitignore path, caching the stale rules forever.
- Both fixes are one line each in handle_event; verified by new regression test
  gitignore_self_ignore_still_bumps_reload and by empirical probes (see 8).
- Doc wording (4.7 / Honest note) corrected: mid-session completions are
  picked up one iteration later (not "never older than kernel delivery
  latency"), and the loop wakes every IDLE_SWEEP_MS while a session is active.

## 3. Rejected alternatives

- Keep the 100ms poll (current): rejected by user (no polling).
- Shared-event Approach C (`watcher-design.md` v5, archived): one manual-reset
  event shared by all WatchEntry OVERLAPPED, 9 handles, no repo cap. Rejected
  in favor of this design per the user's choice. Its `armed` gate and Box fix
  are adopted here. The cap (48) is the cost of keeping per-repo wakeup
  granularity and per-entry events; LRU eviction makes the cap graceful (a miss
  on re-ensure, never a failure).
- Ungated scan (v2 4.5): rejected by Agent 3 once per-entry events returned to
  the wait set - redundant, and probing zeroed OVERLAPPED is undefined.
- ARM_RETRIES loop on ERROR_NOTIFY_ENUM_DIR (v2 4.3): rejected by Agent 3.
  ensure() re-arms dead entries on every touch, so a transient overflow kill
  self-heals on the next request for that repo.
- Unbounded per-entry events in the wait set (pre-253794e): WAIT_FAILED past
  64 handles. This design's cap restores boundedness.

## 4. Design

### 4.1 Load-bearing constraints

- MSDN GetOverlappedResult: "When an I/O operation is pending, the function
  that started the operation resets the hEvent member of the OVERLAPPED
  structure to the nonsignaled state." Every RDWC issue (initial arm AND
  re-arm) silently clears the per-entry event. Correctness MUST NOT depend on
  event state persisting across a re-arm.
- The OVERLAPPED must remain valid (same address) until the completion is read.
  Any move of a struct containing a live OVERLAPPED is a UAF (4.2, 4.2b).
- The wait set must stay under MAXIMUM_WAIT_OBJECTS=64. MAX_WATCHED_REPOS
  (48) must stay <= 55.

### 4.2 WatchEntry: stable heap address

```rust
pub struct WatchEntry {
    repo_root: PathBuf,
    dir_handle: HANDLE,
    change_buf: Vec<u8>,                 // 64KB
    pub(crate) change_event: HANDLE,     // per-entry manual-reset event
    overlapped: ffi::OVERLAPPED,         // stable address: entry lives in a Box
    ignore: Option<GitignoreFilter>,
    pending: bool,
    armed: bool,                         // only true while an RDWC is outstanding
    last_touch: u64,                     // LRU stamp; compared, not wall-clock
    version: u64,                        // per-entry version (flush target)
}
```

`entries: Vec<Box<WatchEntry>>` keeps each entry (and its OVERLAPPED) at a
stable heap address. The Vec-realloc UAF (by-value OVERLAPPED + `Vec<WatchEntry>`
moves the struct while RDWC holds a pointer to the old address; the kernel
writes the completion into freed memory and the copy keeps internal =
STATUS_PENDING, so a later probe hangs or misreads) is unreachable. The Box
also means eviction removal does not move neighbors. Element-type change only;
auto-deref keeps call sites (`entries[i].pending`, `entries[i].version`)
compiling.

### 4.2b Arming through the Box (new finding, verified in implementation)

A second, independent UAF: arming RDWC on a stack-local `WatchEntry` and THEN
moving it into a Box. The kernel holds the overlapped pointer to the stack
frame; on completion it writes `internal`/`internalHigh` through that stale
pointer (dead stack), signals the event (via the handle copied into the box),
and GetOverlappedResult on the boxed copy reads the box's own `internal` fields
- which are still STATUS_PENDING / zero. Result: every completion reports a
spurious `bytes==0` "overflow", so every write (including ignored ones) bumps.

Symptom (reproduced deterministically): with a loaded `a/**/b` filter, writing
`a/x/b` produced `paths=[("a/x/b",3)]` on HEAD (no bump) but an overflow
completion + spurious bump before the fix. The bug ALSO corrupts unrelated
process state: parallel integration tests intermittently failed with
ERROR_INVALID_USER_BUFFER (1784) on plain `std::fs::write`.

Rule: arm AFTER boxing. `let mut entry = Box::new(WatchEntry {...}); entry.armed
= start_watch(&mut entry); self.entries.push(entry);` - `&mut entry` auto-derefs
to the stable heap allocation; `push` moves only the 3-word Box pointer.

### 4.3 start_watch

```rust
fn start_watch(we: &mut WatchEntry) -> bool {
    unsafe {
        ffi::ResetEvent(we.change_event);
        we.overlapped = mem::zeroed();
        we.overlapped.h_event = we.change_event;
        let mut bytes: DWORD = 0;
        ffi::ReadDirectoryChangesW(we.dir_handle, we.change_buf.as_mut_ptr() as LPVOID,
            CHANGE_BUF_SIZE, 1,
            FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE,
            &mut bytes, ol_ptr(&mut we.overlapped), std::ptr::null()) != 0
    }
}
```

Unchanged from before. The explicit ResetEvent is kept - with a per-entry event
it is correct and cheap (no shared-event hazard). The caller's rule (4.2b): the
WatchEntry must already live at its final heap address.

### 4.4 ensure(): touch/re-arm or create, LRU evict-before-push

```rust
pub fn ensure(&mut self, repo_root: &Path) {
    for i in 0..self.entries.len() {
        if self.entries[i].repo_root == repo_root {
            self.entries[i].last_touch = self.touch;
            self.touch += 1;
            if !self.entries[i].armed {
                let start_ok = start_watch(&mut self.entries[i]);
                self.entries[i].armed = start_ok;
                self.entries[i].pending = true;  // revival bump: cover the dead window
            }
            return;
        }
    }
    if self.entries.len() >= MAX_WATCHED_REPOS {
        self.evict_lru(); // EVICT-BEFORE-PUSH: transient total stays < 64
    }
    // CreateFileW (FILE_LIST_DIRECTORY | FILE_FLAG_OVERLAPPED |
    //   FILE_FLAG_BACKUP_SEMANTICS), CreateEventW(manual=1),
    //   load_gitignore. On CreateFileW/event failure: return, no entry, no
    //   epoch consumed.
    let mut entry = Box::new(WatchEntry { repo_root: repo_root.to_path_buf(),
        dir_handle, change_buf: vec![0u8; CHANGE_BUF_SIZE],
        change_event, overlapped: unsafe { mem::zeroed() },
        ignore, pending: false, armed: false,
        last_touch: self.touch, version: self.epoch });
    self.epoch += 1;
    self.touch += 1;
    entry.armed = start_watch(&mut entry); // arm AFTER boxing (4.2b)
    if !entry.armed { entry.pending = true; } // preserves ensure_on_initial_arm_failure_bumps_once
    self.entries.push(entry);
}

fn evict_lru(&mut self) {
    let mut victim = 0usize;
    let mut oldest = self.entries[0].last_touch;
    for (i, e) in self.entries.iter().enumerate().skip(1) {
        if e.last_touch < oldest { oldest = e.last_touch; victim = i; }
    }
    self.entries.swap_remove(victim); // Drop runs; Box keeps other addresses stable
}
```

Two cases: entry alive (touch + re-arm if dead) vs no entry (create with a
fresh epoch). A re-ensured repo gets a NEW version by construction, so it can
never serve a stale cached render (4.8). Self-eviction is impossible (a repo
that just touched is never the LRU min). Single-threaded; no multi-request
race.

M1 note: a dead entry is re-armed on every ensure() touch, so a transient
overflow kill self-heals on the next request for that repo. A permanently
unwatchable repo (arm always fails) bumps on every ensure touch (pending re-set
each time) - always-fresh render, never stale. Known tradeoff, 7.4.

### 4.5 process_signaled: gated (per-entry WFS) + handle_event

```rust
pub fn process_signaled(&mut self) {
    for i in 0..self.entries.len() {
        if unsafe { ffi::WaitForSingleObject(self.entries[i].change_event, 0) } == ffi::WAIT_OBJECT_0 {
            self.handle_event(i);
        }
    }
}
```

GATED, as before 253794e. The ungated scan is gone (Agent 3): with per-entry
events in the wait set, an event is set only by a real completion for that
entry, so the gate never skips work and never misprobes an unarmed entry.

handle_event (watch.rs:186-229):

```rust
pub fn handle_event(&mut self, idx: usize) {
    if idx >= self.entries.len() { return; }
    let changed = {
        let we = &mut self.entries[idx];
        let mut bytes: DWORD = 0;
        let ok = unsafe {
            ffi::GetOverlappedResult(we.dir_handle, ol_ptr(&mut we.overlapped), &mut bytes, 1)
        };
        let changed = if ok != 0 {
            if bytes == 0 {
                true // overflow: buffer discarded, changes lost (MSDN)
            } else {
                let len = (bytes as usize).min(we.change_buf.len());
                let paths = extract_watcher_paths(&we.change_buf[..len]);
                // Reload the ignore filter when this batch touches .gitignore.
                // Check BEFORE filtering: the old filter may itself ignore
                // .gitignore and would otherwise suppress the reload and the
                // bump, caching the stale rules forever.
                let mut reload = false;
                for (path, _) in &paths {
                    if path == ".gitignore" { reload = true; break; }
                }
                if reload {
                    we.ignore = load_gitignore(&we.repo_root);
                }
                // A reload is itself a visible change even when the new rules
                // filter .gitignore out of this batch (e.g. a `*` rule):
                // without this the new rules would be cached with no bump and
                // the old ones kept forever (critique M2).
                let matches = paths.iter().any(|(path, _)| {
                    if is_git_internal(path) { return false; }
                    if let Some(ref ig) = we.ignore {
                        if is_ignored_str(ig, path) { return false; }
                    }
                    true
                });
                reload || matches
            }
        } else {
            // Errored completion (e.g. ERROR_NOTIFY_ENUM_DIR on kernel buffer
            // overflow): the buffered batch is lost, so treat it as a change
            // to avoid a stale cache with no self-heal (critique M1).
            true
        };
        let start_ok = start_watch(we); // re-arm; event is reset here (4.1)
        we.armed = start_ok;
        changed || !start_ok
    };
    if changed {
        self.entries[idx].pending = true;
    }
}
```

- wait=1 on GetOverlappedResult is safe: the per-entry event is reset on issue
  (4.1), so the wait succeeds only for this operation's completion.
- gitignore reload (perf-review edge): raw batch checked BEFORE the old filter;
  known residuals documented in 7.5.
- flush() (watch.rs:231-239): `if e.pending { e.pending = false; e.version =
  self.epoch; self.epoch += 1; }`. poll() (watch.rs:241-244) = process_signaled
  + flush, kept (tests call it).

### 4.6 Drop: canonical MSDN cancel pattern (wait=1)

```rust
impl Drop for WatchEntry {
    fn drop(&mut self) {
        unsafe {
            if self.dir_handle != ffi::INVALID_HANDLE_VALUE {
                if ffi::CancelIoEx(self.dir_handle, ol_ptr(&mut self.overlapped)) != 0
                    || ffi::GetLastError() != ffi::ERROR_NOT_FOUND
                {
                    let mut bytes: DWORD = 0;
                    let _ = ffi::GetOverlappedResult(self.dir_handle, ol_ptr(&mut self.overlapped), &mut bytes, 1);
                }
                ffi::CloseHandle(self.dir_handle);
            }
            ffi::CloseHandle(self.change_event);
        }
    }
}
```

ERROR_NOT_FOUND guard is required, not optional: it is the only thing
distinguishing never-armed (no cancel issued) from pending. The wait=1 spin
debate (v2 4.6) is settled by B7: with per-entry events the shared-event
early-satisfy hazard is gone, so the canonical MSDN pattern is correct AND
simpler. Eviction runs on the request hot path; a completed RDWC's cancel
returns promptly, so the wait is short.

### 4.7 Main loop (src/main.rs)

- Wait set: `handles` = 8 session events + up to 48 entry events = max 56 < 64
  (margin 8). `Vec::with_capacity(MAX_SESSIONS + MAX_WATCHED_REPOS)`, rebuilt
  fresh each iteration: `for s in &sessions { handles.push(s.event); }` then
  `for i in 0..watcher.num_entries() { handles.push(watcher.change_event(i)); }`.
  `handles.clear()` at loop end unchanged.
- Conditional timeout (the no-polling guarantee):
  `let timeout = if sessions.iter().any(|s| s.active) { IDLE_SWEEP_MS /* 1000 */ }
  else { ffi::INFINITE };`
  No active session -> WFMO blocks forever; the daemon sleeps until an event
  (connect or watcher completion) wakes it. Active session -> 1000ms tick ONLY
  for the idle-sweep (5s idle -> disconnect ~5-6s, L5), never for freshness; a
  change event still wakes WFMO immediately.
- NO dispatch guard change: the session loop checks each session event with
  WaitForSingleObject(ev,0); a watcher wake (rc = WAIT_OBJECT_0+8+k) simply
  finds no signaled session - benign. process_signaled runs exactly once per
  iteration regardless of rc; entry events only accelerate WFMO.
- `watcher.process_signaled()` stays unconditional BEFORE the session loop:
  drains on every wake and before every request, so a change landing during a
  render is picked up before the next request.
- Handle-lifetime invariant: eviction can only run inside
  service_session -> process_request -> ensure(), which runs AFTER WFMO returned
  and BEFORE handles.clear() at loop end. An evicted entry's event handle is
  therefore never passed to a subsequent WFMO call (the array is rebuilt from
  live entries each iteration). Single-threaded.

### 4.8 Cache-key semantics (M3 fix; replaces pre-bump)

Two separate counters (A1 fix):

- `epoch: u64` - consumed ONLY by entry creation (initial version) and flush
  bumps. Starts at 1 so 0 unambiguously means "unknown repo".
- `touch: u64` - consumed ONLY by ensure() touches (LRU stamps). Starts at 0.

`version(repo_root)`: linear scan -> `entries[i].version`, else 0 (preserves
unknown_repo_returns_zero_version).

Cache safety: a repo's version is constant while its entry is alive (bumps only
via flush). On eviction the entry (and its version) is dropped; on re-ensure a
NEW epoch is assigned. Epochs are globally unique and never reused, so a
re-ensured repo's cache key can NEVER match any previously cached key for that
repo -> always a fresh render. Strictly stronger than pre-bump (which could in
principle collide with an old key) and it handles evict-before-flush: a pending
bump lost to eviction is immaterial because the re-ensure epoch supersedes it.
The absolute-delta assertions in burst_coalesces_real_fs_events hold because
creation and touch counters are independent (A1). No stale render, no leak.

## 5. Correctness invariants

1. No UAF from Vec realloc: entries are Box<WatchEntry>; OVERLAPPED at a stable
   heap address. (Pre-fix reachable at the 2nd push and on eviction.)
2. No UAF from arming-then-moving: RDWC is armed only AFTER boxing (4.2b). This
   was a second, every-creation live bug, found and fixed during implementation.
3. No misprobe of unarmed entries: process_signaled is gated by per-entry event
   state; a zeroed OVERLAPPED is never probed.
4. No premature completion wait: Drop uses the canonical CancelIoEx + wait=1
   pattern, skipping the wait only on ERROR_NOT_FOUND.
5. Wait set bounded: 8 + 48 = 56 < MAXIMUM_WAIT_OBJECTS=64 (margin 8).
   MAX_WATCHED_REPOS must stay <= 55.
6. Handle lifetime: eviction only runs between WFMO return and handles.clear().
7. Stale-cache safety: version is constant within an entry's lifetime; a new
   epoch on re-ensure forces a cache miss. No stale render ever served.
8. Dead watches self-heal: ensure() re-arms armed==false entries on every touch.
   Churn bursts cannot kill a watch.
9. Event-driven freshness: a completion wakes WFMO at kernel delivery time; the
   maintenance timeout is not part of delivery. With no active session the
   daemon blocks indefinitely (INFINITE) - no polling, by construction.
10. .gitignore rules reload: a batch containing .gitignore reloads the filter
    BEFORE the old filter is applied; the reload itself forces a bump even when
    the new rules filter .gitignore out of the batch (M2).

## 6. Test impact

Existing tests pass unchanged (all use <= 2 repos):

- tests/watch.rs: per-entry event still exists; WFS(change_event(0),0) ==
  WAIT_TIMEOUT after ensure still holds (event starts unsignaled, no
  completion yet).
- watch.rs unit tests: `entries[i].pending` / `entries[i].version` compile via
  Box auto-deref. ensure_on_initial_arm_failure_bumps_once unaffected
  (file-handle arm fails on attempt 1 -> pending bumps once).
- unknown_repo_returns_zero_version, poll_increases_version_on_file_change,
  burst_coalesces_real_fs_events, flush_clears_pending... all use relative
  version assertions -> epoch approach passes.
- tests/mtime_git_ops.rs, tests/multi_instance.rs: <= 2 repos; use
  assert_version_bumped (relative).

New tests (watch.rs mod tests, all deadline-bounded):

1. many_repos_all_bump_after_reallocs: 9 subdirs under one tempdir, ensure() all
   9 (forces Vec reallocs), write to each, assert all 9 bump. Pre-fix outcome is
   nondeterministic (clean-fail / spurious-pass via stale internalHigh==0 ->
   bytes==0 -> changed=true / heap corruption), NOT a guaranteed hang.
2. lru_eviction_caps_entries: 49 repos -> num_entries()==48, first repo
   evicted, re-ensure gets a fresh version. Exercises eviction hot-path Drop.
3. ensure_rearms_a_dead_entry: ensure() a file path (arm fails), re-ensure ->
   re-arm attempted (armed stays false), pending bumped on poll (7.4 behavior).
4. reensure_version_exceeds_all_prior_versions: ensure + 60 flushes on repo A,
   evict via 48 more repos, re-ensure A -> version exceeds every prior version.
   v2-regression canary for the monotonicity guarantee.
5. gitignore_reload_picks_up_rule_changes: rule added via .gitignore write
   suppresses matching writes after reload; rule removed re-enables bumps.
   Handles the trailing reload (the filter may land one completion after the
   bump - the test drains until the rule is live before asserting suppression).
6. boxed_arm_filters_ignored_writes: a/**/b rule; writing a/x/b must not bump
   and a visible write must. Regression for 4.2b (the spurious-overflow bug).
7. gitignore_self_ignore_still_bumps_reload: writing `*` to an empty
   .gitignore must bump even though the new rule ignores .gitignore itself.
   Regression for M2.

Full suite: 124 lib + 82 integration = 206 tests, green.

## 7. Open items (deferred)

- 7.1 >48 simultaneous repos: LRU eviction means the evicted repo re-renders
  (cache miss) on every switch back. Inherent to the cap; the shared-event
  alternative removes the cap but was not chosen. Documented, not a defect.
- 7.2 Duplicate path spellings (case/trailing sep/`..`) create duplicate
  entries + versions, burning LRU slots. Fix: canonicalize repo_root in ensure().
- 7.3 Permanently-unwatchable repo (arm always fails) bumps version on every
  ensure touch -> always-fresh render. Correct (never stale) but defeats
  caching for that repo.
- 7.4 Gitignore reload residuals: overflow batches (bytes==0) and errored
  completions bypass reload (both still bump - M1/M2 - but the filter may lag
  until the next .gitignore batch); root-level .gitignore only (no nested
  gitignores).
- 7.5 The constants INFINITE, ERROR_NOT_FOUND (1168), ERROR_IO_INCOMPLETE (996)
  now live in ffi.rs (moved from local scopes).

## 8. Verified claims (evidence)

- "the function that started the operation resets the hEvent member ... to
  the nonsignaled state" - MSDN GetOverlappedResult.
- "If the buffer overflows, ReadDirectoryChangesW will still return true, but
  the entire contents of the buffer are discarded and the lpBytesReturned
  parameter will be zero" - MSDN ReadDirectoryChangesW.
- CancelIoEx: wait only skipped when the call fails with ERROR_NOT_FOUND
  (1168) - MSDN "Canceling Pending I/O Operations".
- OVERLAPPED must remain valid until the operation completes - MSDN OVERLAPPED
  docs. Vec<WatchEntry> by-value OVERLAPPED UAF reachable at the 2nd push
  (capacity 1->2), not 5th+ (capacity growth 1->2->4->8).
- NEW, empirically verified (4.2b): arming RDWC on a stack-local entry and then
  boxing it yields a spurious bytes==0 "overflow" on every completion (bump on
  every write, including ignored ones) and corrupts unrelated process state
  (intermittent ERROR_INVALID_USER_BUFFER on plain std::fs::write under
  parallel load). Confirmed by running the identical probe on HEAD (arm after
  push) - no overflow, no bump - vs the buggy arm-then-box.
- "A notify change request is being completed and the information is not being
  returned in the caller's buffer" - ERROR_NOTIFY_ENUM_DIR (1022), WinError.h.
  An overflowed ReadDirectoryChangesW can complete as an errored completion,
  not only as bytes==0 success (critique M1). Not reproduced on this NTFS
  system: a 20k-file burst without polling produced bytes==0 overflow, never
  an errored completion - so M1 is a latent robustness gap, and the fix (bump
  on any errored completion) is conservative with no downside.
- NEW, empirically verified (M2): writing `*` to an empty .gitignore reloads
  the filter but the new rule ignores .gitignore itself - without the reload
  bump the version does not change and the stale rules survive. Probe assert
  failed before the fix; passes after.
- MAXIMUM_WAIT_OBJECTS = 64, constant across architectures (confirmed in
  mingw-w64 and Wine winnt.h; the "128 on x64" claim is a myth). Cap math
  8+48=56 < 64 holds.
- GetOverlappedResult(wait=TRUE) on a per-op manual-reset event: event reset on
  issue; the wait succeeds only for that operation's completion - MSDN.
