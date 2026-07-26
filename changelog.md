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

multiline statements look kinda off?

Max is 8.32ms against a median of 1.09ms — that's a ~7.6x ratio, way more elongated than your no-cache max/median ratio (~1.4x).
Possibilities:
Occasional cache invalidation/miss firing mid-run (e.g. if something touches $script:LastStarshipConfig state or a timestamp-based cache check occasionally fails)
Named pipe contention/GC pause on the client side
One or two outlier scheduler blips (200 samples isn't huge, a couple of unlucky ones can skew max without moving median)

daemon seems to keep a folder open?

1. Keep a persistent git repo handle open in the daemon (biggest win, no starship code changes)

If your daemon opens a fresh git2::Repository (or shells out to git) on every request, you're paying repo-discovery + index-parsing + object-db setup costs every single call, even with "no cache." Since your daemon is long-lived, you can keep an open Repository handle per working directory and just re-run status against it:

Avoids re-walking up the directory tree to find .git each time
Avoids re-reading packed-refs from disk each time

in starship config file, manually disable a ton of features that are not used

push gix change to starship?
