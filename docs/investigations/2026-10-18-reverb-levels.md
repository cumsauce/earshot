# Convolution reverb is quiet and short in caves (Earshot)

*2026-10-18. Root-caused by rendering the room's impulse response with the same
pipeline the game uses, not by listening.*

## Symptom

> "convolution and eax seem to be different when it comes to things like reverb
> and echo. eax is more what i'd expect, whereas convolution feels flat in caves
> etc., like its not applying or something."

## Where the two paths actually are

Foley implements no reverb; it feeds the game's OpenAL like any mod (beds as
streams via `getAudioStream`, one-shots via `mixin/SoundBufferLibraryMixin`), and
Earshot shapes every channel. Both modes live in **this** repo:

- **EAX** — `mod/src/main/java/io/github/cumsauce/earshot/audio/SoundShaper.java`,
  `updateEax`, glided at `EAX_GLIDE = 0.15`/tick into one aux slot at **gain 1**.
  The room's decay time, gains and reflection delays go in uncapped.
- **Convolution** — `mod/src/main/java/io/github/cumsauce/earshot/audio/ConvolutionReverb.java`,
  using OpenAL Soft's `AL_SOFTX_convolution_effect`; IRs from
  `crates/earshot-sim/src/ir.rs::impulse_response`, B-format3D float32 @48 kHz,
  two slots crossfaded at `FADE_STEP = 0.2`/tick.
- **Selection** — `SoundShaper.chooseMode`: convolution only when asked for and not
  underwater, else EAX if the device has it, else off.

## Measurement

`crates/earshot-sim/tests/reverb_probe.rs` renders a cave-like room
(24×10×24 m rock chamber, listener at (12,3,12), 1024 rays × 8 bounces) through
`measure_with_early` → `impulse_response` — the same call the engine makes — and
compares it against `eax_reverb` for the same room.

Room: decay (low/mid/high) **8.00 / 5.13 / 2.59 s**, mean free path 10.83 m,
enclosure 1.00, volume 5751 m³, `late_energy` **0.2903**.

| cave room | before | after |
| --- | --- | --- |
| IR length as loaded | 1.50 s (cut from 5.13 s, tail forced −40 dB) | **3.00 s** |
| IR energy `2·ΣW²` (target = `late_energy` 0.290) | 0.1756 (−2.2 dB) | **0.2205 (−1.2 dB)** |
| wet/dry as the game mixes it | **−16.6 dB** | **−9.2 dB** (+7.4 dB) |
| effective RT60 (EAX reference: `decay_time` 5.13 s) | ≈2.25 s | **≈4.5 s** |
| open ground (IR 0.41 s, unchanged) | −18.5 dB | −12.5 dB |

EAX for the same room: `gain` 0.320, `late_reverb_gain` 1.260,
`decay_time` 5.13 s, `reflections_gain` 0.453 — and the EAX slot never has its
`AL_EFFECTSLOT_GAIN` lowered.

## Cause — two compounding attenuations, not a broken effect

1. **The convolution slot was held 6 dB under the room's own reverb.** `LEVEL`
   was 0.5, but the IR is *already* normalised to the room's `late_energy`
   (`ir.rs`, the renormalising step), so the slot gain was a second attenuation
   the EAX path does not have. Wet sat **−16.6 dB** against EAX's unity slot.
2. **The IR was cut to 1.5 s**, so a 5.13 s room decay was forced down to an
   effective 2.25 s — that is the "flat" quality: the tail was simply missing.

Both hypotheses from the investigation were partly right and partly wrong: the
IR **energy cap** (`MAX_ENERGY = 1.0`) was never binding (0.176), and
`late_energy` itself is healthy in a cave (0.29). The double-cap theory is
**refuted**; the "too short" theory is **confirmed**, as is the slot-level
theory.

The open ground barely changes, because its IR is short either way — which is
exactly why the complaint was cave-specific.

## Fix

`LEVEL` 0.5 → **1.0** and `MAX_SECONDS` 1.5 → **3.0** in
`ConvolutionReverb.java`.

Cost, from `docs/PLAN.md`'s own benchmark table: 2×3 s IRs across 32 sources
rise from 17.1% to 32.0% of a core — and only rooms that actually ring past
1.5 s pay it; the open ground's 0.41 s IR is untouched.

## Honest limits

- `ConvolutionReverbTest`'s new tests (`the_slot_is_not_held_below_the_rooms_own_reverb`,
  `a_response_may_ring_as_long_as_the_room`) assert **constants**. There is no
  OpenAL seam to unit-test the mix; the real evidence is the table above.
- The absolute loudness of OpenAL Soft's own convolution cannot be rendered
  outside the game, so the comparison is model-free against the IR's design
  target (`late_energy`) and against EAX's parameters for the same room.
