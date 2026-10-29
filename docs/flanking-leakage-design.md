# Structure-borne relay along a shared wall (R4)

R4 in `docs/PLAN.md` is called flanking leakage. What is built here is the
direct-line half of it: a wall crossed at a shallow angle, which the mass law
in `trace.rs` collapses to nothing. True architectural flanking, sound leaving
one room through a junction or a continuous structure into another without
touching either room's air, is a different path and stays out of scope.

## What is missing

`crates/earshot-world/src/trace.rs`'s `transmission(world, from, to)` walks the
straight line and charges every solid block

```
passing *= material.transmission.powf(length * fill)
```

with `material.transmission` the material's whole-block share
(`crates/earshot-world/src/material.rs:63-72`, "a real wall's ... rather than a
solid metre's"). Head on that is right: the ray crosses the wall's thickness
once and pays the wall's loss once.

At a shallow angle it is wrong twice over. The travel inside the block grows as
`thickness / cos(incidence)`, so the wall's loss is raised to a length that goes
to infinity as the angle grazes, and the path dies. Sound does not do that.
Entering a dense structure it refracts toward the normal, so what it actually
pays is the wall's loss over the *thickness*, plus a lateral run inside the
material at the material's own structure-borne rate, which is far lower than a
wall crossing. That lateral run is the relay, and no existing path has it: the
straight walk charges it at wall-crossing rate, and the room trace
(`crates/earshot-sim/src/paths.rs`) either reflects or stochastically drains a
ray at a wall and never carries it along the structure.

For a slab of thickness `t` crossed at incidence θ the physical path is the
dogleg: enter perpendicular at one point, run `t·tanθ` inside the wall, leave
perpendicular at another. Splitting each block's travel by the *wall's* normal
computes exactly that dogleg along the straight walk, with no separate path
search: `through = t`, `along = t·tanθ`.

## The model

New material property, `Acoustics::structure: Bands`: the share of energy kept
per metre of lateral run *inside* the material. Dense and stiff conducts well
(metal, glass, rock), soft and porous does not (snow, cloth, leaves). The table
holds it at or above the material's own wall crossing everywhere but water — a
water crossing loses a thousandth of a decibel, a share just under one that no
relay rate could sit above — so a per-block max in `flank_transmission` carries
the rest of the guarantee: a relay never costs more than the crossing does.

New function in `crates/earshot-world/src/trace.rs`:

```
pub fn flank_transmission(world: &World, from: Vec3, to: Vec3) -> Bands
```

Same walk, same end-block skip, same water toll as `transmission`. It groups
the solid blocks into contiguous runs and gives every block in a run the same
normal, `n`, the face the ray entered *the run* through. Per block:

```
across  = |dir . n|                    // 1 head on, 0 grazing
through = length * across              // the thickness it pays a crossing for
along   = length * sqrt(1 - across^2)  // the run inside the material's plane
cost    = material.transmission.powf(through * fill)
        * material.structure.powf(along * fill)
passing *= cost.max(material.transmission.powf(length * fill))  // the mass-law floor
```

Head on, `along` is 0 and this is bit for bit `transmission`. Grazing, the
thickness pays the wall's loss and the long way along it pays the structure's —
never less than the mass law would charge for the same block, which is what the
`.max(...)` holds: the table cannot promise that on its own, since water's relay
rate sits under its crossing share, and a run the ray starts inside has no face
to charge a crossing against.

`transmission` itself is untouched. `hear::occlusion` switches to
`flank_transmission`; `early.rs`'s clear-line gates and everything else keep
calling `transmission`. That matters: cheaper grazing lines would otherwise
flip `early.rs:119-120`'s `transmission(...)[1] < CLEAR` gate open and spawn
early reflections *through* walls, which is a much larger behaviour change than
a golden drifting.

### Finding a run's normal

`walk_solid` visits only solid blocks, so the air-to-solid transition is not
among the visited steps. A new run starts when a step's `enter` is not the
previous step's `exit` (a gap: air, or a jumped section), and the first step of
a run sets `n = step.normal`. `Step::normal` is the face the block was entered
through (`trace.rs:115-116`), so this is the wall's normal for every block in
the run, which is what makes the split correct for a grazing ray.

## The values

`structure` is the share kept per metre, low to high band. Ordered by how well
the material conducts, and `>= transmission` for the same material and band
everywhere but water, where the crossing share is just under one (a test
enforces it, skipping that row). Water's relay sits within a hair of its
crossing, and the per-block max above holds the floor there.

| material | structure | material | structure |
| --- | --- | --- | --- |
| air | 1.0, 1.0, 1.0 | metal | 0.92, 0.88, 0.80 |
| rock | 0.80, 0.72, 0.60 | water | 1.0, 0.95, 0.90 |
| wood | 0.75, 0.65, 0.55 | lava | 0.55, 0.45, 0.35 |
| glass | 0.85, 0.78, 0.65 | ice | 0.85, 0.78, 0.68 |
| cloth | 0.40, 0.30, 0.22 | snow | 0.55, 0.45, 0.35 |
| leaves | 0.80, 0.70, 0.55 | rug | 0.85, 0.78, 0.70 |
| soil | 0.55, 0.45, 0.35 | sand | 0.50, 0.42, 0.32 |

Soft materials sit at or just above their own crossing, so they gain almost
nothing; hard ones gain a lot, which is the sign the mass law gets wrong.

## What could move

Only geometry where `hear`'s straight line crosses a wall at a shallow angle
changes. A near-perpendicular crossing is unchanged within rounding. Any scene
or BRAS figure that moves gets its before and after in the commit body, as
earlier milestones did; a golden is never edited to hide a move.

## Tests

Written first, watched fail, then implemented.

1. `an_axis_aligned_crossing_is_unchanged` (trace.rs, unit): a wall crossed
   exactly along an axis gives bit-for-bit the same bands from
   `flank_transmission` and `transmission`. Only an axis-aligned ray is exactly
   `across = 1`; anything else is compared within a small tolerance.
2. `a_shallow_crossing_beats_the_mass_law` (trace.rs, unit): the same wall
   crossed at a shallow angle passes strictly more from `flank_transmission`
   than `transmission`, per band, and the *ratio* grows as the angle shallows.
   Do not claim passing itself grows with shallowness; it falls in both models.
3. `the_relay_is_never_below_the_wall_crossing` (trace.rs, unit): every
   material's `structure[band] >= transmission[band]`, all shares in `0..=1`,
   and `flank_transmission >= transmission` band by band on a grazing ray.
4. `a_partial_block_grazed_lengthwise_scales_with_its_fill` (trace.rs, unit): a
   half slab grazed lengthwise keeps the `fill` scaling on both the crossing and
   the relay term.
5. `a_wall_between_two_offset_people_leaks_along_it` (hear.rs, unit): a sealed
   rock wall with source and listener each pressed against it and offset along
   it hears strictly more in the mid band than the pure mass law, and above an
   audibility floor (mid band `> 1e-4`), so the test proves the path is present
   rather than merely non-zero.
6. `gains` stay in `0..=2` and the determinism tests stay green.

## Out of scope

- Junction flanking: sound leaving one room through a shared floor, ceiling or
  corner into another with no common wall on the straight line. Needs a
  structure relay path of its own; not started here.
- A room-to-room reverb relay, one space's tail re-radiating into another.
- Any change to `paths.rs`'s reflection model, `early.rs`, or the material
  table's existing columns.
- Any extern, quality knob or ABI change. The ABI stays 16.
