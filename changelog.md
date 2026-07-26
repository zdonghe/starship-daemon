finish readme, have simple installation process like starship

get an understanding of the project: the llm is just going crazy and in circles

optimize perf, cold launch perf
- done

fix the red colour of error code persisting
- done

llm changes doesn't result in git status update?

currently working on implementing the git dir walk
- manual or gix?
- done, manual

config hot watch live reload (cache invalidation on hotreload change?)
- just going to do a syscall to check mtime
- need to ensure the syscall actually calls the starship_config location
- done

speed up compile time

reduce unused dependencies

per module caching?

no cache perf still poor

multiline statements look kinda off?

One thing worth flagging in the cached numbers

Max is 8.32ms against a median of 1.09ms — that's a ~7.6x ratio, way more elongated than your no-cache max/median ratio (~1.4x). Worth a quick look at what's driving that tail, since it's the only slightly odd shape in these three datasets. A few likely candidates:

Occasional cache invalidation/miss firing mid-run (e.g. if something touches $script:LastStarshipConfig state or a timestamp-based cache check occasionally fails)
Named pipe contention/GC pause on the client side
One or two outlier scheduler blips (200 samples isn't huge, a couple of unlucky ones can skew max without moving median)

daemon seems to keep a folder open?

1. Keep a persistent git repo handle open in the daemon (biggest win, no starship code changes)

If your daemon opens a fresh git2::Repository (or shells out to git) on every request, you're paying repo-discovery + index-parsing + object-db setup costs every single call, even with "no cache." Since your daemon is long-lived, you can keep an open Repository handle per working directory and just re-run status against it:

Avoids re-walking up the directory tree to find .git each time
Avoids re-reading packed-refs from disk each time
Still gives you a genuinely fresh status result — this isn't caching the answer, just the setup cost

This alone often accounts for a big chunk of "non-cache" latency, since repo discovery/config parsing is surprisingly expensive relative to the actual status diff.

2. Limit what git status actually scans

If you (or starship internally) call something equivalent to git status --porcelain, check whether untracked-file scanning is the expensive part:

rust
// libgit2 status options — this is the actual lever
let mut opts = git2::StatusOptions::new();
opts.include_untracked(false);       // biggest cost cutter on large repos
opts.recurse_untracked_dirs(false);
opts.include_ignored(false);

Untracked-directory recursion is usually the single most expensive part of git status on large repos (it has to stat every file not in the index). If starship's prompt (or your daemon) doesn't strictly need untracked-file detection for the segment you're rendering, skipping it is a real, safe win — not a hack.

3. Only recompute what actually changed (real incremental caching, not on/off)

Instead of disable_cache as a binary switch, consider a file-system watcher approach:

Daemon watches .git/index, .git/HEAD, and the working tree (via notify crate on Rust — wraps inotify/ReadDirectoryChangesW/FSEvents)
Cache stays valid until an actual file-system event says otherwise
You get near-cached-speed (~1ms) results that are always correct, rather than choosing between "fast but stale" and "correct but 130ms slower"

This is architecturally the better fix — it removes the cache/no-cache tradeoff entirely rather than optimizing the no-cache path in isolation.

4. Parallelize independent module evaluation

If your daemon (or starship itself, if you're calling into its module system) evaluates modules sequentially — git branch, git status, directory, language version detection, etc. — and several are independent of each other, running them concurrently (thread pool / async tasks) can cut wall time down to whichever module is slowest, rather than the sum of all of them.

5. Only if the above aren't enough — yes, touch git-status logic directly

If you've done 1-2 and it's still not fast enough, then yes, you'd be looking at the actual status-computation path (whether that's starship's own git_status module or your daemon's reimplementation of it). Things to look at there specifically:

Are you calling git status as a subprocess (shelling out) instead of using libgit2 bindings directly? Subprocess-per-status-check reintroduces a smaller version of the exact spawn problem you just solved for the whole prompt.
Is diff/ahead-behind calculation happening even when the user's config doesn't display it? Skipping unused segments based on the parsed starship.toml config (rather than computing everything and discarding) avoids wasted work.


