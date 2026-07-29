cold and warm testing -> git_status is the slowest 

now, benching what is causing the slowness in git_status

don't think anything can be done about the warm/cold perf in git_status

improve perf in cached state (i.e. pipe and other stuff)
- cache git repo location

external actions... i swear this was fixed, but still not. refer to git-fast
- mostly subdir, cwd_mtime is the issue
- going to take git-fast change

keymap cachekey does not seem to work

GetFileAttributesExW(..., 1, ...) — the earlier std::fs::metadata. GetFileAttributesExW i believe is faster


i think that the method is to split up the src even more, to separate the cache system

some sort of api that allows benches to access stuff more easily
- lib.rs allows any rust program to call another rust program's function

for watcher_gen, if > 0, then we just ignore all events. there is no point to keep tracking, if we know the cache is invalid

why tf do benches require recompilation

why did bust dir disappear?


