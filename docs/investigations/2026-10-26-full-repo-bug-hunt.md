# Full-repo bug hunt and fix pass — 2026-10-26

Date: 2026-10-26. Scope: the entire repository (Rust workspace + Java Fabric mod + docs +
build tooling), run as three phases — a read-only hunt fan-out, a parallel fix fan-out
partitioned by file ownership, and a verification/review loop. Outcome: **~85 findings,
~70 fixed across 42 files (+1079 / −337), 223 workspace tests green under the exact CI
gate.** Tree left uncommitted on `b76504f` (0.1.0-alpha.59), per the milestone workflow.

---

## 1. Method

### Phase 1 — hunt (13 read-only agents, run in parallel)

| # | Area | Files |
|---|------|-------|
| 1 | sim IR/reverb | ir.rs, reverb.rs, tests/reverb_probe.rs |
| 2 | sim ground/early/EAX | early.rs, ground.rs, eax.rs (+ design docs) |
| 3 | sim engine/hear | engine.rs, hear.rs, quality.rs, rng.rs, lib.rs |
| 4 | sim pathing/paths (first attempt rejected, retried) | pathing.rs, paths.rs, tests/scenes.rs, tests/bras.rs, examples/trace_bench.rs |
| 5 | world crate | trace.rs, world.rs, material.rs, vec3.rs |
| 6 | FFI crate | earshot-ffi lib.rs, voice.rs, guard.rs, cbindgen |
| 7 | Java audio | audio/* (SoundShaper, VoicePlayer, ConvolutionReverb, …) |
| 8 | Java natives/OpenAL | natives/*, openal/* (cross-checked against Rust FFI field-by-field) |
| 9 | Java world/config | world/*, EarshotConfig (+ JSON resources, Rust material table) |
| 10 | Java UI/mixin/client | ui/*, command/*, mixin/*, mixins.json |
| 11 | tests/bench/tooling | gametest, unit tests, bench, xtask, build.gradle, scripts |
| 12 | cross-language constants | every quantity duplicated between Rust and Java |
| 13 | docs vs implementation | docs/* vs code; PLAN.md open-items inventory |

Instructions to every hunter: read-only, exclude `.claude/worktrees/` (stale copy), report
only defensible defects with file:line + evidence + severity + confidence + one-line fix,
and distrust the repo's known bug-hiding patterns (tests re-deriving the formula under
test, ratio-only assertions masking absolute-scale errors).

### Phase 2 — triage and partition (parallelizable by file ownership)

Findings were deduplicated (~85 unique after merging cross-agent overlaps) and grouped so
**no two fix agents owned the same file**. Eleven clusters were dispatched in two waves.
Two deliberate cross-agent contracts were required because the largest bug spanned the
language boundary (see §3).

### Phase 3 — verification and review

- Rust: exact CI gate (`fmt --check`, `clippy --workspace --all-targets --locked -- -D
  warnings`, `xtask header --check`, `test --workspace`).
- Java: **no JDK on this machine** (the `/usr/bin/java`/`javac` shims both report "Unable
  to locate a Java Runtime"), so Java's gate was two adversarial read-only review passes
  over every diff, plus bytecode disassembly of `OptionInstance` from the Minecraft 26.3
  loom jar to settle a comment dispute, and `bash -n` + stub-`gh` simulation for the shell
  script. The real Java compile/test happens in CI.
- Every fix agent was told to re-verify each finding before editing and to report (not
  fix) anything that didn't hold. Two findings were downgraded/skipped on re-verification;
  one (`check()` NUL-tail) was fixed on the Rust side instead of Java; one (aperture
  tie-break) initially fell through the partitioning and was caught during compilation of
  this document, then fixed with failing-test-first evidence.

---

## 2. Findings ledger

Severity as judged at hunt time. All statuses are final.

### A. earshot-sim — hear / pathing / ground / engine

| ID | Location | Finding | Sev | Status |
|----|----------|---------|-----|--------|
| A1 | hear.rs:142 | No-detour guard gated on `!round_is_better`, contradicting its own comment: with HJ 2.4 (or a deep ISO mid null) an unobstructed straight line refills every band to 1.0, erasing the ground null and hijacking `heard_from`/`path_length` | **high** | fixed — guard unconditional; regression test `a_route_no_longer_than_the_line_never_fills_a_ground_null_back_in` |
| A2 | hear.rs:169-180 + pathing.rs:134 | `detour()` compared a block-centre-based `route.length` against a true-position straight line — up to ~0.9 blocks of phantom detour from sub-block position alone (every test used a block-centre listener, hiding it) | med-high | fixed — `listener_leg` projects the listener's offset onto the first step; off-centre-listener tests added |
| A3 | hear.rs:186-196 + pathing.rs bend | Enclosed-corner loss charged on *arrival deviation* (~half the path's turn: canonical 90° corner charged 45°) while PLAN:112 says "per right angle turned" | low-med | fixed — bend is now the turn at the opening between (opening−source) and (listener−opening); pin test (1.46 rad vs old 0.67) |
| A4 | ground.rs C0 / hear / engine | Temperature never reached the ground model although temperature-design.md:65-66 and PLAN R3 both claim it — `excess_attenuation_db` had no speed parameter | high (design-mandated) | fixed — `speed` threaded hear → ground_gain → band_gains → k0; engine passes `speed_of_sound(air_celsius())`; design's monotonic-null-shift test added; ground unit tests pin c=343.0 explicitly |
| A5 | hear.rs:120-123 fold | Ground's **amplitude** factor (10^(EA/20), cap 2) folded raw into an **energy** array: overlay printed +3 dB where every comment promises +6 | med | fixed — squared at the fold (energy cap 4); `Heard.gains` doc, bounds test (0..=4), and scenes' open-ground range updated with reasoning |
| A6 | ground.rs:384-392 | Test pin centers stale by up to 0.03 dB vs an independent high-accuracy re-implementation | low | fixed — re-pinned −2.4691 / +3.2089 / ±0.05 window; design doc quotes updated to match |
| A7 | ground.rs:216-217 | Comment claimed the design shows the `-i` form; the design shows `+i`, matching the code | low | fixed — sentence removed |
| A8 | engine.rs:573-589 | Settled room trace never invalidated by a quality change (frozen old rays/reach until listener moves) | low | fixed — quality fingerprint folded into `RoomTrace.last`; regression test verified failing pre-fix |
| A9 | engine.rs:438-452 | Unusable listener stored as `None`, suspending *all* worker maintenance (stale field/room with quality off) | low | fixed — last good listener kept, mirroring `set_air`'s precedent; regression test |
| A10 | pathing.rs aperture() | Axis tie-break for diagonal steps measures the plane *parallel to the wall* — a 1×2 door entered diagonally reports 64 m² instead of 2 (full coupled reverb instead of its share) | med | fixed (late — caught while compiling this doc) — min over all tied planes; two failing-first tests (64.0 → 2.0) |
| A11 | hear.rs WAVELENGTH | Diffraction λ hardcodes 343 like C0 did | low | noted, left — outside the design's mandate; see §6 |

### B. earshot-sim — IR / reverb probe / paths / scene tests

| ID | Location | Finding | Sev | Status |
|----|----------|---------|-----|--------|
| B1 | ir.rs:246-248 | Air absorption energy gain (10^(−dB/10)) applied to amplitude samples → every tap's air loss double in dB | med | fixed — exponent /20; comment states amplitude factor |
| B2 | ir.rs:235-240 | `AIR_DB_PER_METRE = [0.0002, 0.0013, 0.0124]` matches nothing — Java's `Air`/`AirTest`, `reverb.rs`, and PLAN all give ≈[0.0013, 0.0047, 0.0297] at the band centres (middle value was ISO's 250 Hz figure — shifted table) | med | fixed together with B1 (the two errors partially cancelled: 2×12.4 ≈ 28, masking both from ratio tests) |
| B3 | reverb_probe.rs:7-8 | Header stated LEVEL 0.5 / 1.5 s; real game is 1.0 / 3.0 | low | fixed |
| B4 | reverb_probe.rs:67-77 | EAX reference tail's noise carrier had power 1/3 → conv-vs-EAX comparison biased ~4.8 dB | low-med | fixed — scaled to unit power (√3); probe prints +4.77 dB shift as predicted |
| B5 | scenes.rs:262 | IR duration computed as `ir.len()/rate` without ÷4 interleaved channels → length assertion 4× weaker than its comment claims | med | fixed — ÷ IR_CHANNELS |
| B6 | scenes.rs:468-472 | Mass-law tolerance anchored to the *measured* slab, not the expected solid → >100 % upward slack vs the claimed 10 % check | low | fixed — anchored to `solid.sqrt()` |
| B7 | paths.rs:347-368, 470-492 | "Bookkeeping" box tests Bernoulli-drew against rock's transmission every hit (~7 % odds of random failure on any future constant bump) | low | fixed — two-block-thick walls make `leaky` false; RNG stream bit-identical |

### C. earshot-world

| ID | Location | Finding | Sev | Status |
|----|----------|---------|-----|--------|
| C1 | trace.rs:393 vs :415-421 | Empty-section jump tie-broke toward the *lowest* axis while the per-block walk advances the *highest* — jump and walk visit different corner blocks at section boundaries; corner water tolls/flank anchoring could diverge | low | fixed — `t <= leave`; regression test verified failing pre-fix |
| C2 | trace.rs:94/104, 398/436 | Negative `max` produced `exit < enter` → any exponent consumer computes `powf(negative)` > 1 (energy gain) | low | fixed — early return for `max < 0`; `max == 0` semantics pinned in `bad_input_ends_the_walk_at_once` |
| C3 | world.rs:127-131 | `set_block_fill` cannot represent fill 0 (code 0 = full); NaN/−∞ silently stored as 1/16 | low | fixed — refuses non-finite/≤0 with `false`, documented; round-trip test |
| C4 | material.rs:261 | Test comment says "18 dB" while pinning the 35 dB constant (written in the commit that *raised* it) | low | fixed |

### D. earshot-ffi

| ID | Location | Finding | Sev | Status |
|----|----------|---------|-----|--------|
| D1 | voice.rs stream map | Unbounded, never-freed map keyed by network-influenced UUIDs; past Java's `MAX_VOICE_STREAMS` (4096) the listing API broke *forever* while native heap grew | med | fixed — `MAX_STREAMS = 4096` cap, new UUIDs refused (known ones never freed, per the OpenAL-pulling invariant); test uses `stream_capped(_, 0)` seam |
| D2 | voice.rs:393 | `silence()` ran *outside* the panic barrier — misaligned OpenAL buffer panics in debug → unwind through `extern "C"` → abort on the mixer thread (the exact failure the barrier exists to prevent) | low | fixed — moved inside `guard_value` |
| D3 | voice.rs:284-285 | `pull()` double-read `queue.len()`; `stream_end` racing between reads → usize underflow → debug panic / release wrap draining post-cut frames | low | fixed — single load + `saturating_sub` |
| D4 | voice.rs Ring::pop | Reader snapshotted start twice; two back-to-back max pushes could overtake it (atomics → no UB, transient garble) | low | fixed — one consistent written/start pair; doc states the real guarantee |
| D5 | lib.rs (6 fns) | `# Safety` sections omitted the non-overlap requirement where `&` and `&mut` alias caller memory (`earshot_early` primary) | low | fixed — contract clauses added (`earshot_hear` already collects to a Vec) |
| D6 | lib.rs:1005-1014 | `earshot_last_error` returned `message.len()` but wrote only a char-boundary prefix → Java appended U+0000 tails to >1 KiB messages | low | fixed — returns bytes actually written; Java's `min()` unchanged |
| D7 | include/earshot.h | Regenerated after doc-comment edits; `xtask header --check` green | — | done |

### E. Java — audio

| ID | Location | Finding | Sev | Status |
|----|----------|---------|-----|--------|
| E1 | SoundShaper `filter()` (+ space filter) | Energy shares fed raw to OpenAL **amplitude** gains — audible loss = 2× documented dB everywhere (rock −90 vs −45); `send()` sqrt'd the same array, so the two sinks disagreed | **high** | fixed — sqrt at both sinks; combined response = √E/lift verified |
| E2 | SoundShaper:913 portalShare | Power ratio used as amplitude gain — documented door −9 dB played as −18 dB | med | fixed — `sqrt(aperture/16)`, floor kept audible-identical; new test pins −9.03 dB |
| E3 | SoundShaper lift() | Rebuilt as amplitude crest `min(max(1, max √E), 2)` on the energy contract; AL_GAIN rides it, filter divides it out | — | fixed |
| E4-E6 | SoundShaper | Three same-fault-line sites: UNDERWATER factors, the distance `fade`, and the `WALLED` threshold all multiplied/compared against gains pre-conversion — each squared at its site so audible behavior is preserved exactly | med/low | fixed |
| E7 | SoundShaper:773 javadoc | "last quarter" vs `FADE_SHARE = 0.15` (constant retuned in 0b3597d) | low | fixed |
| E8 | SoundShaper chooseMode javadocs | "best the game's OpenAL has" vs actual Auto→EAX since alpha.23 | low | fixed |
| E9 | SoundShaper:178 | Own-reverb IR still passed `1.5f` after MAX_SECONDS retuned to 3.0 (the exact flat-tail bug the 2026-10-18 investigation fixed for the listener) | med | fixed — passes `ConvolutionReverb.MAX_SECONDS` |
| E10 | VoicePlayer cone javadoc | Read OpenAL full-cone angles as half-angles ("full within 90°" — actually 45°; "past 270°" unreachable) | low | fixed — javadoc rewritten (45°/135°, −4/−12 dB verified); constants stay 90/270 per PLAN:210 |
| E11 | VoicePlayer reverbOnly latch | Own-voice latch one-way: a SELF stream becoming positioned stayed a wet-only unshaped ghost for life | low | fixed — `unOwnVoice` on transition via `SoundShaper.releaseFilters`; re-reviewed clean |
| E12 | VoicePlayer reverbOnly | Regenerated AL filters without deleting (leak on re-latch) and never set room rolloff on its send filter | low | fixed — create-if-0 + explicit gains; delete/recreate on un-own restores rolloff |
| E13 | ConvolutionReverb:315 | Stale-generation load returning empty IR wiped the *current* generation's `madeFrom` → spurious regenerate+crossfade after context switch | low | fixed — generation-guarded restructure; all paths traced (loading can't stick) |

### F. Java — natives / OpenAL

| ID | Location | Finding | Sev | Status |
|----|----------|---------|-----|--------|
| F1 | HrtfFiles:159 | Zip entry selector matched `…/SADIE II KU100.sofa`; real entries are `D1_HRIR_SOFA/D1_48K_24bit_256tap_FIR_SOFA.sofa` — **the SOFA download could never succeed**, failing as a misleading "SHA mismatch" on an empty file (verified against live Zenodo content; pinned SHAs are the 48K entries) | **high** | fixed — exact entry stored per record; target created only after match; clear "no <entry> in <url>" error |
| F2 | NativeBridge MAX_IR_FLOATS | `1<<22` below the native contract's own ceiling (10 s × 192 kHz × 4 = 7,680,000) — blessed inputs would throw; javadoc cited stale 1.5 s sizing | med | fixed — bound raised, javadoc rewritten; test references symbolically |
| F3 | NativeBridge hear/early/tracePaths | `int` length guards wrap at huge counts → native builds GB slices over the JVM heap (consumer SoundShaper already cast long) | low | fixed — long casts + negative rejection |
| F4 | NativeBridge:474 NUL-tail | See D6 — fixed Rust-side (Java's `min()` compatible) | low | fixed |
| F5 | ContextAttributes:14 | "two … for the spaces" — reality is 2 listener + 1 space + 1 early | low | fixed |

### G. Java — UI / client / world mirror

| ID | Location | Finding | Sev | Status |
|----|----------|---------|-----|--------|
| G1 | AcousticsScreen:100 + HrtfTestScreen:46 | Bare SOFA names never entered the dropdowns on a fresh install → `onHrtfPicked`'s download branch unreachable (F1's sibling) | med | fixed — `HrtfFiles.NAMES` added to both lists |
| G2 | AcousticsScreen removed() | Screen close mid-download cancelled *and persisted* the undownloaded pick; same-value `set` never re-fired → no retry, ever | med | fixed — saves `previousHrtf` while cancelling; mechanism comments now bytecode-verified (MC 26.3 `OptionInstance` has one `ValueUpdateListener`, fires only on change) |
| G3 | AcousticsScreen updateActive | `diffraction` never greyed with Quality Off though the class javadoc promises all dead settings are | low | fixed |
| G4 | AcousticsScreen:275 | Comments promised a progress bar that didn't exist; `rebuildWidgets()` calls inert | low | fixed — false comments/inert calls removed (real progress UI would need a new lang key; noted) |
| G5 | DebugOverlay:68 | Background 1 px short at the bottom (exclusive y2) | low | fixed — true ±1 px padding, comment states exclusivity |
| G6 | DebugOverlay:136 + EarshotCommands:139 | Gains ≥ 1 (ground +6 dB) displayed as "0 dB" | low | fixed — always print real dB, −0 normalized |
| G7 | EarshotClient:164 | Ultra's room reach initialized to hardcoded 64; `followDimension`'s early-return meant `use()` never corrected it in the overworld — whole session at 64 instead of 96 | med | fixed — `config.quality(roomReach())` with null-guard for init-time |
| G8 | WorldMirror:143 | `pending.remove` before the null-fetch check → chunk in neither set, never mirrored again (permanent occlusion hole) | low | fixed — key re-added on null; set can't grow |
| G9 | WorldMirror:33 | `RADIUS = 8` (128 blocks) < Ultra's 192-block room reach → room rays traced phantom air past the mirror | low | fixed — RADIUS 12 = 192 |

### H. Tests / build tooling

| ID | Location | Finding | Sev | Status |
|----|----------|---------|-----|--------|
| H1 | ContextAttributesTest | Tautology comparing three literals; never touched `MHR_SUFFIX` though its javadoc claims it pins drift | med | fixed — asserts the real constant + bundled resource naming |
| H2 | xtask natives/dist | `natives()` wiped only the staged platform dir but `write_manifest` hashed *all* — stale sibling dylibs re-hashed and shipped (observed in-tree: `macos-universal` from Oct 16 beside Oct 19's `macos-aarch64`, both in the manifest and jar); `dist --from` called `stage()` per file (fresh each time → only last file would survive) | med | fixed — wipe `out` at start (CI verified single-invocation), per-platform copy once in `dist --from`; validated end-to-end against a pre-seeded stale throwaway out dir |
| H3 | EarshotClientGameTest:215 | Assertion "a speaker not in the world can't face a way" checked a speaker already *provably* absent | med | fixed — moved inside the live window |
| H4 | OpenAlBench:183 | "IR swap" timing included a full render block; `alGetError` attributed render errors to the swap | low | fixed — per-block baseline subtracted (units verified: `elapsed` is seconds), error drain moved; format unchanged |
| H5 | EarshotConfigTest | `quality()`'s `!occlusion` branch had zero coverage; `off` assertions re-asserted preset literals | low | fixed — occluded/noOcclusion pair on the same preset |
| H6 | build.gradle + HrtfFilesTest | `git lfs pull` on *every* test/build (no inputs, never up-to-date) hard-failed offline machines; missing fixture = silent pass | low-med | fixed — fixture inputs + `onlyIf` presence check (config-cache-safe, threshold separates pointer/real); missing fixture now `fail()`s with the LFS hint |
| H7 | ci-wait.sh:23, :38 | Under `set -euo pipefail` a transient `gh` failure aborted with no output, breaking the documented RESULT contract (poll loop guarded, these didn't) | low | fixed — both guarded; three paths simulated with stub `gh` |
| H8 | EarshotClientGameTest:348 | `awaitSound` re-found the snapshot after `waitFor` → bare `NoSuchElementException` on a stop race | low | fixed — capture from the successful poll |
| H9 | EarshotClientGameTest:381 | `expectTransmission` caught only `RuntimeException`; siblings catch `AssertionError` too → diagnostic lost on timeout | low | fixed |

### I. Docs / i18n (~26 edits across 6 files)

GUIDE.md: falloff row (On by default, past *three* blocks); CPU budget "six tenths" → 1.2
cores (paragraph made self-consistent); materials list adds `rug`; carpets removed from
"stay open" (they classify as the solid rug material); HRTF filenames → `… 64-tap.mhr` +
SOFA first-selection note; reverb Auto row → EAX/convolution-on-choice; new Diffraction
settings row; intro reverb bullet rewritten (was convolution-by-default).

en_us.json: auto tooltip → EAX-first; Ultra tooltip → "two loudest get their own reverb,
the next four early echoes" (was "four loudest").

PLAN.md: falloff bullet → on/3 blocks/15 %; resolved-question 5 → 15 %; non-goal §9 marked
superseded (alpha.19, 0b3597d); M2 OpenAL bullet → amplitude gain = √(low share); perf
table 1.5 s → 3.0 s IR + Auto=EAX wording + footnote that only the room-trace interval is
a real timer (occlusion is per tick, field is event-driven); aux-sends rationale → 2+1+1;
stone worked example −23 dB → current table's −35/−45/−55 dB per block; R0 crossfade line
pointer 258 → crossfade() at ~380.

ground-reflection-design.md: Goals' "deeper the lower the band" corrected (deepest of the
three nulls lands in the high band at the pinned geometry, as the code asserts); pin quotes
→ −2.47/+3.21/−3.82; fold section clarified as pressure factor squared into energy.

flanking-leakage-design.md: formula gains the `.max(mass-law)` guard the code has; the
"always ≥ transmission (a test enforces it)" claim carved out for water (the test exempts
water); stale line refs (material.rs, early.rs, trace.rs) re-verified and updated.

temperature-design.md: ground bullet past tense (now implemented + tested); new scope note
that thunder/propagation delays and OpenAL Doppler deliberately stay at a fixed 343 m/s.

### J. Findings produced by the review phase (post-fix, all fixed)

1. **high** — SoundShaper `filter()`'s pre-sqrt clamp at energy **1** canceled the lift
   mechanism entirely and discarded the ground boost (gains {4,·,·} played as free field;
   regression vs HEAD; contradicted its own comment). Fixed to clamp at the contract's
   [0,4]; `ampLow/lift ≤ 1` proven by construction; `send()`'s clamp-at-1 left alone (no
   lift there, EFX filter gain caps at 1 anyway).
2. low — `lift()` sqrt unclamped (NaN could ride `Math.max(1f, NaN)` into AL_GAIN) → clamped.
3. low — "WALLed" comment typo.
4. low — `earlySlots` ranking javadoc now ambiguous (it ranks by mid-band energy per unit
   distance; expression left untouched since HEAD's mixed values were ≈energy too).
5. low — filter comment overclaimed the response identity (GAINHF ≤ 1 caveat added).
6. low — DebugOverlay fill comment vs exclusivity (fixed to true ±1 px).
7. low — contradictory `OptionInstance.set()` comments (settled by bytecode: fires only on
   change; both comments rewritten to the confirmed mechanism).
8. low — `progress.failed()` uneven across HrtfFiles failure paths → single choke point in
   `ensure()`'s catch, explicit calls removed, all six paths traced to exactly-once.
9. low — NativeBridge javadoc attributed rate/second ranges to earshot.h → lib.rs.
10. low — VoicePlayer `unOwnVoice` javadoc overstated "silence" (pre-fix state was a
    wet-only ghost, not silence) — fixed directly.

---

## 3. Cross-agent contracts

Two findings were coupled across languages/files and could not be owned by one agent:

1. **Energy/amplitude contract.** Agent owning hear.rs/ground.rs squares the ground factor
   at the fold (`gains` become pure energy in [0, 4] — ground pressure ×2 = energy ×4 =
   +6 dB in both dB readings, making the overlay's "+6 dB" literally true); the agent
   owning SoundShaper.java converts at every amplitude sink (`√`), reworks `lift()` to the
   amplitude crest, and squares the three pre-existing mixed sites (UNDERWATER, fade,
   WALLED). Review confirmed each side of the contract; the one bug it introduced (J1) was
   caught by the second-pass review.
2. **Ground pin values.** The independent re-computation (−2.4691 / +3.2089 / −3.8190) was
   given verbatim to both the ground.rs test-repin agent and the design-doc agent so code
   pins and doc quotes match.

## 4. Verification results

Final state, after the aperture fix:

| Gate | Command | Result |
|------|---------|--------|
| Format | `cargo fmt --all --check` | clean |
| Lints (CI-exact) | `cargo clippy --workspace --all-targets --locked -- -D warnings` | clean (one violation found and fixed during the pass: `too_many_arguments` on the new `hear_with_speed`) |
| Tests | `cargo test --workspace --locked` | **223 passed, 0 failed** (31 ffi + 143 sim lib + 10 scenes + 1 bras + 1 reverb_probe + 37 world) |
| Header | `cargo xtask header --check` | clean (regenerated after FFI doc edits; no signature/ABI change — still 16) |
| JSON | `json.load(en_us.json)` | valid |
| Shell | `bash -n` + stub-`gh` simulation of all 3 paths | exact RESULT lines, correct exits |

Test-integrity note: several new regression tests were **verified to fail against the
pre-fix code** before the fix was applied (guard, detour origin, bend, quality-unsettle,
listener-kept, DDA tie, aperture×2), and the one bound that legitimately moved (scenes'
open-ground range under the ×4 energy fold) was recomputed from physics with the same
±10 % width — no bound was loosened to make a regression pass.

Java gate: two full adversarial diff reviews + targeted re-review of VoicePlayer; every
issue found in review was fixed (§2J). Compilation happens in CI (no JDK on this host).

## 5. Findings deliberately left open

- **Pre-existing, flagged not introduced by this pass:** on non-EFX devices the ground
  boost rides `AL_GAIN` with no filter to divide it out (boost applies unfiltered to all
  bands) — unchanged behavior, candidates for a follow-up.
- **Hear.rs `WAVELENGTH`** still hardcodes 343 for diffraction λ (the temperature design
  mandated only the ground wavenumber; noted as the same class of issue).
- **PLAN.md's known-issue list, untouched by design:** copper grates occlude as full
  blocks; junction flanking not implemented; reverb send room rolloff untuned by ear; the
  M4 two-client listening session; R0 Candidate B contingency; R2 SOFA personalization.
- **Outside this repo:** yakvc-side caveats from the voice-sink docs (whether upstream
  skips its own acoustics before the sink) cannot be verified here.

## 6. Files changed (42)

- **Rust (15):** earshot-sim — hear.rs, ground.rs, pathing.rs, engine.rs, ir.rs, paths.rs,
  lib.rs, tests/scenes.rs, tests/reverb_probe.rs; earshot-world — trace.rs, world.rs,
  material.rs; earshot-ffi — lib.rs, voice.rs, include/earshot.h; xtask/src/main.rs.
- **Java (21):** main — SoundShaper, VoicePlayer, ConvolutionReverb, NativeBridge,
  HrtfFiles, ContextAttributes, AcousticsScreen, HrtfTestScreen, DebugOverlay,
  EarshotCommands, EarshotClient, WorldMirror; tests — SoundShaperTest,
  ContextAttributesTest, HrtfFilesTest, EarshotConfigTest; gametest —
  EarshotClientGameTest; bench — OpenAlBench; resources — en_us.json; build —
  mod/build.gradle.
- **Docs/scripts (6):** GUIDE.md, PLAN.md, ground-reflection-design.md,
  flanking-leakage-design.md, temperature-design.md, scripts/ci-wait.sh.
