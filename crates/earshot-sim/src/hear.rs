use earshot_world::{BANDS, Bands, Vec3, World, transmission};

use crate::Field;

/// How far around the source the extra occlusion rays go, in blocks, so a
/// block edge muffles gradually instead of switching on and off.
const SPREAD: f64 = 0.35;
/// What a right-angle bend round an edge costs per band, in dB: highs bend
/// less well than lows.
const BEND_LOSS_DB: Bands = [3.0, 6.0, 12.0];
/// Above this much of the mid band straight through, the way round can't do
/// better, so it isn't looked for.
const CLEAR: f32 = 0.9;

/// How a sound reaches the listener.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heard {
    /// The share of the energy that arrives, per band, besides distance.
    pub gains: Bands,
    /// Where it seems to come from: the source itself, or a point towards the
    /// opening it comes round through.
    pub heard_from: Vec3,
    /// How far it travels, in blocks.
    pub path_length: f64,
}

/// The share of energy per band that gets straight from `source` to
/// `listener`: the mean over `rays` rays, one to the source and the rest to
/// points [`SPREAD`] around it (4 or 8 of them, so `rays` is 1, 5 or 9; any
/// other count is taken as 5).
pub fn occlusion(world: &World, listener: Vec3, source: Vec3, rays: u32) -> Bands {
    let dir = (source - listener).normalized();
    if dir == Vec3::ZERO {
        return [1.0; BANDS];
    }
    if rays == 1 {
        return transmission(world, listener, source);
    }
    let helper = if dir.x.abs() < 0.9 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let u = cross(dir, helper).normalized() * SPREAD;
    let v = cross(dir, u).normalized() * SPREAD;
    let (a, b) = (
        (u + v) * std::f64::consts::FRAC_1_SQRT_2,
        (u - v) * std::f64::consts::FRAC_1_SQRT_2,
    );
    let offsets = [Vec3::ZERO, u, -u, v, -v, a, -a, b, -b];
    let offsets = if rays == 9 {
        &offsets[..]
    } else {
        &offsets[..5]
    };
    let mut sum = [0.0f32; BANDS];
    for &offset in offsets {
        let through = transmission(world, listener, source + offset);
        for (sum, through) in sum.iter_mut().zip(through) {
            *sum += through;
        }
    }
    sum.map(|s| s / offsets.len() as f32)
}

/// How a sound at `source` reaches `listener`: straight through whatever is in
/// the way (over `rays` occlusion rays), or round it along `field`'s shortest
/// path, whichever loses less in the mid band. Round, it seems to come from
/// the opening. With no rays, nothing is in the way.
pub fn hear(
    world: &World,
    field: Option<&Field>,
    listener: Vec3,
    source: Vec3,
    rays: u32,
) -> Heard {
    let path_length = (source - listener).length();
    if rays == 0 {
        return Heard {
            gains: [1.0; BANDS],
            heard_from: source,
            path_length,
        };
    }
    let direct = occlusion(world, listener, source, rays);
    let straight = Heard {
        gains: direct,
        heard_from: source,
        path_length,
    };
    if direct[1] >= CLEAR {
        return straight;
    }
    let Some(route) = field.and_then(|field| field.route(world, source)) else {
        return straight;
    };
    let around = bend_loss(route.bend);
    if around[1] <= direct[1] {
        return straight;
    }
    let mut gains = direct;
    for (gain, around) in gains.iter_mut().zip(around) {
        *gain = gain.max(around);
    }
    Heard {
        gains,
        heard_from: route.heard_from,
        path_length: route.length,
    }
}

/// What bending by `bend` radians costs, per band.
fn bend_loss(bend: f64) -> Bands {
    let right_angles = (bend / std::f64::consts::FRAC_PI_2) as f32;
    BEND_LOSS_DB.map(|db| 10f32.powf(-db * right_angles / 10.0))
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

#[cfg(test)]
mod tests {
    use earshot_world::Material;

    use super::*;

    const LISTENER: Vec3 = Vec3::new(0.5, 0.5, 0.5);
    const SOURCE: Vec3 = Vec3::new(9.5, 0.5, 0.5);

    /// Rock ground at y = -1, and a rock wall on it at x = 5, z from -10 to
    /// 10, y from 0 to 9 (too high to go over), with a doorway at z = `door`.
    fn wall(door: Option<i32>) -> World {
        let mut world = World::new();
        for x in -16..=16 {
            for z in -16..=16 {
                world.set_block(x, -1, z, Material::Rock);
            }
        }
        for y in 0..=9 {
            for z in -10..=10 {
                if door == Some(z) && (0..2).contains(&y) {
                    continue;
                }
                world.set_block(5, y, z, Material::Rock);
            }
        }
        world
    }

    #[test]
    fn nothing_in_the_way_is_heard_as_is() {
        let world = World::new();
        let heard = hear(&world, None, LISTENER, SOURCE, 5);
        assert_eq!(heard.gains, [1.0; BANDS]);
        assert_eq!(heard.heard_from, SOURCE);
    }

    #[test]
    fn a_wall_muffles_and_a_doorway_brings_the_sound_round() {
        // Straight through the wall.
        let through = hear(&wall(None), None, LISTENER, SOURCE, 5);
        assert!(through.gains[1] < 0.05, "{through:?}");

        let open = wall(Some(4));
        let field = Field::build(&open, LISTENER, 16);
        let round = hear(&open, Some(&field), LISTENER, SOURCE, 5);
        assert!(round.gains[1] > through.gains[1] * 5.0, "{round:?}");
        assert!(
            round.gains[2] < round.gains[0],
            "highs bend less: {round:?}"
        );
        assert!(
            round.heard_from.z > 1.5,
            "heard from the doorway's side: {round:?}"
        );
        assert!(round.path_length > 9.0);
    }

    #[test]
    fn half_hidden_sources_are_partly_muffled() {
        let mut world = World::new();
        // A wall that ends at z = 1, where the line of sight crosses it.
        for z in -10..=0 {
            for y in -2..=2 {
                world.set_block(5, y, z, Material::Rock);
            }
        }
        let edge = occlusion(&world, LISTENER, Vec3::new(9.5, 0.5, 1.4), 5);
        assert!(edge[1] > 0.1 && edge[1] < 0.95, "{edge:?}");
    }

    #[test]
    fn the_ray_count_sets_how_soft_edges_are_and_none_turns_occlusion_off() {
        let mut world = World::new();
        for z in -10..=0 {
            for y in -2..=2 {
                world.set_block(5, y, z, Material::Rock);
            }
        }
        let at = Vec3::new(9.5, 0.5, 1.4);
        // One ray clips the wall's end, through half a block of rock; nine
        // see past it too.
        let one = occlusion(&world, LISTENER, at, 1);
        let nine = occlusion(&world, LISTENER, at, 9);
        assert!(one[1] < 0.1, "{one:?}");
        assert!(nine[1] > 0.1 && nine[1] < 0.95, "{nine:?}");
        let off = hear(&wall(None), None, LISTENER, SOURCE, 0);
        assert_eq!(off.gains, [1.0; BANDS]);
        assert_eq!(off.heard_from, SOURCE);
    }

    #[test]
    fn bending_costs_more_the_further_it_turns() {
        let slight = bend_loss(0.3);
        let sharp = bend_loss(1.5);
        assert!(sharp[1] < slight[1]);
        assert_eq!(bend_loss(0.0), [1.0; BANDS]);
    }
}
