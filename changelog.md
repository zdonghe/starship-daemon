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

make sure every test in starship git status passes
- use it to run through some pass versions that i'm pretty sure work pretty decently
