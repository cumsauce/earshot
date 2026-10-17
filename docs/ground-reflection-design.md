# Outdoor ground reflection: design

Status: design for milestone R1 in `docs/PLAN.md`. No FFI or Java change.

## Intent

Open ground is dry today. Earshot's reverb gain is enclosure², so outdoors it nearly
vanishes, and the direct path treats the ground only as a thing the line of sight might
cross. Real ground reflects sound coherently, and that reflection interferes with the
direct wave: a deep, height-dependent null in the low band, moving as the source and the
listener move. It is the most audible gap left for outdoor play.

The sim is energy-based (three bands: 250 Hz, 1 kHz, 4 kHz), so the fine interference
cannot be traced ray by ray. Instead we compute the **excess attenuation** of the ground
effect at each band's centre frequency from the Weyl-Van der Pol model and fold it into
the direct path as a per-band real factor.

## Goals

- A per-band gain factor for the direct path over ground, from the ground's material and
  the two heights, at each band's centre frequency.
- Hard ground gives the +6 dB Lloyd's-mirror boost at grazing; porous ground gives a
  low-band null, deeper the lower the band.
- Deterministic, bounded, and cheap (no per-ray work).

## Non-goals

- Atmospheric turbulence, surface roughness, layered/variable ground, foliage, and
  impedance changes along the path (Fresnel zones). One impedance for the whole path.
- Indoor floors: a shut-in listener's floor is already the room trace's business; the
  ground effect is only applied in the open (see *When it applies*).
- Water waves; water is treated as hard reflecting (it does get a reflection, just no
  porous null).

## The model

Time dependence `e^{-iωt}` throughout. `Z` is the ground's normalised surface impedance
(relative to ρ₀c₀), a locally reacting (impedance-plane) surface.

Geometry, with the ground plane at `y = y_g`:

- `h_s = source.y - y_g`, `h_r = listener.y - y_g` (both ≥ 0), horizontal separation `d`.
- Direct path `R1 = |source - listener|`.
- Image-source (reflected) path `R2 = sqrt(d² + (h_s + h_r)²)`.
- `cos θ = (h_s + h_r) / R2` (θ from the surface normal).
- Excess length `Δ = R2 - R1`.
- `k0 = 2π f / c0`, `c0 = 343` m/s.

Surface impedance, **Miki's one-parameter model** (passivity-constrained refit of
Delany-Bazley; preferred below the fit range, which is where outdoor ground at these
bands sits). With `X = f / σ` (σ the effective flow resistivity, Pa·s/m²):

```
Z(f) = 1 + 0.070 X^(-0.632) + i 0.107 X^(-0.632)
```

(Imaginary part negated from the published `e^{+iωt}` form, where it is negative, into
this `e^{-iωt}` convention, so a passive ground has Im Z > 0. The sign was confirmed
against the two pinned limits below: only `+i` makes a porous ground dip.)

Weyl-Van der Pol (Attenborough 2.40c, Salomons D.58), with `β = 1/Z` the normalised
admittance:

```
Rp   = (cosθ - β) / (cosθ + β)
w    = sqrt(i k0 R2 / 2) · (cosθ + β)          (complex square root)
F(w) = 1 + i √π · w · e^(-w²) · erfc(-i w)
     = 1 + i √π · w · wofz(w)                  (wofz = Faddeeva function)
Q    = Rp + (1 - Rp) · F(w)
EA   = 20 log10 | 1 + Q · (R1/R2) · e^(i k0 Δ) |   dB re free field
```

The band factor is `g = 10^(EA/20)`, clamped to `[0, 2]` (2 is the +6.02 dB hard-ground
ceiling).

### Limits the implementation must reproduce (and the tests pin)

- **Hard ground** (`|Z| → ∞`, e.g. `β → 0`): `Rp → 1`, `w → 0`, `F → 1`, `Q → 1`. With the
  two heights small (grazing, `R2 → R1`, `Δ → 0`), `EA → 20 log10 2 = +6.02 dB`.
- **Grazing over porous ground** (`h_s, h_r → 0`): `Rp → -1` and the ground wave keeps `Q`
  bounded (the whole point of the `F(w)` term); `EA` stays finite, not `-∞`.
- **Reciprocity**: swapping `h_s` and `h_r` leaves `EA` unchanged (`R2`, `cosθ`, `w` depend
  only on the sum `h_s + h_r`).

### Faddeeva function

`wofz(z) = e^(-z²) erfc(-i z)` has no closed form; port **Weideman's rational
approximation** (Weideman 1994, *Computation of the complex error function*, SIAM J.
Numer. Anal. 31(5)) — about forty lines, complex arithmetic only, accurate to ~1e-14 over
the region needed here. Validate against `wofz(0) = 1` and a couple of tabulated complex
values. `wofp`'s argument here has small magnitude (near-grazing → `|w|` small), the
easiest regime.

## Inputs and where each comes from

| input | source |
| --- | --- |
| σ (flow resistivity) | new table, per `Material` (below) |
| `h_s`, `h_r` | ground plane `y_g` from the block under the listener; heights from `source.y`, `listener.y` |
| `d` | horizontal distance between them |
| `f` | band centres 250 / 1000 / 4000 Hz (as `WAVELENGTH` in `hear.rs` uses) |

### Flow resistivity per material (design values, Pa·s/m²)

Representative figures from the HARMONOISE/CNOSSOS ground classes and Attenborough's
fitted outdoor surfaces. A material with no entry is acoustically hard and gets the
Lloyd's-mirror boost instead (its `Z → ∞`), not a porous null.

| material | σ | note |
| --- | --- | --- |
| `Soil` (dirt, grass, mud, clay, moss) | 200 000 | grassland, the common case |
| `Sand` | 100 000 | dry, porous |
| `Snow` | 20 000 | very absorptive (deep low-band null) |
| `Leaves` (forest floor) | 15 000 | litter over soil |
| everything else (`Rock`, `Wood`, `Glass`, `Metal`, `Water`, `Ice`, `Lava`, `Cloth`, `Rug`, `Air`) | none | hard: `Z → ∞`, `Q → 1` |

These are starting values to tune by ear; each is a one-line change.

## When it applies

The ground effect is added when **all** hold, so it never double-counts an indoor floor
the room trace already handles:

- the listener is not shut in: `enclosure < OPEN` (start at `0.9`; a sealed room measures
  ~1, open ground ~0.5, sky ~0 — see `Room::enclosure`);
- there is solid ground under the listener within a short scan (cap the downward scan at
  16 blocks) — `Material::Air` below means sky, no ground;
- the source stands above the same ground plane (`h_s ≥ 0`, and the source's own ground,
  scanned the same way, is at (about) that plane — v1 accepts any solid block under the
  source within the cap);
- the ray count is not zero (`hear`'s `rays == 0` case stays free-field).

## The fold

In `crates/earshot-sim/src/hear.rs`, `hear(...)`: right after `let direct = occlusion(...)`,
and before the direct-vs-round comparison, multiply `direct` band by band by the ground
factor. The direct path over open ground is essentially free-field today (the straight
line does not hit the ground), so this adds the reflected wave's interference to a free
field, which is exactly what `EA` measures. The round-trip path (`field.route`) is not
affected: it is a different path, and its own end reflecting off the ground is a second
order effect.

## What the builder decides

- The shape of the new module (`crates/earshot-sim/src/ground.rs` is the natural home;
  a tiny local `Complex` and `wofz` live there since neither crate has a complex type).
- The exact Weideman port and its constants.
- The ground plane rule for v1 (listener's ground as the plane; the source's tolerance).
- The `OPEN` enclosure threshold and the σ values above, both easy to tune.

## Test plan

Unit (`ground.rs`), pure functions, no world needed:

1. `a_hard_ground_gives_the_six_decibel_boost_at_grazing` — `|Z| → ∞`, small heights:
   `EA = +6.02 dB`.
2. `a_porous_ground_dips_the_low_band` — `σ = 200_000`, `h_s = h_r = 1.6` m, `d = 20` m:
   the 250 Hz band's `EA` is a null (−2.44 dB), the mid band rides the crest
   (+3.20 dB), and the deepest null of the three lands in the 4 kHz band (−3.81 dB),
   because Miki's ground stiffens toward low frequency; assert the band pattern and
   pin each dB against the model's own value.
3. `grazing_over_a_porous_ground_stays_finite` — heights → 0: `EA` finite and above a
   sane floor, matching the ground-wave limit.
4. `the_ground_effect_is_reciprocal` — swap `h_s`, `h_r`: identical.
5. `the_ground_factor_stays_between_silence_and_the_hard_boost` — `g ∈ [0, 2]` over a
   sweep of materials, heights and distances.
6. `wofz_matches_its_known_values` — `wofz(0) = 1`, plus tabulated values.

Integration (`hear.rs`):

7. `the_ground_lifts_grazing_sound_over_hard_ground` — a rock floor at `y = -1`, listener
   and source a block above it, `enclosure = 0` (open): the mid band is boosted over the
   free-field value; a high source overhead is not.

Scene (`tests/scenes.rs`):

8. `the_ground_reflection_reaches_the_hearing` (scenes.rs): over open soil the direct
   gains change, while a shut-in listener and a listener over no ground hear them
   unchanged. The band-by-band shape is pinned in the unit tests, not here: it turns over
   with a fraction of a block of height.

`gains_stay_between_silence_and_full_however_it_gets_there` now allows gains up to 2
(the hard ground's +6 dB lift), not 1: over open ground the direct share can exceed full.

## The lift rides the source's own gain

`SoundShaper.filter` applies the direct gain as an EFX low-pass, whose gain is valid in
`[0, 1]`. So the band **shape** (attenuation) goes through the filter, and the part of the
gain above one, the ground's **lift** over hard ground, rides the source's own `AL_GAIN`
instead: the source gain is the game's volume (`Channel.setVolume`, remembered by the
`Channel` mixin into `SourceState`) times `SoundShaper.lift(gains)`, and the filter takes
`gains[0] / lift`. The two are exactly equivalent to the filter-only path for gains of one
or less, so nothing indoors changes. Voices are left alone: a voice's gain is
`VoicePlayer`'s own.
