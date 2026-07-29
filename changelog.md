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

perhaps just isolate the time module, as that is the one that invalidates often
- agent is working on that

i think that the method is to split up the src even more, to separate the cache system

some sort of api that allows benches to access stuff more easily
- lib.rs allows any rust program to call another rust program's function


watcher_gen > 0 results in mtime checking? is watcher_gen the number of things changed?

watcher lowk can just look at cwd_mtime, index_mtime, branch_mtime, and remote_mtime (the stuff in .git), which just removes those cachekeys
- agent claims that all of that can be simplified to just when watcher_gen > 0, which might be right. it's just that in the future, if we want to do like even nicher level optimzation (only update external actions, only check for git index level caching), then we would need to watch each individual one.
- bust dir

talking about some  Multi-directory minute tick: 5 dirs ~100μs vs ~75ms (750×)

why tf do benches require recompilation

why did bust dir disappear?

it seems that only the dotfiles repo is affected by the time change

simplify time only path
