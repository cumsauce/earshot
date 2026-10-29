# Temperature-dependent speed of sound: design (R3)

## Intent

The sim fixes `SPEED_OF_SOUND = 343.0` m/s everywhere. Real air's speed is
`c ≈ 331.3 + 0.606·T` (T in °C), so a desert at 40 °C carries sound about 2 % faster than a
snowy biome at −10 °C. That shifts echo and reflection delays (and the pitch OpenAL's
Doppler gives), which is a small but audible difference between climates: the plan puts it
in an A/B listening pass, not a numeric test.

## Goal, and what it deliberately does not touch

Thread one temperature, sampled from the listener's biome, from the game into the sim and
use it for the *timing* paths:

- the early reflections' and edge sources' delays (`early.rs`, `ir.rs`),
- the reverb's EAX delays (`eax.rs`),
- the ground-reflection model's wavenumber and path lengths (`ground.rs`),
- the impulse response's length and onset (`ir.rs`).

It does **not** change room measurement (`reverb.rs`): its tally, Eyring decay and the
BRAS benchmark keep the fixed `SPEED_OF_SOUND`. That keeps the golden scenes and BRAS
exactly as they are, confines the change to what is heard rather than measured, and is a
defensible line: the decay of a room is a measured property, the timing of its echoes is
the air's.

Two more paths stay on the fixed constant on purpose, outside the sim: thunder's
propagation delay in Java, which uses its own `SPEED_OF_SOUND` of 343 m/s
(`Thunder.java`), and the pitch OpenAL's Doppler gives, which keeps OpenAL Soft's own
fixed speed of sound — the mod never calls `AL_SPEED_OF_SOUND`. The climate moves
neither.

## The ABI (v15 → v16)

The sim cannot know the temperature without Java telling it. Cleanest fit with the
existing shape: one new setter, `earshot_set_air(engine, celsius)`, called on the same
clock as `earshot_set_listener` (once a tick).

- Add `Engine::set_air(celsius: f64)` storing a clamped temperature (say −60..=60 °C);
  `Engine::air_celsius()` reads it back. Default 20 °C, so `c = 343.42` — the current
  constant within 0.1 %, so nothing changes until a climate differs.
- Add the extern in `lib.rs` inside `guard`, validating finiteness and range like the
  other setters; it takes a scalar, not an array.
- Bump `EARSHOT_ABI_VERSION` 15 → 16 in `lib.rs` and `NativeBridge.ABI_VERSION`; document
  line 16 in the header's changelog comment. Regenerate the header with
  `cargo xtask header` (CI checks it with `--check`).
- `cargo xtask dist` and the natives build are unaffected; only the version gate refuses
  an old library, as intended.

This **does** touch the ABI, which the ground rules say to flag: the version bump, the
`xtask header` regeneration and the Java `ABI_VERSION` are all part of the change.

## Formulas

- Java: `celsius = clamp(20 · biome.getBaseTemperature(), -30, 50)`, the same sampling
  `SoundShaper.followClimate` already does for humidity. Reused, not re-derived.
- Sim: `pub fn speed_of_sound(celsius: f64) -> f64 { 331.3 + 0.606 * celsius.clamp(-60.0, 60.0) }`.
  At 20 °C that is 343.42; at −30 °C 313.1; at 50 °C 361.6.
- Clamp the result into a sane band (`300..=370`) so a silly temperature can never produce
  a zero or negative speed and a divide-by-zero.

## Where it is used

The timing functions gain the speed as an argument (or read it off the `Room`), rather
than reaching for the constant:

- `early.rs`: `early_taps`, `edge_sources` and their delay divisions take `c`.
- `ir.rs`: `impulse_response[_for]` takes `c` for its length/onset and the early taps'
  travel-time maths.
- `eax.rs`: `eax_reverb(room, c)` for the three delay figures.
- `ground.rs`: `excess_attenuation_db` takes `c` (it built `k0 = 2π f / c` from a
  constant `C0`; that is the parameter now), and `hear` passes it down.
- `reverb.rs`: unchanged — `SPEED_OF_SOUND` stays the constant its decay maths uses.

`Engine` reads `temperature` where it assembles a `Room` or an IR and passes
`speed_of_sound(self.air_celsius())`.

## Test plan

1. `speed_of_sound_matches_the_textbook_curve` (unit): 0 °C ≈ 331.3, 20 °C ≈ 343.4,
   clamped outside −60..60, and the clamp band for absurd inputs.
2. `colder_air_delays_an_echo` (unit, `early.rs`): the same geometry at −20 °C and 40 °C
   gives delays that differ by about the ratio of the two speeds, and both stay finite.
3. `eax_delays_scale_with_the_air` (unit, `eax.rs`): the delay fields at a cold and a warm
   temperature differ in the expected direction.
4. A scene or `hear` test: the ground reflection's null at a band shifts direction with the
   temperature (the wavelength changes), asserted as a monotonic move, not a pinned value.
   It landed as a `ground.rs` unit test, walking the temperatures weather produces.
5. The golden scenes and BRAS **do not move** (the measurement is untouched) — the existing
   tests are the regression proof, run unchanged.
6. `Engine::set_air` clamps and is readable; the FFI extern rejects a non-finite or
   out-of-range temperature (an ffi test, like the other setters').

## What the builder decides

- Whether the speed is threaded as a parameter or read from a small `Air` struct on the
  `Room`; the parameter is simpler and keeps `Room` a measurement.
- The exact clamp band and the default (20 °C) — both one-liners.
- Whether to also scale the `ir.rs` fade and `air_absorption` path length by it (they use
  the same constant; leave them unless it falls out naturally).
