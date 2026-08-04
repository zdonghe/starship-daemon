# Cold-call latency: where the time goes + improvement plan

Source: E2E breakdown harness (e2e.ps1, 100 cold samples @ 3s idle, cache-hit, throttle OFF/opt-out).
Measured on this machine; **absolute numbers vary run-to-run (C-state swings 690us-2.1ms), within-run ratios are stable.**

## Coauthoritive cold E2E (real Get-StarshipPrompt)

- cold: median ~1180us (trim10 1201us)
- warm: median ~207us
- cold-over-warm overhead: +973us
- Note: real interactive prompt() adds the prelude (Get-History exit-code, `$global:error[0]`,
  Width read, Write-Host), so users see ~1-5ms on deep idle. E2E above calls Get-StarshipPrompt directly.

## The 4 portions of the cold call (median us)

| # | portion | cold | warm | cold overhead |
|---|---------|-----:|-----:|--------------:|
| 1 | pipe read (daemon wake + render + transfer) | 293 | 22 | +471 (combined 1+2) |
| 2 | pipe write (client/OS wake) | 166 | 25 | see above |
| 3 | PowerShell/module overhead (invocation, branching, Parse) | ~580 | ~160 | +~420 |
| 4 | providerpath + frame build | ~144 | ~small | ~+100 |

Stage sum (1+2+4) = ~603us; the difference to E2E total (~1180us) is portion 3 (~580us).
Pipe round-trip (1+2 ~459us) is ~76% of the stage sum.

---

## Portion 1 - pipe read (daemon wake + render + transfer)

- **What**: time from flush to the response fully arriving in the client. Includes the daemon
  thread waking from C-state, EcoQoS/DVFS ramp, the render (cache-hit ~0, miss ~100-300us), and
  writing the response back.
- **Cost**: ~293us cold. The single biggest controllable wake cost.
- **Status**: DONE for the wake part - `STARSHIP_DAEMON_THROTTLE` opt-out (default). Confirmed
  significant: ON 838 vs OFF 722us median, ratio 1.16-1.45x, Mann-Whitney p=0.0224.
- **Remaining ideas (if pursuing)**:
  - Nothing safe left in-app: heartbeats/keep-awake/busy-threads/all provably backfire (b21-b23).
  - Environmental: high-performance power plan / disable core parking / PM-QoS to depth-limit C-states.
- **Verify**: throttle_stats.ps1 (150 iters interleaved, MWU).

## Portion 2 - pipe write (client/OS wake)

- **What**: time to push the frame into the pipe. Mostly the pwsh client's write syscall + the
  just-woken client process / OS path waking.
- **Cost**: ~166us cold (~25us warm).
- **Status**: not addressable from the daemon. Client-side EcoQoS opt-out measured = NO gain.
- **Remaining ideas**: none software-side; same environmental C-state tools as portion 1.

## Portion 3 - PowerShell/module overhead

- **What**: everything the PS layer adds on top of the raw pipe trip: module fn invocation, the
  keymap ternary, per-call `$env:STARSHIP_CONFIG` / `$env:STARSHIP_DAEMON_CACHE` reads, `Parse`,
  and (in real `prompt()`) the Get-History + `$global:error[0]` prelude and Width read.
- **Cost**: ~160us FIXED per call (warm E2E 207 vs bare pipe ~45), scaling to ~580us cold.
- **Status**: NOT done. This is the only leftover controllable software cost - it hits EVERY prompt.
- **Concrete micro-opts** (starship-daemon.psm1 + prompt()):
  - Hoist `$env:STARSHIP_DAEMON_CACHE` (static) to a `$script:` var read once at import.
  - Skip the `$cfgChanged` env re-read when unchanged (cache last `STARSHIP_CONFIG` value).
  - Slim `prompt()` prelude: avoid Get-History + `$global:error[0]` inspect every prompt;
    track last error once.
  - Cache Vi/emacs keymap detection minimally.
  - Target: reclaim ~30-50% of the ~160us floor (helps warm + cold).
- **Verify**: e2e.ps1 warm E2E delta before/after.

## Portion 4 - providerpath + frame build

- **What**: `$PWD.ProviderPath` (~53us) + `[StarshipFrame]::Build` (~91us, C#).
- **Cost**: ~144us cold, small warm.
- **Status**: mostly DONE - Build+Parse already moved into the compiled C# helper (StarshipFrame
  DLL); Build was ~1165us native PS before, ~91us now.
- **Remaining ideas**: cache `ProviderPath` rarely changes; negligible further gain. Leave.

---

## Cross-cutting

- **Bench-all vs reality gap**: bench-all measures a raw/tiny path; real E2E adds portions 3+4 and
  the prompt() prelude, and runs on deeper idle. Use e2e.ps1 (module, cache-hit, cold) for truth.
- **Do not**: add heartbeats, busy keep-awake threads, or client-side power-throttle opt-out -
  all measured regression or no-op (b21-b23).

---

## How the results were obtained + how to reproduce

### Setup / environment
- OS, 64-bit Windows; any recent CPU with C-states + EcoQoS.
- Machine idle ~2.5-3s between samples reaches C-package states; **longer idle = deeper wake, so
  absolute us climb**. Numbers here were captured at 3s idle (shallow) unless noted.
- Power plan + Turbo + current DVFS state change day-to-day; only the OFF-vs-ON ratio is stable.

### Measurement tools (in C:\Users\Dong\AppData\Local\Temp\opencode\)
- `e2e.ps1 -BudgetSeconds <n> -WarmSamples <m>` - REAL module E2E + per-stage breakdown.
  Invokes the actual `Get-StarshipPrompt` through module scope (`& $script:_mod { ... }`) and
  times 4 stages (ProviderPath / StarshipFrame::Build / pipe write+flush / pipe read) with QPC.
  Prints median + 10%-trimmed mean for cold (3s idle, cache-hit) and warm (back-to-back).
  Reports the numbers in this file's tables.
- `throttle_stats.ps1` - the ON-vs-OFF A/B used for portion 1. Two concurrent daemons on separate
  pipes, 30-iter warmup discarded, 150 iters/config interleaved, 3s idle/sample, QPC; reports
  median + trim10 + p10/25/50/75/90 + Mann-Whitney U p-value.
- `breakdown.ps1 -BudgetSeconds <n>` - raw pipe-only write/read split (no module), for the
  cold-over-warm overhead view of portions 1+2.

### The disciplines (why these numbers can be trusted)
- Mandatory warm-up; **first 20-50 iters discarded** (JIT / cold paths).
- **Interleave** the things being compared (A,B,A,B...) - never all-A then all-B - so drift hits
  both equally.
- **Median + 10%-trimmed mean**, not raw mean: cold-call latency is right-tailed (C-state spikes),
  so the mean lies high; the trim removes the top/bottom 10%.
- QPC via `[System.Diagnostics.Stopwatch]` only, never `Measure-Command`.
- For a claimed difference, a proper test (Mann-Whitney U), not eyeballing.

### Exact repro (the number in the tables)
```
# ensure release daemon built (current src; throttle opt-out is the default when
# STARSHIP_DAEMON_THROTTLE is unset):
cargo build --release

# cold E2E + stage breakdown (medians + trim10):
powershell -NoProfile -File C:\Users\Dong\AppData\Local\Temp\opencode\e2e.ps1 -BudgetSeconds 300 -WarmSamples 40
```
- e2e.ps1 sets cwd to C:\Users\Dong\Documents\Code\starship-daemon, imports the module via
  STARSHIP_DAEMON_PATH, starts the daemon, warms 25 calls, then samples cold every 3s until the
  budget elapses; finally a back-to-back warm run and the per-stage loop. It cleans up the daemon
  at the end.
- All frames use the same cwd => **cache-hit** after the first call (lru 256, stable watcher
  version, disableCache=0). "Cold" = the wake cost, not a render miss.
- Config inherited via STARSHIP_CONFIG -> the dotfiles starship.toml; default ~/.config equals it.

### Reproducing portion 1 (throttle OFF vs ON)
```
powershell -NoProfile -File C:\Users\Dong\AppData\Local\Temp\opencode\throttle_stats.ps1
```
Spawns daemon A with STARSHIP_DAEMON_THROTTLE=1 (ON) on pipe `starship-throttle-a`, daemon B without
(OFF, opt-out) on `starship-throttle-b`, interleaves 150 samples each after 30-iter discard, and
prints the MWU p-value. OFF should come out faster; a hit if p < 0.05.

### Caveats to trust the numbers
- Absolute us are not portable - rerun on this machine to compare; ratios within a run are stable.
- The 8-sample / 32-sample early attempts showed NO signal; only ~150 iters + interleave + MWU
  surfaced the real effect (portion 1). Don't under-power these tests.
- bench-all reports lower numbers because it exercises a thinner path; use e2e.ps1 for true E2E.
