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

unused compute_cache_key import

disable caching doesn't seem to work

make sure the git bust gets deleted/cleaned up

add a "starship timings" but for the daemon
