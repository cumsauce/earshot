use earshot_world::{Material, Vec3, World, transmission};

use crate::{Reflection, SPEED_OF_SOUND};

/// How far from the source a wall may be to give a first reflection, in
/// blocks.
const REACH: i32 = 16;
/// A leg of a reflection's way must let at least this much of the mid band
/// through to count.
const CLEAR: f32 = 0.5;
/// Steps a point this far off a wall face, to test the block either side.
const NUDGE: f64 = 0.01;

/// A sound's first reflections off the walls around it, as the listener
/// hears them, summed: for an early-reflections effect per source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Early {
    /// Their energy, mid band, relative to the direct sound in the open.
    pub energy: f32,
    /// How much later than the direct sound the first of them arrives, in
    /// seconds.
    pub delay: f32,
    /// Where they come from on average, energy-weighted: a world-axes
    /// vector from the listener, no longer than 1 (shorter when they come
    /// from all round).
    pub direction: Vec3,
    /// How many there are.
    pub count: u32,
}

impl Early {
    pub const NONE: Early = Early {
        energy: 0.0,
        delay: 0.0,
        direction: Vec3::ZERO,
        count: 0,
    };
}

/// The first-order reflections of a sound at `source` off the walls around
/// it, heard at `listener`. Walls are block faces, so each of the six axis
/// directions from the source gives at most one: the nearest face that way,
/// whose image source is exact. A reflection counts when its point lands on
/// a solid block, the listener is on the source's side of the wall, and
/// both legs are mostly clear. Its energy is the wall's mirrored share,
/// `(1 - absorption) * (1 - scattering)`, times the inverse-square loss of
/// the longer way against the direct one.
pub fn early_reflections(world: &World, listener: Vec3, source: Vec3) -> Early {
    let direct = (source - listener).length().max(0.5);
    let mut energy = 0.0f32;
    let mut first = f32::INFINITY;
    let mut sum = Vec3::ZERO;
    let mut count = 0;
    for axis in 0..3 {
        for sign in [-1i32, 1] {
            let Some(plane) = wall_along(world, source, axis, sign) else {
                continue;
            };
            let (s, l) = (get(source, axis), get(listener, axis));
            // The listener must be on the source's side of the wall.
            if (l - plane) * (s - plane) <= 0.0 {
                continue;
            }
            let image = with(source, axis, 2.0 * plane - s);
            let t = (plane - l) / (get(image, axis) - l);
            let point = listener + (image - listener) * t;
            // The point must be on a wall face: solid behind it.
            let behind = with(point, axis, plane + f64::from(sign) * NUDGE);
            let cell = [behind.x, behind.y, behind.z].map(|v| v.floor() as i32);
            let wall = world.block(cell[0], cell[1], cell[2]);
            if !wall.is_solid() {
                continue;
            }
            let front = with(point, axis, plane - f64::from(sign) * NUDGE);
            if transmission(world, listener, front)[1] < CLEAR
                || transmission(world, front, source)[1] < CLEAR
            {
                continue;
            }
            let way = (image - listener).length();
            let acoustics = wall.acoustics();
            let mirrored = (1.0 - acoustics.absorption[1]) * (1.0 - acoustics.scattering);
            let e = mirrored * (direct / way).powi(2) as f32;
            energy += e;
            first = first.min(((way - direct) / SPEED_OF_SOUND) as f32);
            sum = sum + (point - listener).normalized() * f64::from(e);
            count += 1;
        }
    }
    if count == 0 {
        return Early::NONE;
    }
    Early {
        energy,
        delay: first,
        direction: sum * (1.0 / f64::from(energy)),
        count,
    }
}

/// How far echoes are looked for, in blocks: as far as the world is copied.
pub const ECHO_REACH: i32 = 128;
/// Echoes up to this order: a ravine's walls send a shout back and forth.
const ECHO_ORDER: u32 = 4;
/// Reflections arriving sooner than this after the sound blend into the
/// reverb; only later ones are heard as echoes.
const ECHO_MIN_DELAY: f64 = 0.05;
/// A wall must be solid this far round the point a ray hits it to give a
/// clean echo, so a tree trunk or a pillar doesn't.
const WALL_SPAN: i32 = 2;

/// The echoes of a sound beside `listener` off the big walls along each axis
/// (cliffs, ravine sides, a cave's far end), up to [`ECHO_ORDER`] bounces
/// back and forth between two facing walls, from their image sources. Only
/// echoes later than [`ECHO_MIN_DELAY`] count, and first ones within
/// `traced` blocks are left out: the room trace already has those.
pub fn echoes(world: &World, listener: Vec3, traced: f64) -> Vec<Reflection> {
    let mut out = Vec::new();
    for axis in 0..3 {
        // The wall each way: its distance and how much of a reflection it
        // mirrors.
        let side = |sign: i32| -> Option<(f64, f32)> {
            let plane = wall_along_within(world, listener, axis, sign, ECHO_REACH)?;
            let material = wall_material(world, listener, axis, sign, plane)?;
            let a = material.acoustics();
            let distance = (plane - get(listener, axis)).abs();
            let behind = with(listener, axis, plane - f64::from(sign) * NUDGE);
            if transmission(world, listener, behind)[1] < CLEAR {
                return None;
            }
            Some((distance, (1.0 - a.absorption[1]) * (1.0 - a.scattering)))
        };
        let walls = [side(-1), side(1)];
        // Images of the listener: start off each wall, then alternate.
        for first in 0..2 {
            let Some((d, r)) = walls[first] else {
                continue;
            };
            let (mut position, mut energy, mut wall) = (2.0 * d, r, first);
            for order in 1..=ECHO_ORDER {
                let delay = position / SPEED_OF_SOUND;
                if delay >= ECHO_MIN_DELAY && (order > 1 || d > traced) {
                    let sign = if wall == 0 { -1.0 } else { 1.0 };
                    out.push(Reflection {
                        direction: with(Vec3::ZERO, axis, sign),
                        delay: delay as f32,
                        energy: energy / (position * position) as f32,
                    });
                }
                // Back across the room to the other wall, if there is one.
                wall = 1 - wall;
                let Some((d, r)) = walls[wall] else {
                    break;
                };
                position += 2.0 * d;
                energy *= r;
            }
        }
    }
    out
}

/// What the wall at `plane` along `axis` is made of, if it is a wall: solid
/// for [`WALL_SPAN`] blocks each way round where the axis meets it.
fn wall_material(world: &World, at: Vec3, axis: usize, sign: i32, plane: f64) -> Option<Material> {
    let hit = with(at, axis, plane + f64::from(sign) * NUDGE);
    let cell = [hit.x, hit.y, hit.z].map(|v| v.floor() as i32);
    let material = world.block(cell[0], cell[1], cell[2]);
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    for (du, dv) in [
        (-WALL_SPAN, 0),
        (WALL_SPAN, 0),
        (0, -WALL_SPAN),
        (0, WALL_SPAN),
    ] {
        let mut c = cell;
        c[u] += du;
        c[v] += dv;
        if !world.block(c[0], c[1], c[2]).is_solid() {
            return None;
        }
    }
    Some(material)
}

/// The nearest wall from `from` along `axis` in direction `sign`, within
/// [`REACH`]: the coordinate of its face.
fn wall_along(world: &World, from: Vec3, axis: usize, sign: i32) -> Option<f64> {
    wall_along_within(world, from, axis, sign, REACH)
}

fn wall_along_within(world: &World, from: Vec3, axis: usize, sign: i32, reach: i32) -> Option<f64> {
    let mut cell = [from.x, from.y, from.z].map(|v| v.floor() as i32);
    for _ in 0..reach {
        cell[axis] += sign;
        if world.block(cell[0], cell[1], cell[2]).is_solid() {
            let face = if sign > 0 { cell[axis] } else { cell[axis] + 1 };
            return Some(f64::from(face));
        }
    }
    None
}

fn get(v: Vec3, axis: usize) -> f64 {
    [v.x, v.y, v.z][axis]
}

fn with(v: Vec3, axis: usize, value: f64) -> Vec3 {
    let mut xyz = [v.x, v.y, v.z];
    xyz[axis] = value;
    Vec3::new(xyz[0], xyz[1], xyz[2])
}

#[cfg(test)]
mod tests {
    use earshot_world::Material;

    use super::*;

    /// A floor of `material` under y = 0, 40 blocks square.
    fn floor(material: Material) -> World {
        let mut world = World::new();
        for x in -20..20 {
            for z in -20..20 {
                world.set_block(x, -1, z, material);
            }
        }
        world
    }

    #[test]
    fn a_floor_gives_one_reflection_from_below_a_little_later() {
        let world = floor(Material::Rock);
        let (listener, source) = (Vec3::new(0.5, 1.5, 0.5), Vec3::new(8.5, 1.5, 0.5));
        let early = early_reflections(&world, listener, source);
        assert_eq!(early.count, 1, "{early:?}");
        // Image at y = -1.5: the way is sqrt(8² + 3²) = 8.54 against 8.
        let expected = ((8.0f64.powi(2) + 9.0).sqrt() - 8.0) / SPEED_OF_SOUND;
        assert!(
            (f64::from(early.delay) - expected).abs() < 1e-4,
            "{early:?}"
        );
        assert!(early.direction.y < -0.1, "from below: {early:?}");
        assert!(early.energy > 0.5 && early.energy < 1.0, "{early:?}");
    }

    #[test]
    fn soft_walls_reflect_less_and_open_air_not_at_all() {
        let (listener, source) = (Vec3::new(0.5, 1.5, 0.5), Vec3::new(8.5, 1.5, 0.5));
        let rock = early_reflections(&floor(Material::Rock), listener, source);
        let cloth = early_reflections(&floor(Material::Cloth), listener, source);
        assert!(cloth.energy < rock.energy / 2.0, "{cloth:?} vs {rock:?}");
        assert_eq!(
            early_reflections(&World::new(), listener, source),
            Early::NONE
        );
    }

    /// Two big rock walls facing across x, `width` blocks apart, the listener
    /// `left` blocks from the west one.
    fn ravine(width: i32, left: f64) -> (World, Vec3) {
        let mut world = World::new();
        for y in -10..10 {
            for z in -10..10 {
                world.set_block(-1, y, z, Material::Rock);
                world.set_block(width, y, z, Material::Rock);
            }
        }
        (world, Vec3::new(left, 0.5, 0.5))
    }

    #[test]
    fn a_ravine_sends_echoes_back_and_forth() {
        let (world, listener) = ravine(40, 10.0);
        let echoes = echoes(&world, listener, 0.0);
        let delays: Vec<f64> = echoes
            .iter()
            .map(|e| f64::from(e.delay) * SPEED_OF_SOUND)
            .collect();
        // Off the near wall (2 × 10 = 20 blocks there and back) and the far
        // one (60), then both (80), and on: 20 + 60 + 20 ...
        for expected in [20.0, 60.0, 80.0, 100.0] {
            assert!(
                delays.iter().any(|d| (d - expected).abs() < 0.01),
                "no echo at {expected}: {delays:?}"
            );
        }
        // Later echoes are fainter, and come from the wall they last left.
        let near = echoes
            .iter()
            .find(|e| (f64::from(e.delay) * SPEED_OF_SOUND - 20.0).abs() < 0.01)
            .unwrap();
        let far = echoes
            .iter()
            .find(|e| (f64::from(e.delay) * SPEED_OF_SOUND - 60.0).abs() < 0.01)
            .unwrap();
        assert!(far.energy < near.energy && near.direction.x < 0.0 && far.direction.x > 0.0);
    }

    #[test]
    fn a_pillar_or_a_close_wall_gives_no_echo() {
        let mut world = World::new();
        for y in -10..10 {
            world.set_block(30, y, 0, Material::Rock);
        }
        assert!(
            echoes(&world, Vec3::new(0.5, 0.5, 0.5), 0.0).is_empty(),
            "a pillar is no wall"
        );
        // A wall 5 blocks off answers within 50 ms: reverb, not an echo.
        let (world, listener) = ravine(200, 5.0);
        assert!(
            echoes(&world, listener, 0.0)
                .iter()
                .all(|e| e.delay >= 0.05)
        );
        // And one the room trace already reached isn't counted twice.
        let (world, listener) = ravine(200, 30.0);
        assert!(echoes(&world, listener, 64.0).is_empty());
    }

    #[test]
    fn a_wall_between_them_reflects_nothing_to_the_listener() {
        let mut world = World::new();
        for y in -5..5 {
            for z in -5..5 {
                world.set_block(4, y, z, Material::Rock);
            }
        }
        // The source's +x wall is between it and the listener's side.
        let early = early_reflections(&world, Vec3::new(8.5, 0.5, 0.5), Vec3::new(2.5, 0.5, 0.5));
        assert_eq!(early.count, 0, "{early:?}");
    }
}
