# Convolution comb artifact ("comb effect with high freqs") — investigation

Scope: root cause only, from code. No fix applied. Earshot commit `f36f696` + uncommitted
bug/test-pass edits.

## 0. Where Earshot's convolution actually lives

Earshot does **not** implement convolution. It synthesizes a B-format impulse response in Rust
and hands it to **OpenAL Soft's in-progress convolution effect** (`AL_EFFECT_CONVOLUTION_SOFT`,
`0xA000`), which does the DSP inside the game's mixer.

- IR synthesis: `crates/earshot-sim/src/ir.rs:54` `impulse_response(...)`, exposed as
  `crates/earshot-ffi/src/lib.rs:605` `earshot_impulse_response`.
- Upload + effect plumbing: `mod/src/main/java/io/github/cumsauce/earshot/audio/ConvolutionReverb.java:220`
  `AL10.alBufferData(buffer, AL_FORMAT_BFORMAT3D_FLOAT32, ir, SAMPLE_RATE)` then
  `:222` buffer → slot, `:224` effect → slot.
- Sends: `mod/src/main/java/io/github/cumsauce/earshot/audio/SoundShaper.java:632` `send(...)`,
  `AL11.alSource3i(source, AL_EFFECTSLOT_...)` at `:644`.

So the comb must come from either (a) the synthesized IR, or (b) how Earshot drives the effect.
Both were read in full. Findings below.

## 1. The IR synthesis: two mechanisms that *can* make a comb

### 1a. CONFIRMED DEFECT — the two-slope tail's knee is applied independently per band, per channel

`ir.rs:86-108`:

```rust
let early_t60 = room.early_decay.map(|t60| t60.max(0.05));
let late_t60  = room.decay.map(|t60| t60.max(0.05));
let knee: [f32; 3] = std::array::from_fn(|band| early_t60[band] / 6.0);
...
let decays: [f32; 3] = std::array::from_fn(|band| {
    if t <= knee[band] {
        (-6.907_755 * t / early_t60[band]).exp()
    } else {
        (-6.907_755 * knee[band] / early_t60[band]).exp()
            * (-6.907_755 * (t - knee[band]) / late_t60[band]).exp()
    }
});
out[i * IR_CHANNELS + channel] =
    level * weight * (low * decays[0] + mid * decays[1] + high * decays[2]);
```

`decays[band]` multiplies a **low-passed copy of the same noise** (`bands.split(noise)`,
`ir.rs:100`). The three bands are summed back together (`:110`), so each band's slope change is a
**multiplicative gain step applied to a different frequency slice of one signal** — the classic
shape of a time-varying comb.

The knee times differ per band only through `early_decay`, but the step's *magnitude* is set by the
ratio of the slopes. When `early_decay < decay` (a coupled room, the case the feature was built
for), the band's envelope breaks **downward** at `t = knee[band]`, and the size of the discontinuity
in the *derivative* is proportional to `(1/early − 1/late)`. For high frequencies this is routinely
large. The high band's knee is at `early_t60[2]/6`; at High's 1.5 s IR that is typically 100–300 ms,
i.e. tens of milliseconds after onset — inside the region the ear reads as timbre, not as a tail.

This is not a hypothesis about audibility I can prove from code alone; what is proven from code is
that the synthesis introduces a **per-band gain-step discontinuity at three different times in the
same broadband signal**, and that this is the only structural candidate in the IR that produces
metallic high-frequency coloration.

### 1b. CONFIRMED DEFECT (same code path) — `early_decay` is fitted per band from a noisy EDC, so the knee is inconsistent between bands

`reverb.rs` `fit_early_decay` least-squares-fits the first −1..−10 dB of the raw occupancy curve
per band. The heavy analysis in this session already established the fit can return
`early_decay > decay` (noise in the fit). Combined with 1a: the three knee times and the three step
directions are **independently fitted**, so in a real room the low band can step up while the high
band steps down at different times. That is worse than a single step — it is a phase/spectral
redistribution that reads exactly as "comb with high freqs."

Existing tests do not pin this: `ir.rs` tests cover `early < decay` and `early == decay` only, and
the new test added this session (`an_early_decay_slower_than_the_rooms_still_falls_over_the_knee`)
pins only finiteness and non-increasing windowed energy — which a comb satisfies.

### 1c. NOT a defect, ruled out

- **Tail added twice / dry summed.** No dry+wet mixing inside Earshot's own code. The wet path is
  an aux send (`SoundShaper.java:644`, `AL_AUXILIARY_SEND_FILTER`), so OpenAL mixes dry and wet.
  The wet path has the IR's own bulk delay; there is no equal-latency second copy. Not a comb
  multiplier of the kind the user describes.
- **No crossfade / overlap-add.** Not Earshot's code: OpenAL Soft does segmented overlap-add
  internally (`alc/effects/convolution.cpp`, 128-sample segments / 256-sample FFT, double-precision
  forward FFT, real time-domain FIR for segment 0). That is a correct overlap-add. No boundary bug
  reachable from Earshot's side.
- **Stale/unzeroed tail buffer in Earshot.** None exists — Earshot holds no tail.
- **IR sample-rate mismatch.** `ConvolutionReverb.java:32` `SAMPLE_RATE = 48_000`, uploaded at
  `:220`; OpenAL Soft resamples with a polyphase resampler when the device rate differs
  (`convolution.cpp`, `PPhaseResampler resampler; if(device->mSampleRate != buffer->mSampleRate)`).
  Handled upstream. Not the cause.
- **Normalization per block.** Normalization is once at synthesis (`ir.rs:115` `norm`, `:156`
  `MAX_ENERGY` rescale). Not per block. No DC removal is done, but the tail is zero-mean noise and
  the taps are single samples — no DC offset to speak of.
- **Two reverbs at once (listener + own).** Real, but not a comb: `SoundShaper.java:555` makes
  `owned = 2` own reverbs and `:563` sends the loudest two sources to *their own* slot **instead of**
  the listener's (`ownSends[i] != null ? ownSends[i] : sends`, `:402`). Sources 3..6 get early
  reflections over the listener's reverb (`:565-570`). Each source sends to exactly one convolution
  slot per update, so no double-convolution of one source. The crossfade (`ConvolutionReverb.java:258`)
  fades old/new over 5 ticks, so **during a swap one source genuinely feeds two convolvers with
  different IRs**. That is intentional and is the one place a transient comb is expected — it lasts
  ~5 ticks (~250 ms) per room change and would recur as the player moves. Worth flagging, but it is a
  transient, not the steady metallic timbre reported.
- **IR applied twice via early reflections.** `EarlyReflections` (`EarlyReflections.java:18`) is
  `AL_EFFECT_EAXREVERB`, a parametric effect with discrete reflection taps — not a convolution of
  the IR. It does not convolve with the same IR. But note it **is** a second, unfiltered aux send on
  the same source (`SoundShaper.java:610`, `sendEarly`, `EarlyReflections.SEND = 3`), added *on top of*
  the convolution send (`:402`). In echo-dense geometry, EAX early reflections + a convolution whose
  IR already contains those same early reflections = **the same reflections arriving twice at
  slightly different delays**. That is a genuine comb, and it is the second real candidate.

## 2. The two candidate root causes, ranked

**Candidate A (most likely) — per-band knee discontinuity in the synthesized IR** (`ir.rs:86-108`).
One broadband signal, three independently-fitted multiplicative slope changes at three different
times. Metallic/hollow, high-frequency-weighted, present whenever `early_decay != decay`. Matches
"comb effect with high freqs" as a steady timbre.

**Candidate B — early reflections counted twice** (`ir.rs:120-152` builds them into the IR;
`SoundShaper.java:404/610` also sends the source to a separate EAX early-reflections slot with the
same source geometry). Those reflections then arrive once via the convolution and once via EAX at
different delays. This is a literal comb and would be strongest where reflections are discrete and
strong — corridors, small rooms, i.e. exactly where a listener notices metallicity.

Both are reachable from the code; neither is confirmable as *the* cause without a measurement, because
both produce the same perceptual family. They are also distinguishable by experiment (section 4).

## 3. What I could NOT determine

- Which candidate dominates, from code alone. Both are structurally capable of the reported artifact.
- Whether the user's particular room had `early_decay != decay` (needed for A) — the fit is
  runtime-dependent.
- The actual device sample rate on the user's machine (irrelevant per 1c, but it does mean the IR is
  being resampled, which adds its own HF attenuation, not a comb).
- Whether the artifact is steady or transient. This is the single most diagnostic fact and only the
  user can supply it.
- I have no audio capture, no game client, and could not reproduce. **I did not reproduce this.**

## 4. Experiments that would confirm which candidate

Each isolates one path with no code change. Run them in order.

**E1 — Is it the IR or the routing?** Set `/earshot reverb convolution` but move to an *outdoor,
open* area (or a large hangar). Candidate A needs `early_decay != decay` and a loud diffuse tail; in
the open the tail is ~0, so the knee steps carry almost no energy. Candidate B needs discrete
reflections.
- Comb gone outdoors, present indoors → consistent with A.
- Comb present outdoors → B (the EAX early-reflection send is still running).

**E2 — Does the EAX early-reflection send own it?** Temporarily set the early-reflection send to
null for a test by walking to a spot where `earlySlots[...] == 0` (an area where `earlyOn` is false,
or a scene with fewer than `OWN_REVERBS` sources so `chosen == 0`, `SoundShaper.java:565`). If the
comb weakens as fewer sources get early slots, B is implicated.

**E3 — A/B the two-slope feature.** This is the decisive test for A. `Room.early_decay` is the only
input to 1a's knee. If a build is made where `early_decay` is forced equal to `decay` (the feature
collapses to the single exponential it was before alpha.43/#4), then:
- Comb disappears → **Candidate A confirmed**, and the fix is in `ir.rs`'s knee construction.
- Comb unchanged → A is exonerated; pursue B.

I deliberately did **not** apply E3 as a fix, because collapsing the feature silently would remove
work the user asked for (P2 #4).

**E4 — Steady vs transient.** Ask the user: does the comb persist steadily, or does it appear for a
moment after walking through a doorway / turning? Steady → A or B. Momentary and correlated with
movement → the intended `ConvolutionReverb` crossfade (section 1c), which would be a different bug
(crossfade too slow / two live IRs too long).

## 5. Minimal root-cause fixes (NOT APPLIED — awaiting E3/E1 result)

**If A is confirmed.** The knee must not introduce a band-dependent gain step. Two candidates, in
order of preference:
1. Move the two-slope envelope from the per-band gains to the **per-band *energy* of a single shared
   envelope**, i.e. apply one broadband slope change (a single `knee`, single step) and keep only the
   band *weights* fixed. That removes the per-band step entirely and keeps the coupled-room decay
   behaviour.
2. Failing that, cross-fade the two exponentials over a few milliseconds around `knee[band]` instead
   of switching at a hard `t <= knee` boundary, so no band's derivative is discontinuous. This is a
   smaller, local change to `ir.rs:101-108`.

Either way the test that proves it is an **impulse/noise through the convolver showing notches at the
expected spacing** — concretely, a `#[test]` in `ir.rs` that (i) synthesizes an IR with
`early_decay != decay`, (ii) takes its magnitude spectrum of the *band-summed* W channel, and (iii)
asserts no notch deeper than X dB in the high band away from the room's resonances. The current tests
would not catch a regression here because they assert only monotone windowed energy.

**If B is confirmed.** The fix is in `SoundShaper`, not the convolver: do not send a source to the
EAX early-reflections slot when that source is already being served a convolution whose IR contains
early reflections. `earlySlots`/`sendEarly` (`SoundShaper.java:542-572`, `:608-616`) is the single
place to gate. Flag: this changes which auxiliary sends exist per source, i.e. behaviour, but **not**
the FFI ABI (all of this is Java-side `alSource3i` state). No ABI bump needed.

## 6. Answer to the A/B the user can run

- **Switch Earshot to a non-convolution reverb (EAX) while leaving Foley's settling enabled.**
  If the comb disappears → the artifact is in the convolution path (IR synthesis, candidate A) or in
  the convolution-vs-EAX-early-reflection overlap (candidate B); both live behind
  `ReverbMode.CONVOLUTION`.
- **Disable Foley's settling while Earshot stays on CONVOLUTION.** If the comb persists → Earshot's
  convolution path owns it (this also confirms it is not the Foley beta.56 grain issue).
- If it disappears only when *both* are off, the two are summing — which points at B, since B is the
  only mechanism where two sources of the same reflections combine.

The clean separator is: **convolution off, settling on.** That single toggle removes Earshot's IR
entirely while leaving every Foley grain path intact.
