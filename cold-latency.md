# Cold-call latency: where the time goes + improvement plan

Source: E2E breakdown harness (e2e.ps1, 100 cold samples @ 3s idle, cache-hit, throttle OFF/opt-out).
Portion-3 logic numbers come from the interleaved two-process module A/B harness (module_ab.ps1)
with its logic/prelude brackets, not the E2E window (which cannot resolve sub-100us logic deltas).
Measured on this machine; **absolute numbers vary run-to-run (C-state swings 690us-2.1ms), within-run ratios are stable.**

## Authoritative cold E2E (real Get-StarshipPrompt)

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
- **Status**: DONE for the module-logic micro-opts. Applied and verified reproducibly.
- **What was done** (starship-daemon.psm1, merged + synced to dotfiles):
  - Hoist `STARSHIP_DAEMON_PIPE` to `$script:DaemonPipeName` at import (A1). The cache-byte
    stays a per-call `$env:` read - caching it ignores mid-session toggles (correctness).
  - `STARSHIP_CONFIG` stays a per-call `$env:` read (A2 reverted to HEAD behavior). Caching it
    broke the documented "switch configs on the fly" - the env can change after import.
  - Cache the built request frame (`StarshipFrame::Build`) - only rebuild when any input changes
    (A3, `LastBuildKey`/`LastBuildBuf`). Recomputing `$cwd` each call makes cd/resize/exit rebuild.
  - A5 (`$PWD.Path` for FileSystem) was measured and REJECTED - it made the bundle *slower*
    (+3us, p<0.0001) and diverges from `ProviderPath` on PSDrive/UNC. Use plain `$PWD.ProviderPath`.
- **Verified** (in-situ logic-only QPC bracket, pipe/wake excluded, interleaved + Mann-Whitney):
  - A1+A2+A3+A5 bundle originally measured -38us warm (head 48 vs applied 10us, p<0.0001).
  - Corrected set (A1 pipe + A3 only, both per-call env reads restored): -8us warm logic
    (head 44 vs final 36us, p<0.0001, N=2000).
  - The correctness-safe savings are ~1/4 of the racy bundle - the A2/A1-cache-byte wins *were*
    the correctness bugs and are not recoverable without them.
- **Caveat / measurement pain**: E2E black-box A/B (the old Verify) could NOT resolve these gains -
    the ~90-100us pipe floor + wake noise spans ~±40us p10-p90, drowning the sub-10us logic signal.
  All earlier "portion 3 gives nothing reproducible" verdicts were artifacts of measuring through
  the E2E window. The internal logic-only bracket is the trustworthy tool (isolation bench +
  bracket agree; E2E-visible parity stays floored by pipe I/O).
- **Also applied (B1)**: `Invoke-Starship-PreCommand` is now guaranteed to exist - the module
  installs a no-op `global:` fallback at import if the user hasn't defined it yet, and
  `global:prompt` calls it unconditionally (no per-prompt `Test-Path`, no import-time cache).
  This is correct for hooks defined AFTER `Import-Module` (the profile's own hook is line ~42,
  after the import at line ~6), self-heals if the hook is added/removed, and it emits the
  `OSC 9;9;"cwd"` sequence that Windows Terminal needs for duplicate-pane CWD tracking (a cached
  `HasPreCommand` at import silently disabled the hook and broke pane duplication).
- **Also applied (B2)**: `$loc = CurrentLocation` is resolved lazily - only in the plain-fallback
  branch (`if (-not $result)`), not on the daemon-path. Negligible gain (~0us) but harmless.
- **Rejected / not applied**: B3 (error-path caching) a
  loss (slower) - keep the error-path gate already in HEAD. A4 width-cache breaks terminal-resize
  correctness (stale width) - rejected. A5 (see above) rejected.
- **Verify**: in-situ logic-only bracket (module_ab.ps1 -Worker module_ab_worker_internal.ps1)
  warm + cold; e2e.ps1 warm E2E delta is floored by pipe I/O and cannot resolve the ~8us.

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
- **E2E black-box can't resolve the small stuff**: for module/prelude logic deltas (<~100us) the
  E2E window's pipe floor + wake noise hides the signal - use the interleaved two-process
  module_ab harness AND its logic/prelude brackets (p<0.0001, reproducible), not a single-process
  before/after.
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

### The module A/B harness (portion 3) - `module_ab.ps1` + workers
This is what produced the portion-3 numbers. It also explains why the older single-process
`e2e.ps1` before/after could NOT resolve portion-3 gains, and why we now trust brackets.

- `module_ab.ps1` - one controller pwsh that pre-starts TWO daemons on separate named pipes
  (`starship-mod-a`, `starship-mod-b`), then spawns TWO worker pwsh subprocesses, each importing a
  DIFFERENT copy of the module (both named `starship-daemon.psm1` so the module name matches) and
  each bound to its OWN daemon/pipe. It interleaves the two workers **A,B,A,B** at a fixed idle gap
  (default 3s) and, per side, reports median + 10%-trimmed mean + p10/25/50/75/90 + a Mann-Whitney
  U p-value on the raw samples. Cleanup: `Stop-Process` all daemons + workers, remove the
  `ready_*.txt` handshake files between runs.
  - Params: `-N` (samples each side, default 150), `-Warm` (30 discarded), `-IdleMs` (3000),
    `-Tag`, `-AModule`/`-BModule` (paths to the two module copies), `-AOut`/`-BOut` (CSVs),
    `-Worker` (which worker script to use). A side defaults to `ab\head\starship-daemon.psm1`.
  - Reliable launch (needed to actually get output): run the controller via
    `Start-Process pwsh -ArgumentList @('-NoProfile','-File',<module_ab.ps1>,'-N',<n>,'-Warm',<w>,
    '-IdleMs',<ms>,'-Tag',<tag>,'-AOut',<csv>,'-BOut',<csv>) -WindowStyle Hidden
    -RedirectStandardOutput <log> -RedirectStandardError <err> -PassThru`, then `WaitForExit` and
    read `<log>`. Piping the harness through `Out-File`/`Tee-Object` silently drops output.

- Workers (each returns one measured value per call):
  - `module_ab_worker.ps1` (E2E) - times the full `Get-StarshipPrompt` call.
  - `module_ab_worker_prompt.ps1` (prompt) - imports PSReadLine first, calls `global:prompt`
    directly, reads the prelude value; used for B-suite prelude candidates.
  - `module_ab_worker_internal.ps1` (logic-only bracket) - after each call reads
    `$script:LastLogicUs`, a Stopwatch bracketed around ONLY the module-logic region
    (cwd -> keymap -> config -> cache -> `StarshipFrame::Build`), pipe/wake EXCLUDED.
  - `module_ab_worker_prelude.ps1` (prelude bracket) - times only `global:prompt`'s prelude
    (precommand presence test + `$loc` resolve + exit-code gate) via `$script:LastPreludeUs`,
    pipe EXCLUDED.

- **Why brackets are needed**: the E2E window has a ~90-100us pipe floor PLUS ~±40us wake noise
  (p10-p90). Portion-3's real logic savings (dozens of us) sit *below* that noise, so a black-box
  module A/B is non-reproducible (medians swing 147-204us run-to-run; deltas vanish). The internal
  brackets shrink the pipe/wake away and reveal the logic delta cleanly (the racy
  A1+A2+A3+A5 bundle measured -38us warm / -177us cold, reproducible, p<0.0001 every run;
  the corrected shipped set - A1 pipe + A3 - measures -8us warm, head 44 vs final 36us).
- Variant modules live under `C:\Users\Dong\AppData\Local\Temp\opencode\ab\` (`head` = pristine,
  `single_*` single-candidate, `*_internal` = pristine-HEAD logic + bracket). A pristine base
  `module_head.psm1` is hash-checked (B70BC4E8) before regenerating candidates. Rebuild generators:
  `mk_internal2.ps1` (A-suite logic-bracket variants), `mk_prelude.ps1` (B-suite prelude-bracket
  variants). Always re-verify 0 cross-contamination markers
  (`DaemonPipeName`/`DisableCacheByte`/`CachedConfig`/`LastBuildKey`/`$PWD.Provider.Name`) before
  trusting a module-side result.

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

# portion-3 module A/B (interleaved two-process, cold 3s idle):
Start-Process pwsh -ArgumentList @('-NoProfile','-File','C:\Users\Dong\AppData\Local\Temp\opencode\module_ab.ps1',`
  '-N',150,'-Warm',30,'-IdleMs',3000,'-Tag','<tag>','-AModule','<head copy>','-BModule','<variant copy>',`
  '-AOut','<csv>','-BOut','<csv>') -WindowStyle Hidden -RedirectStandardOutput <log> -RedirectStandardError <err> -PassThru

# same, warm (IdleMs 0, higher N) or a logic-only bracket via -Worker module_ab_worker_internal.ps1
```
- e2e.ps1 sets cwd to C:\Users\Dong\Documents\Code\starship-daemon, imports the module via
  STARSHIP_DAEMON_PATH, starts the daemon, warms 25 calls, then samples cold every 3s until the
  budget elapses; finally a back-to-back warm run and the per-stage loop. It cleans up the daemon
  at the end.
- All frames use the same cwd => **cache-hit** after the first call (lru 256, stable watcher
  version, disableCache=0). "Cold" = the wake cost, not a render miss.
- Config inherited via STARSHIP_CONFIG -> the dotfiles starship.toml; default ~/.config equals it.

### Reproducing the corrected module logic win (A1 pipe + A3)
```
# build the two logic-bracket module copies (pristine HEAD vs applied/corrected):
pwsh -NoProfile -File C:\Users\Dong\AppData\Local\Temp\opencode\mk_internal2.ps1

# warm logic bracket (N=2000, IdleMs 0): expect HEAD ~44us vs corrected ~36us, p<0.0001
# (A5 is dropped - it made the bundle slower; A2 stays a per-call env read for correctness)
# via module_ab.ps1 -Worker module_ab_worker_internal.ps1 -AModule ab\head_internal -BModule ab\<corrected>_internal
```

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
