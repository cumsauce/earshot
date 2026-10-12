use earshot_world::{Vec3, World, transmission};

use crate::SPEED_OF_SOUND;

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

/// The nearest wall from `from` along `axis` in direction `sign`, within
/// [`REACH`]: the coordinate of its face.
fn wall_along(world: &World, from: Vec3, axis: usize, sign: i32) -> Option<f64> {
    let mut cell = [from.x, from.y, from.z].map(|v| v.floor() as i32);
    for _ in 0..REACH {
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
