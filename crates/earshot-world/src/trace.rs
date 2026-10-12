use std::ops::ControlFlow;

use crate::{BANDS, Bands, Material, Section, SectionPos, Vec3, World};

/// One block a ray passes through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub block: [i32; 3],
    pub material: Material,
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
    pub distance: f64,
    pub point: Vec3,
    /// The outward normal of the face hit; zero when the ray starts inside a
    /// solid block.
    pub normal: Vec3,
}

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
        let material = cache.block(world, block);
        let here = Step {
            block,
            material,
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
    walk(world, origin, dir, max, |step| {
        if !step.material.is_solid() {
            return ControlFlow::Continue(());
        }
        let [x, y, z] = step.normal.map(f64::from);
        hit = Some(Hit {
            block: step.block,
            material: step.material,
            distance: step.enter,
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
/// chest or a note block comes from inside its own block.
pub fn transmission(world: &World, from: Vec3, to: Vec3) -> Bands {
    let delta = to - from;
    let distance = delta.length();
    let mut passing = [1.0f32; BANDS];
    if distance == 0.0 {
        return passing;
    }
    let ends = [floor(from), floor(to)];
    walk(world, from, delta * (1.0 / distance), distance, |step| {
        if !step.material.is_solid() || ends.contains(&step.block) {
            return ControlFlow::Continue(());
        }
        let length = (step.exit - step.enter) as f32;
        let through = step.material.acoustics().transmission;
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
    fn block(&mut self, world: &'a World, [x, y, z]: [i32; 3]) -> Material {
        let pos = SectionPos::of_block(x, y, z);
        if self.pos != Some(pos) {
            self.pos = Some(pos);
            self.section = world.section(pos);
        }
        self.section
            .map_or(Material::Air, |section| section.get(x, y, z))
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

    #[test]
    fn bad_input_ends_the_walk_at_once() {
        let world = World::new();
        let nan = Vec3::new(f64::NAN, 0.0, 0.0);
        assert!(steps(&world, nan, Vec3::new(1.0, 0.0, 0.0), 10.0).is_empty());
        let still = steps(&world, Vec3::ZERO, Vec3::ZERO, 10.0);
        assert_eq!(still.len(), 1);
    }
}
