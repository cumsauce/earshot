use std::ops::ControlFlow;

use crate::{BANDS, Bands, Material, Section, SectionPos, Vec3, World};

/// One block a ray passes through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub block: [i32; 3],
    pub material: Material,
    /// How much of the block is there, 0 to 1: half for a slab, 1 for a
    /// full block. Transmission loss scales with it (the mass law: the loss
    /// is of the thickness).
    pub fill: f32,
    /// How far along the ray it enters and leaves the block.
    pub enter: f64,
    pub exit: f64,
    /// The outward normal of the face the ray came in through; zero for the
    /// block it starts in.
    pub normal: [i32; 3],
}

/// Where a ray first meets something solid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub block: [i32; 3],
    pub material: Material,
    /// How much of the hit block is there, 0 to 1: the share of the energy
    /// a crossing takes.
    pub fill: f32,
    pub distance: f64,
    /// How far along the ray it leaves the hit block, for sampling what's
    /// behind it.
    pub exit: f64,
    pub point: Vec3,
    /// The outward normal of the face hit; zero when the ray starts inside a
    /// solid block.
    pub normal: Vec3,
}

/// The furthest a ray may travel, in blocks: past it the walk would step
/// the i32 block coordinates toward an overflow, so both walks stop here
/// whatever `max` asks for, and the FFI refuses a larger reach.
pub const MAX_REACH_METRES: f64 = 4096.0;

/// Walks the blocks a ray from `origin` in direction `dir` (unit length)
/// passes through within `max` blocks, in order (Amanatides–Woo: each step
/// crosses one block boundary). `visit` can stop the walk early.
pub fn walk(
    world: &World,
    origin: Vec3,
    dir: Vec3,
    max: f64,
    mut visit: impl FnMut(&Step) -> ControlFlow<()>,
) {
    let finite = [origin.x, origin.y, origin.z, dir.x, dir.y, dir.z, max];
    if !finite.iter().all(|v| v.is_finite()) {
        return;
    }
    // A larger reach would walk the block coordinates toward an i32
    // overflow, so the bound holds whatever the caller passed.
    let max = max.min(MAX_REACH_METRES);
    let mut block = [
        origin.x.floor() as i32,
        origin.y.floor() as i32,
        origin.z.floor() as i32,
    ];
    let mut step = [0i32; 3];
    let mut t_delta = [f64::INFINITY; 3];
    let mut t_max = [f64::INFINITY; 3];
    for axis in 0..3 {
        let (d, o, b) = (dir.axis(axis), origin.axis(axis), f64::from(block[axis]));
        if d > 0.0 {
            step[axis] = 1;
            t_delta[axis] = 1.0 / d;
            t_max[axis] = (b + 1.0 - o) / d;
        } else if d < 0.0 {
            step[axis] = -1;
            t_delta[axis] = -1.0 / d;
            t_max[axis] = (o - b) / -d;
        }
    }

    let mut cache = Cache::default();
    let mut enter = 0.0;
    let mut normal = [0; 3];
    loop {
        let axis = if t_max[0] < t_max[1] {
            if t_max[0] < t_max[2] { 0 } else { 2 }
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        let exit = t_max[axis].min(max);
        let (material, fill) = cache.block(world, block);
        let here = Step {
            block,
            material,
            fill,
            enter,
            exit,
            normal,
        };
        if visit(&here).is_break() || t_max[axis] >= max {
            return;
        }
        enter = t_max[axis];
        block[axis] += step[axis];
        t_max[axis] += t_delta[axis];
        normal = [0; 3];
        normal[axis] = -step[axis];
    }
}

/// The first solid block a ray from `origin` in direction `dir` (unit
/// length) meets within `max` blocks.
pub fn cast(world: &World, origin: Vec3, dir: Vec3, max: f64) -> Option<Hit> {
    let mut hit = None;
    walk_solid(world, origin, dir, max, |step| {
        let [x, y, z] = step.normal.map(f64::from);
        hit = Some(Hit {
            block: step.block,
            material: step.material,
            fill: step.fill,
            distance: step.enter,
            exit: step.exit,
            point: origin + dir * step.enter,
            normal: Vec3::new(x, y, z),
        });
        ControlFlow::Break(())
    });
    hit
}

/// The share of sound energy, per band, that gets straight through the
/// blocks between `from` and `to`. Each block passes its material's
/// transmission raised to the length of the path inside it, so clipping a
/// corner costs little. The blocks at either end don't count: a sound from a
/// chest or a note block comes from inside its own block. Crossing the
/// water's surface costs [`WATER_INTERFACE_DB`] once per crossing.
pub fn transmission(world: &World, from: Vec3, to: Vec3) -> Bands {
    let delta = to - from;
    let distance = delta.length();
    let mut passing = [1.0f32; BANDS];
    if distance == 0.0 {
        return passing;
    }
    let dir = delta * (1.0 / distance);
    let ends = [floor(from), floor(to)];
    walk_solid(world, from, dir, distance, |step| {
        if ends.contains(&step.block) {
            return ControlFlow::Continue(());
        }
        if step.material == Material::Water {
            // The surface's toll is per crossing, not per block: charged on
            // each face of a water block that borders another medium, so a
            // submerged path pays nothing and an air bubble pays twice.
            let mut surface = |neighbour: [i32; 3]| {
                if world.block(neighbour[0], neighbour[1], neighbour[2]) != Material::Water {
                    for (passing, db) in passing.iter_mut().zip(WATER_INTERFACE_DB) {
                        *passing *= 10f32.powf(-db / 10.0);
                    }
                }
            };
            if step.normal != [0; 3] {
                surface([
                    step.block[0] + step.normal[0],
                    step.block[1] + step.normal[1],
                    step.block[2] + step.normal[2],
                ]);
            }
            surface(exit_neighbour(from, dir, step));
        }
        let length = (step.exit - step.enter) as f32;
        let through = step.material.acoustics().transmission;
        // The loss is of the thickness (the mass law): a half-slab passes
        // the square root of what the full block does.
        let length = length * step.fill;
        for (passing, through) in passing.iter_mut().zip(through) {
            *passing *= through.powf(length);
        }
        if passing.iter().all(|&p| p < 1e-6) {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    passing
}

/// What crossing the air–water surface costs, once per crossing, per band.
/// The plane-wave figure for the impedance jump (water is some 3600× air,
/// which passes a tenth of a percent of the energy) is 29.5 dB flat; the
/// tilt lets lows squeeze through an edged or rough surface a little better.
const WATER_INTERFACE_DB: Bands = [25.0, 30.0, 35.0];

/// The block just across the face a ray leaves `step` through.
fn exit_neighbour(origin: Vec3, dir: Vec3, step: &Step) -> [i32; 3] {
    let past = origin + dir * (step.exit + 1e-6);
    [
        past.x.floor() as i32,
        past.y.floor() as i32,
        past.z.floor() as i32,
    ]
}

/// [`walk`], visiting only the solid blocks, and crossing a section with
/// nothing in it (absent from the world: all air) in one go rather than block
/// by block. Outdoors and in big rooms most of a ray is in such sections.
/// The jump makes the same additions the block-by-block walk would, so it
/// lands exactly where that would.
fn walk_solid(
    world: &World,
    origin: Vec3,
    dir: Vec3,
    max: f64,
    mut visit: impl FnMut(&Step) -> ControlFlow<()>,
) {
    let finite = [origin.x, origin.y, origin.z, dir.x, dir.y, dir.z, max];
    if !finite.iter().all(|v| v.is_finite()) {
        return;
    }
    // A larger reach would walk the block coordinates toward an i32
    // overflow, so the bound holds whatever the caller passed.
    let max = max.min(MAX_REACH_METRES);
    let mut block = [
        origin.x.floor() as i32,
        origin.y.floor() as i32,
        origin.z.floor() as i32,
    ];
    let mut step = [0i32; 3];
    let mut t_delta = [f64::INFINITY; 3];
    let mut t_max = [f64::INFINITY; 3];
    for axis in 0..3 {
        let (d, o, b) = (dir.axis(axis), origin.axis(axis), f64::from(block[axis]));
        if d > 0.0 {
            step[axis] = 1;
            t_delta[axis] = 1.0 / d;
            t_max[axis] = (b + 1.0 - o) / d;
        } else if d < 0.0 {
            step[axis] = -1;
            t_delta[axis] = -1.0 / d;
            t_max[axis] = (o - b) / -d;
        }
    }

    let mut current: Option<SectionPos> = None;
    let mut section: Option<&Section> = None;
    let mut enter = 0.0;
    let mut normal = [0; 3];
    loop {
        let pos = SectionPos::of_block(block[0], block[1], block[2]);
        if current != Some(pos) {
            current = Some(pos);
            section = world.section(pos);
        }
        let Some(section) = section else {
            // Nothing here: on to where the ray leaves the section, the
            // first of the crossings that leave it along each axis.
            let mut leave = f64::INFINITY;
            let mut across = 3;
            for axis in 0..3 {
                if step[axis] == 0 {
                    continue;
                }
                let inside = if step[axis] > 0 {
                    15 - (block[axis] & 15)
                } else {
                    block[axis] & 15
                };
                let mut t = t_max[axis];
                for _ in 0..inside {
                    t += t_delta[axis];
                }
                if t < leave {
                    leave = t;
                    across = axis;
                }
            }
            if leave >= max {
                return;
            }
            // Every crossing before it stays in the section; then out.
            for axis in 0..3 {
                while t_max[axis] < leave {
                    block[axis] += step[axis];
                    t_max[axis] += t_delta[axis];
                }
            }
            enter = t_max[across];
            block[across] += step[across];
            t_max[across] += t_delta[across];
            normal = [0; 3];
            normal[across] = -step[across];
            continue;
        };
        let axis = if t_max[0] < t_max[1] {
            if t_max[0] < t_max[2] { 0 } else { 2 }
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        let material = section.get(block[0], block[1], block[2]);
        if material.is_solid() {
            let here = Step {
                block,
                material,
                fill: section.fill(block[0], block[1], block[2]),
                enter,
                exit: t_max[axis].min(max),
                normal,
            };
            if visit(&here).is_break() {
                return;
            }
        }
        if t_max[axis] >= max {
            return;
        }
        enter = t_max[axis];
        block[axis] += step[axis];
        t_max[axis] += t_delta[axis];
        normal = [0; 3];
        normal[axis] = -step[axis];
    }
}

fn floor(point: Vec3) -> [i32; 3] {
    [
        point.x.floor() as i32,
        point.y.floor() as i32,
        point.z.floor() as i32,
    ]
}

/// The section of the last block looked up: a ray stays in one section for
/// many steps, so most steps skip the hash lookup.
#[derive(Default)]
struct Cache<'a> {
    pos: Option<SectionPos>,
    section: Option<&'a Section>,
}

impl<'a> Cache<'a> {
    fn block(&mut self, world: &'a World, [x, y, z]: [i32; 3]) -> (Material, f32) {
        let pos = SectionPos::of_block(x, y, z);
        if self.pos != Some(pos) {
            self.pos = Some(pos);
            self.section = world.section(pos);
        }
        self.section.map_or((Material::Air, 1.0), |section| {
            (section.get(x, y, z), section.fill(x, y, z))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift64*, enough for test rays.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
        }

        fn signed(&mut self) -> f64 {
            self.next() * 2.0 - 1.0
        }
    }

    fn steps(world: &World, origin: Vec3, dir: Vec3, max: f64) -> Vec<Step> {
        let mut steps = Vec::new();
        walk(world, origin, dir, max, |step| {
            steps.push(*step);
            ControlFlow::Continue(())
        });
        steps
    }

    #[test]
    fn a_walk_stops_at_the_reach_limit() {
        let world = World::new();
        let steps = steps(
            &world,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(1.0, 0.0, 0.0),
            1e300,
        );
        let last = steps.last().unwrap();
        assert!((last.exit - MAX_REACH_METRES).abs() < 1e-9, "{last:?}");
    }

    #[test]
    fn walks_cover_the_ray_block_by_block() {
        let world = World::new();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..2000 {
            let origin = Vec3::new(rng.signed(), rng.signed(), rng.signed()) * 40.0;
            let dir = Vec3::new(rng.signed(), rng.signed(), rng.signed()).normalized();
            let max = rng.next() * 30.0;
            let steps = steps(&world, origin, dir, max);
            assert_eq!(steps[0].enter, 0.0);
            assert!((steps.last().unwrap().exit - max).abs() < 1e-9);
            for pair in steps.windows(2) {
                assert_eq!(pair[0].exit, pair[1].enter);
                let moved: i32 = (0..3)
                    .map(|axis| (pair[1].block[axis] - pair[0].block[axis]).abs())
                    .sum();
                assert_eq!(moved, 1, "steps go to a face neighbour");
            }
            for step in &steps {
                // The ray is inside the block between entering and leaving.
                let middle = origin + dir * ((step.enter + step.exit) / 2.0);
                if step.exit - step.enter > 1e-9 {
                    assert_eq!(floor(middle), step.block, "{step:?}");
                }
            }
        }
    }

    /// The old way: every block, solid or not.
    fn cast_by_walking(world: &World, origin: Vec3, dir: Vec3, max: f64) -> Option<Hit> {
        let mut hit = None;
        walk(world, origin, dir, max, |step| {
            if !step.material.is_solid() {
                return ControlFlow::Continue(());
            }
            let [x, y, z] = step.normal.map(f64::from);
            hit = Some(Hit {
                block: step.block,
                material: step.material,
                fill: 1.0,
                distance: step.enter,
                exit: step.exit,
                point: origin + dir * step.enter,
                normal: Vec3::new(x, y, z),
            });
            ControlFlow::Break(())
        });
        hit
    }

    #[test]
    fn skipping_empty_sections_finds_the_same_hits() {
        let mut world = World::new();
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        // Scattered blocks, leaving most sections empty, some in clusters.
        for _ in 0..400 {
            let at = [rng.signed(), rng.signed(), rng.signed()].map(|v| (v * 60.0) as i32);
            for _ in 0..(rng.next() * 8.0) as usize {
                let jitter = [rng.signed(), rng.signed(), rng.signed()].map(|v| (v * 2.0) as i32);
                world.set_block(
                    at[0] + jitter[0],
                    at[1] + jitter[1],
                    at[2] + jitter[2],
                    Material::Rock,
                );
            }
        }
        for _ in 0..20_000 {
            let origin = Vec3::new(rng.signed(), rng.signed(), rng.signed()) * 50.0;
            let mut dir = Vec3::new(rng.signed(), rng.signed(), rng.signed());
            // Some along axes and planes, where ties are.
            if rng.next() < 0.2 {
                dir = Vec3::new(dir.x, 0.0, if rng.next() < 0.5 { 0.0 } else { dir.z });
            }
            if dir.length() < 1e-6 {
                continue;
            }
            let dir = dir.normalized();
            let max = rng.next() * 120.0;
            assert_eq!(
                cast(&world, origin, dir, max),
                cast_by_walking(&world, origin, dir, max),
                "{origin:?} {dir:?} {max}"
            );
        }
    }

    #[test]
    fn a_cast_stops_at_the_first_solid_face() {
        let mut world = World::new();
        for y in -3..3 {
            for z in -3..3 {
                world.set_block(5, y, z, Material::Rock);
            }
        }
        let origin = Vec3::new(0.5, 0.5, 0.5);
        let hit = cast(&world, origin, Vec3::new(1.0, 0.0, 0.0), 32.0).unwrap();
        assert_eq!(hit.block, [5, 0, 0]);
        assert_eq!(hit.material, Material::Rock);
        assert!((hit.distance - 4.5).abs() < 1e-9);
        assert_eq!(hit.normal, Vec3::new(-1.0, 0.0, 0.0));
        assert!(cast(&world, origin, Vec3::new(-1.0, 0.0, 0.0), 32.0).is_none());
        assert!(cast(&world, origin, Vec3::new(1.0, 0.0, 0.0), 4.0).is_none());
    }

    #[test]
    fn walls_pass_their_transmission_per_block() {
        let mut world = World::new();
        let from = Vec3::new(0.5, 0.5, 0.5);
        let to = Vec3::new(10.5, 0.5, 0.5);
        assert_eq!(transmission(&world, from, to), [1.0; BANDS]);

        world.set_block(5, 0, 0, Material::Rock);
        let rock = Material::Rock.acoustics().transmission;
        let one = transmission(&world, from, to);
        for band in 0..BANDS {
            assert!((one[band] - rock[band]).abs() < 1e-6);
        }
        world.set_block(6, 0, 0, Material::Rock);
        let two = transmission(&world, from, to);
        assert!((two[0] - rock[0] * rock[0]).abs() < 1e-6);
        assert!(two[2] < two[0], "highs are muffled more");
    }

    #[test]
    fn the_blocks_at_either_end_dont_muffle() {
        let mut world = World::new();
        // A note block at the source and the listener's head in a block.
        world.set_block(0, 0, 0, Material::Wood);
        world.set_block(10, 0, 0, Material::Wood);
        let through = transmission(&world, Vec3::new(0.5, 0.5, 0.5), Vec3::new(10.5, 0.5, 0.5));
        assert_eq!(through, [1.0; BANDS]);
    }

    /// The product over the ray's solid blocks of transmission^length, the
    /// way a slow walk would add it up, for holding [`transmission`] to.
    fn transmission_by_walking(world: &World, from: Vec3, to: Vec3) -> Bands {
        let delta = to - from;
        let distance = delta.length();
        let mut passing = [1.0f32; BANDS];
        walk(world, from, delta * (1.0 / distance), distance, |step| {
            if !step.material.is_solid() {
                return ControlFlow::Continue(());
            }
            let length = (step.exit - step.enter) as f32;
            for (passing, through) in passing
                .iter_mut()
                .zip(step.material.acoustics().transmission)
            {
                *passing *= through.powf(length);
            }
            ControlFlow::Continue(())
        });
        passing
    }

    #[test]
    fn diagonal_transmission_matches_a_slow_walk_through_the_same_blocks() {
        let mut world = World::new();
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        for _ in 0..600 {
            let at = [rng.signed(), rng.signed(), rng.signed()].map(|v| (v * 20.0) as i32);
            let solids = [
                Material::Rock,
                Material::Wood,
                Material::Glass,
                Material::Cloth,
                Material::Leaves,
                Material::Soil,
                Material::Sand,
                Material::Metal,
                Material::Lava,
                Material::Ice,
                Material::Snow,
            ];
            let material = solids[(rng.next() * 11.0) as usize % solids.len()];
            world.set_block(at[0], at[1], at[2], material);
        }
        let from = Vec3::new(0.5, 0.5, 0.5);
        for _ in 0..2000 {
            let to = Vec3::new(rng.signed(), rng.signed(), rng.signed()) * 20.0;
            let want = transmission_by_walking(&world, from, to);
            let got = transmission(&world, from, to);
            // The end blocks count in the slow walk but not in transmission;
            // ignore results where that difference can show.
            let ends =
                [from, to].map(|p| [p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32]);
            let end_solid = ends
                .iter()
                .any(|b| world.block(b[0], b[1], b[2]).is_solid());
            if end_solid {
                continue;
            }
            for (got, want) in got.iter().zip(want) {
                assert!((got - want).abs() < 1e-4, "{got} vs {want}");
            }
        }
    }

    #[test]
    fn clipping_a_corner_loses_little_and_the_middle_of_a_wall_loses_it_all() {
        let mut world = World::new();
        // A hedge five blocks thick, x from 3 to 7: thick enough to check
        // the per-block powers, thin enough in loss to stay above
        // transmission's −60 dB early-out.
        for x in 3..=7 {
            for y in -2..=2 {
                for z in -2..=2 {
                    world.set_block(x, y, z, Material::Leaves);
                }
            }
        }
        // Through the middle: five blocks in a row.
        let straight = transmission(&world, Vec3::new(0.5, 0.5, 0.5), Vec3::new(9.5, 0.5, 0.5));
        let leaves = Material::Leaves.acoustics().transmission;
        for (got, want) in straight.iter().zip(leaves) {
            assert!(
                (got - want * want * want * want * want).abs() < 1e-6,
                "{got} vs {want}"
            );
        }
        // A stone wall in the way loses nearly everything in one block.
        let mut world = World::new();
        for y in -2..=2 {
            for z in -2..=2 {
                world.set_block(5, y, z, Material::Rock);
            }
        }
        let straight = transmission(&world, Vec3::new(0.5, 0.5, 0.5), Vec3::new(9.5, 0.5, 0.5));
        let rock = Material::Rock.acoustics().transmission;
        for (got, want) in straight.iter().zip(rock) {
            assert!((got - want).abs() < 1e-12, "{got} vs {want}");
        }
        // Over the wall's top edge, the ray leaves through the top block's
        // upper face partway across: here from (5, 2.9) to (5.19, 3.0),
        // under a quarter of a block, so it loses far less than the full
        // block straight through the middle.
        let grazing = transmission(&world, Vec3::new(0.5, 0.5, 0.5), Vec3::new(9.5, 5.3, 0.5));
        assert!(
            grazing[1] > straight[1] && grazing[1] < 1.0,
            "{grazing:?} vs {straight:?}"
        );
        // Clear over the top, nothing in the way at all.
        let over = transmission(&world, Vec3::new(0.5, 0.5, 0.5), Vec3::new(9.5, 5.6, 0.5));
        assert_eq!(over, [1.0; BANDS], "{over:?}");
    }

    /// A slab of water `at` blocks deep along x, from z -4 to 4 and
    /// y -4 to 4.
    fn water_slab(at: i32, depth: i32) -> World {
        let mut world = World::new();
        for z in -4..=4 {
            for y in -4..=4 {
                for x in at..at + depth {
                    world.set_block(x, y, z, Material::Water);
                }
            }
        }
        world
    }

    #[test]
    fn open_water_passes_sound_almost_as_well_as_air() {
        let world = water_slab(-10, 20);
        let from = Vec3::new(-9.5, 0.5, 0.5);
        let to = Vec3::new(9.5, 0.5, 0.5);
        let through_water = transmission(&world, from, to);
        let through_air = transmission(&World::new(), from, to);
        // Twenty blocks of open water, both ends under: what air passes,
        // water passes too. The old table took a hundred decibels over this.
        for band in 0..BANDS {
            assert!(
                through_water[band] > 0.85 * through_air[band],
                "{through_water:?} vs {through_air:?}"
            );
        }
    }

    #[test]
    fn a_fully_submerged_path_pays_no_surface_toll() {
        let world = water_slab(-10, 20);
        // Both ends in the water: no air on the way, so no crossing at all.
        let through = transmission(&world, Vec3::new(-9.5, 0.5, 0.5), Vec3::new(9.5, 0.5, 0.5));
        assert!(through[2] > 0.95, "{through:?}");
    }

    #[test]
    fn crossing_the_surface_once_costs_the_interface_toll() {
        let world = water_slab(0, 4);
        // In the air above (y = 3 is water; go from y = 5), down into the
        // water: one crossing.
        let down = transmission(&world, Vec3::new(1.5, 5.5, 0.5), Vec3::new(1.5, 0.5, 0.5));
        let mid = f64::from(down[1]);
        assert!(
            (mid - 10f64.powf(-30.0 / 10.0)).abs() < 0.4 * mid,
            "{down:?}"
        );
        // Lows cross the surface a little better than highs.
        assert!(down[0] > down[2], "{down:?}");
        // And back up: the toll is the same either way.
        let up = transmission(&world, Vec3::new(1.5, 0.5, 0.5), Vec3::new(1.5, 5.5, 0.5));
        assert!((f64::from(up[1]) - mid).abs() < 1e-6, "{up:?} vs {down:?}");
    }

    #[test]
    fn in_and_out_of_a_pond_costs_two_crossings() {
        let world = water_slab(-1, 3);
        // Air on both sides: through a 3-block pond and out again.
        let through = transmission(&world, Vec3::new(-4.5, 0.5, 0.5), Vec3::new(4.5, 0.5, 0.5));
        let expected = 10f64.powf(-60.0 / 10.0);
        let mid = f64::from(through[1]);
        assert!((mid - expected).abs() < 0.4 * expected, "{through:?}");
    }

    #[test]
    fn an_air_bubble_in_the_water_costs_twice() {
        let mut world = water_slab(-10, 20);
        for y in -1..=1 {
            for z in -1..=1 {
                world.set_block(0, y, z, Material::Air);
            }
        }
        let through = transmission(&world, Vec3::new(-9.5, 0.5, 0.5), Vec3::new(9.5, 0.5, 0.5));
        let expected = 10f64.powf(-60.0 / 10.0);
        let mid = f64::from(through[1]);
        assert!((mid - expected).abs() < 0.4 * expected, "{through:?}");
    }

    #[test]
    fn a_half_slab_passes_the_square_root_of_the_full_loss() {
        let mut world = World::new();
        // One full rock block.
        world.set_block(5, 0, 0, Material::Rock);
        let from = Vec3::new(0.5, 0.5, 0.5);
        let to = Vec3::new(9.5, 0.5, 0.5);
        let full = transmission(&world, from, to);
        // The same wall, half filled: the mass law halves the thickness.
        world.set_block_fill(5, 0, 0, Material::Rock, 0.5);
        let slab = transmission(&world, from, to);
        for band in 0..BANDS {
            assert!(
                (f64::from(slab[band]) - (f64::from(full[band])).sqrt()).abs()
                    < 1e-4 * f64::from(full[band]).sqrt(),
                "{slab:?} against {full:?}"
            );
        }
        // Pathing still treats the slab as solid: it is something to walk
        // around, whatever it passes.
        assert!(world.block(5, 0, 0).is_solid());
    }

    #[test]
    fn two_half_slabs_are_one_full_block() {
        // The loss is of the thickness: two half-filled blocks in a row pass
        // what one full block does, to within a powf's rounding — the two
        // walks pay different exponents, and libm's last bit differs by
        // platform.
        let mut halves = World::new();
        halves.set_block_fill(5, 0, 0, Material::Rock, 0.5);
        halves.set_block_fill(6, 0, 0, Material::Rock, 0.5);
        let mut whole = World::new();
        whole.set_block(6, 0, 0, Material::Rock);
        let through_halves =
            transmission(&halves, Vec3::new(0.5, 0.5, 0.5), Vec3::new(9.5, 0.5, 0.5));
        let through_whole =
            transmission(&whole, Vec3::new(0.5, 0.5, 0.5), Vec3::new(9.5, 0.5, 0.5));
        for (halves, whole) in through_halves.iter().zip(through_whole) {
            assert!(
                (halves - whole).abs() < 1e-5 * whole,
                "{through_halves:?} against {through_whole:?}"
            );
        }
    }

    #[test]
    fn the_surface_toll_is_the_impedance_jump() {
        // Water is about 3600× air by impedance; a plane wave crossing
        // passes 4·Z₁·Z₂/(Z₁+Z₂)² of its energy — the toll the table's mid
        // band sits on.
        let ratio: f64 = 3600.0;
        let share = 4.0 * ratio / (1.0 + ratio).powi(2);
        let toll = -10.0 * share.log10();
        assert!(
            (f64::from(WATER_INTERFACE_DB[1]) - toll).abs() < 0.6,
            "the table says {} dB, the impedance jump {toll} dB",
            WATER_INTERFACE_DB[1]
        );
        // And the band tilt only ever helps the lows (a const, so the check
        // compiles itself).
        const _: () = assert!(
            WATER_INTERFACE_DB[0] < WATER_INTERFACE_DB[1]
                && WATER_INTERFACE_DB[1] < WATER_INTERFACE_DB[2]
        );
    }

    #[test]
    fn bad_input_ends_the_walk_at_once() {
        let world = World::new();
        let nan = Vec3::new(f64::NAN, 0.0, 0.0);
        assert!(steps(&world, nan, Vec3::new(1.0, 0.0, 0.0), 10.0).is_empty());
        let still = steps(&world, Vec3::ZERO, Vec3::ZERO, 10.0);
        assert_eq!(still.len(), 1);
    }
}
