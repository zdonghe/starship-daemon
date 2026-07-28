cold and warm testing -> git_status is the slowest 

now, benching what is causing the slowness in git_status


don't think anything can be done about the warm/cold perf in git_status

improve perf in cached state (i.e. pipe and other stuff)
- cache git repo location
