keymap cachekey does not seem to work

GetFileAttributesExW(..., 1, ...) — the earlier std::fs::metadata. GetFileAttributesExW i believe is faster

i think that the method is to split up the src even more, to separate the cache system

some sort of api that allows benches to access stuff more easily
- lib.rs allows any rust program to call another rust program's function

for watcher_gen, if > 0, then we just ignore all events. there is no point to keep tracking, if we know the cache is invalid

why tf do benches require recompilation

do we just persistently watch repo's/folder handles, forever? gc approach from git-fast (introduces gc though)

persistent connection model would also allow us to stop watching repo handles forever.

improving .psm1 perf: there was something but i discarded

cache locality

think about background work

unit tests:
- ensure they are all actually accurate and thorough
- add unit tests for ipc
