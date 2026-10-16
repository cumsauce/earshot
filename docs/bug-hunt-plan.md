# Earshot: bug-hunt and cleanup plan

Date: 2026-10-16. Scope: prioritized, dependency-annotated changes that low/medium-effort
coding agents can execute with almost no extra context. Written from a read of the sources
listed in section 1; complements `docs/PLAN.md` (which holds the R0-R6 feature milestones).

## Execution status (living)

- **Wave A (independent, small): DONE.** F7, F8, F10, F11, F12, F13, F14, T1, T3, T4, T5.
- **Wave B (Java audio, serial): F1 -> F2 -> F4 -> F3 -> F9.**
- **Wave C (reverb numerics, serial, one golden move per commit): F5 -> F6 -> T2.**
- **Wave D (structure, after B and C): R1 -> R2, R3 in parallel.**

Commits landed so far: `f66d889` (F11+T3 ir), `f17956c` (F12 early), `1089394` (T1 reverb),
`dee3d4d` (F8 MaterialMap), `cefc332` (F13 SoundShaper), `440391a` (F10+F14 VoicePlayer),
`ab9c86f` (T4+T5 Java tests), plus F7 (ffi reach cap, committed separately).

## 1. What was read, and confidence

Read in full or near-full: `crates/earshot-ffi/src/{guard,voice,lib}.rs`;
`crates/earshot-sim/src/{reverb,engine}.rs`, `ir.rs` lines 1-560;
`crates/earshot-world/src/trace.rs` 30-329; Java `SoundShaper`, `SourceState`,
`EarlyReflections`, `SpaceReverb`, `ConvolutionReverb`, `VoicePlayer`, `MaterialMap`,
`WorldMirror`, `Propagation`, `SoundEngineMixin`, `ChannelMixin`, and the native-count paths
of `NativeBridge`. Git log of the last 15 commits with patches (the water-comb fix landed in
898520f: 0.5 ms grid + deterministic per-bucket jitter + sort; not re-proposed).

Confidence: **high** for FFI, reverb, ir early-tap/merge, MaterialMap, trace, engine, and the
Java audio classes; **medium** for `ir.rs` tail region (560+), `hear.rs`, `early.rs`,
`pathing.rs`; **low** for `ui/`, `EarshotCommands`, `HrtfFiles`, `xtask`.

Severity scale: **S1** crash/JVM abort or audio stuck for the session; **S2** wrong acoustics
or resource leak; **S3** cleanup/dead code.

## 2. Findings

### F1 - S1 - Uncaught RuntimeException on the sound thread leaves the shaping pass half-applied, every tick
`SoundShaper.java:329-435` (`update()`), `:785-795` (`call()`).
**Trigger:** only `setListener`/`hear`/`room` go through `call()` (catches `EarshotException`
only). `voice.update()` (348), `earlySlots()->early.update()->bridge.early()`
(`EarlyReflections.java:62`), `ownReverbs[s].tick()`, `convolution.tick()`,
`spaces.end()->apply()->requireRoom()` (throws `IllegalStateException`), and `requireRoom` in
`updateEax()` (516) are outside it. Any exception escapes the `executeOnChannels` task: MC logs
"Error executing task" and continues; sends set for some sources only, `spaces.begin()` without
`end()`, `unusedOwn` never released, and it repeats every tick without ever setting `failed`.
**Fix:** wrap the body of `update()` in `try { ... } catch (RuntimeException e)`: log once,
`failed = true`, `voice.fail()`, `releaseAll(...)`. Keep `call()` for the typed native errors.
**Test (Java):** a `SoundShaper` built with a stub `NativeBridge` whose `early()` throws
`IllegalStateException`: after one `update()`, `failed` is true and a second `update()` is a
no-op.
**Effort:** med. **Independent.** Must land before F2.

### F2 - S1 - One NaN/huge source position turns shaping off for the rest of the context
`SoundShaper.java:343-348` (targets), `VoicePlayer.java:277-279` (`heard.eye()`,
`stream.heardFrom()`), `EarlyReflections.java:62`.
**Trigger:** positions are passed raw to `bridge.hear`/`bridge.early`; the native side returns
`INVALID_ARGUMENT` for any point failing `Vec3::is_usable` (finite, |coord| <= 1e9) ->
`EarshotException` -> `failed = true`, `voice.fail()`. One mod playing a sound at NaN kills
Earshot until the context closes.
**Fix:** add `SourceState.usable(Vec3)` mirroring `is_usable`; skip such targets (release their
filters via the `keep()` path) and skip voice streams with unusable positions.
**Test (Java):** a channel with position `(NaN, 0, 0)` among usable ones: `hear` stub receives
only the usable positions; `failed` stays false.
**Effort:** low. **Must land after F1.**

### F3 - S2 - Leaving CONVOLUTION mode keeps the convolution slots running until context close
`SoundShaper.java:305-314` (`chooseMode`), `:492-500` (`reverb()`),
`ConvolutionReverb.release()`.
**Trigger:** user switches reverb to EAX/OFF, or goes underwater (forces EAX). Sends are
rewritten so nothing feeds it, but the two slots with a loaded IR keep convolving (~10 % of a
core per project docs).
**Fix:** remember `previousMode` in `update()`; at end of update when
`previousMode == CONVOLUTION && mode != CONVOLUTION` (after all sends were rewritten), call
`convolution.release()`. Optionally delete the EAX effect/slot when leaving EAX.
**Test (Java):** after two updates (CONVOLUTION then EAX), the convolution slots are deleted.
**Effort:** low. **Independent** (cleaner after F9).

### F4 - S2 - EFX effect leaked every tick when slot generation fails
`SoundShaper.java:506-526` (`updateEax`), `EarlyReflections.java:96-107` (`make`),
`ConvolutionReverb.java:200-216` (`make`), `SpaceReverb.java:173-187` (`claim`).
**Trigger:** `alGenAuxiliaryEffectSlots` returns 0 (device slot cap). `updateEax` regenerates
and leaks the effect every tick; `EarlyReflections.free()` skips the effect when
`slots[i]==0`; `SpaceReverb.claim` stores a 0 slot and uses it.
**Fix:** one shape everywhere: gen effect, gen slot, if slot==0 -> delete effect, log once,
mark that feature off (flag reset in `contextClosing`); `free()` deletes the effect whenever
`effect != 0`. `SpaceReverb.claim` returns null and `slotFor` falls back to no send.
**Test (Java):** stub AL whose `alGenAuxiliaryEffectSlots` returns 0: after N updates the
effect gen/delete counts balance.
**Effort:** med. **Independent.** Land after F1.

### F5 - S2 - Tally double-counts every bin where a ray leg ends
`crates/earshot-sim/src/reverb.rs:223-234` (`Tally::add`).
**Trigger:** the occupancy loop is `first_bin..=last_bin` inclusive and the next leg starts at
the same bin, so energy is counted twice at every bounce bin; worst in small rooms (legs <
0.343 m) and at bin 0 when the listener is against a wall, dragging the -1/-10 dB points
earlier -> `early_decay` biased low.
**Fix:** half-open `first_bin..last_bin` per leg plus counting `last_bin` only on the final leg
(or weight each end bin by the fraction occupied). Keep `Tally` boxed.
**Test (Rust):** tighten the uniform-box early/late tolerance (target <= 0.2); add a
listener-against-the-wall case. Run `tests/scenes.rs` + `bras.rs`; **goldens will move** -
record before/after Room numbers in the commit body.
**Effort:** med. **Independent.** Must land before F11 (test tightening) - already done as a
separate, reverb-behaviour-neutral test.

### F6 - S2 - Room measurement depends on CPU count (cross-machine non-determinism)
`crates/earshot-sim/src/reverb.rs:102-131` (`measure_with_early`, `measure_threads`).
**Trigger:** `threads = available_parallelism().min(4)` when rays >= 2048; `chunk = rays/threads`;
chunk `i` is seeded `seed + i`. A 2-core and a 4-core machine split rays differently ->
different Room.
**Fix:** always split into a fixed `PARTS` (4) when rays >= 2048, seed by part index, vary only
the concurrency.
**Test (Rust):** a `measure_with_early_on(threads: usize)` seam asserting `threads=1` and
`threads=4` produce byte-identical `Room`. Goldens may move once (document).
**Effort:** low. **Must land after F5.**

### F7 - S2 - Unbounded `reach` lets a path ray walk ~2^31 voxels
`crates/earshot-ffi/src/lib.rs:703` (`earshot_trace_paths`),
`crates/earshot-world/src/trace.rs:43-105, 203-315`.
**Trigger:** `reach` is only checked `is_finite() && > 0`. With `1e300` the DDA walks until
`block[axis] += step` wraps.
**Fix:** cap in the extern at `MAX_REACH_METRES` (4096) else `INVALID_ARGUMENT`; clamp in
`walk`/`walk_solid`.
**Test (ffi):** `reach = 1e300` -> `INVALID_ARGUMENT`; `4096.0` accepted.
**Effort:** low. **Independent.** DONE.

### F8 - S2 - Walls are solid but packed as thin sheets; open barrels become holes
`MaterialMap.java:215-226` (`fillCode`), `:149-151` (`classify` OPEN check), `:164-165`.
**Trigger (a):** `WallBlock` is a solid panel in `classify` but `fillCode` uses the outline
volume (~0.25-0.4 -> fill 4-6/16). **(b):** `hasProperty(OPEN) && OPEN -> AIR` applies to any
block with `OPEN`, including `BarrelBlock`.
**Fix:** add `WallBlock` to the full-fill list; restrict OPEN->AIR to
`DoorBlock`/`TrapDoorBlock`/`FenceGateBlock`.
**Test (`MaterialMapTest`):** wall packs full; open barrel is wood; open door is air.
**Effort:** low. **Independent.** DONE.

### F9 - S3 - ConvolutionReverb dead code and misleading log after 94234e9
`ConvolutionReverb.java:85-86` (`failures`, `TRIES` never incremented), `:141-142`
(`maker != null` dead), `:273-274` (warn prints `failed ? "...is off" : "trying again"` where
`failed` is always false), `:164-198` (`release()` calls `contextClosing()`, which
`shutdownNow()`s and respawns the "earshot-ir" executor on every own-reverb release).
**Fix:** delete `failures`/`TRIES`; make the warn unconditional; split `contextClosing()` into
`forgetNames()` (AL names + generation bump, used by `release()`) and the executor reset.
**Test (Java):** releasing a reverb does not replace its loader.
**Effort:** low. **Independent.**

### F10 - S3 - VoicePlayer dead branch
`VoicePlayer.java:258`: `if (failed) return List.of();` after `start()` is unreachable. Remove;
comment on `start()` why it never latches. **Effort:** low. **Independent.** DONE.

### F11 - S3 - Edge-source taps are written without the finiteness filter the reflections get
`crates/earshot-sim/src/ir.rs:177-189` vs `merge` at `:268`.
**Trigger:** a NaN delay casts to sample 0; NaN energy produces a NaN IR the `MAX_ENERGY` cap
does not catch and is uploaded to AL.
**Fix:** filter edges with `energy > 0 && delay.is_finite() && delay >= 0`; guard the finished
IR.
**Test (Rust):** a NaN-delay/NaN-energy `EdgeSource` leaves the IR finite and unchanged.
**Effort:** low. **Independent.** DONE.

### F12 - S3 - Duplicated comment block in `early.rs`
`crates/earshot-sim/src/early.rs:~221-227`: the "Drop edge sources that fuse with the direct
sound" block appears twice verbatim. Remove one. **Effort:** trivial. **Independent.** DONE.

### F13 - S3 - `SoundShaper.contextClosing` does not clear per-source ownership maps
`SoundShaper.java:238-251`: `ownOwners`, `earlyOwners`, `unusedOwn` keep old source ids.
**Fix:** zero the three arrays in `contextClosing`.
**Effort:** low. **Independent.** DONE.

### F14 - S3 - `VoicePlayer.place` passes an unchecked yakvc `range` to AL
`VoicePlayer.java:333-343`: `AL_MAX_DISTANCE = max(range, 5)`; NaN range -> `AL_INVALID_VALUE`
into the shared error state.
**Fix:** `Float.isFinite(range) ? ... : DEFAULT`. **Effort:** trivial. **Independent.** DONE.

## 3. Test gaps (secondary)

- **T1** `fit_early_decay` one-bin cliff (finite, >= `HIST_DT`). DONE.
- **T2** Reverb orphan cap + energy conservation invariant. *After F5.*
- **T3** `merge` ordering invariant. DONE.
- **T4** `NativeBridge.checkNativeLength` negative / over-bound / over-`Integer.MAX_VALUE`.
  DONE (note: it throws `IllegalStateException`, not `EarshotException`).
- **T5** `Propagation` hold/due/cancel/clear. DONE.

## 4. Structure (behaviour-preserving, last)

- **R1** Extract an `Efx` helper class (gen effect+slot with failure handling,
  detach-then-delete, error-window drain) used by `SoundShaper`, `EarlyReflections`,
  `SpaceReverb`, `ConvolutionReverb`, `VoicePlayer`. Proof: F4's balance test + gametest.
  *After F1, F3, F4.*
- **R2** Split `SoundShaper.update()` into `collectTargets`, `applyListener`, `shapeTargets`,
  `finishSlots`. Proof: F1/F2 tests + gametest. *After F1, F2.*
- **R3** Move the four `point_arg`/`is_usable` loops in `lib.rs` behind one
  `points_arg(ptr, count)`. Proof: existing ffi validation tests. *Independent; after F7.*

## 5. Execution order

**Wave A (parallel, independent, low/med):** F7, F8, F10, F11, F12, F13, F14, T1, T3, T4, T5.
DONE.

**Wave B (serial inside Java audio):** F1 -> F2 -> F4 -> F3 -> F9.

**Wave C (serial inside reverb.rs, one golden move each):** F5 -> F6 -> T2.

**Wave D (after B and C green):** R1 -> R2 (same file family), R3 in parallel.

Each commit: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D
warnings`, `cargo test --locked --workspace`, `cd mod && ./gradlew :test`. No ABI changes
anywhere; `include/earshot.h` stays at 15.

## 6. Rejected findings / not bugs

- FFI panic containment: all 23 externs and the 4 voice callbacks are inside `guard`/`guard_value`;
  `panicked` takes the payload by value.
- Boundary validation: `point_arg`, `slice_arg`/`array_arg`, `checked_mul` on counts,
  `checkNativeLength` before every `(int)` cast (F7 is the one gap).
- OpenAL lifetime in `releaseAll`/`releaseFilters`/`VoicePlayer.stop`/`SpaceReverb.free`:
  detach-then-delete order correct.
- Failure latches: all reset at `contextClosing`/`switchTo`, none mid-session.
- `ConvolutionReverb.load` error window: drains and brackets its own calls. Correct.
- `voice.rs` ring: torn reads are a click, never UB.
- `engine.rs` `world_change_soon` queueing: FIFO preserved.
- Water comb (1 ms grid): fixed in 898520f.
- `SpaceReverb.glide` and `end` both bind effect to slot: duplicate, harmless.
- `MaterialMap.packed()` lazy grow unsynchronised: client-thread only today; benign if raced.
- `trace.rs` `floor() as i32` saturation: guarded by `is_usable` at the FFI edge.
- Already-investigated list honoured: pathing rim math, 4.096 s histogram horizon, Tally boxing,
  coordinate guards, OptionInstance usage, Maekawa coverage, TODO scan, per-band knee,
  permanent latches, wall classification (except the fill nibble, F8), panic payload.
