use earshot_world::{BANDS, Bands, Vec3, World, transmission};

use crate::Field;

/// How far around the source the extra occlusion rays go, in blocks, so a
/// block edge muffles gradually instead of switching on and off.
const SPREAD: f64 = 0.35;
/// The bands' wavelengths, in blocks (metres): about 250 Hz, 1 kHz and 4 kHz.
const WAVELENGTH: Bands = [343.0 / 250.0, 343.0 / 1000.0, 343.0 / 4000.0];
/// In an enclosed space, sound gets round a corner mostly by reflecting off
/// the walls, as down a corridor: this much per right angle turned, per band.
const ENCLOSED_TURN_DB: Bands = [3.0, 6.0, 12.0];
/// The most diffraction round edges takes, in dB (ISO 9613-2's limit for a
/// double edge): past it, sound that far round is reverb, not a path.
const MAX_DIFFRACTION_DB: f32 = 25.0;
/// Grid steps' lengths, in blocks: through a face, an edge and a corner, as
/// the pathing field counts them.
const STEPS: [f64; 3] = [1.0, 1.4, 1.7];
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
    /// How big the opening it comes round through is, in square blocks
    /// ([`crate::MAX_APERTURE`] when it comes straight).
    pub aperture: f32,
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
/// the opening, and loses what [`round_loss`] says for how `enclosure`d the
/// listener's space is. With no rays, nothing is in the way.
pub fn hear(
    world: &World,
    field: Option<&Field>,
    listener: Vec3,
    source: Vec3,
    rays: u32,
    enclosure: f32,
) -> Heard {
    let path_length = (source - listener).length();
    if rays == 0 {
        return Heard {
            gains: [1.0; BANDS],
            heard_from: source,
            path_length,
            aperture: crate::MAX_APERTURE,
        };
    }
    let direct = occlusion(world, listener, source, rays);
    let straight = Heard {
        gains: direct,
        heard_from: source,
        path_length,
        aperture: crate::MAX_APERTURE,
    };
    if direct[1] >= CLEAR {
        return straight;
    }
    let Some(route) = field.and_then(|field| field.route(world, source)) else {
        return straight;
    };
    let around = round_loss(
        detour(route.length, listener, source),
        route.bend,
        enclosure,
    );
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
        aperture: route.aperture,
    }
}

/// How much farther than the straight line the way round goes, in blocks.
/// The field counts its way in grid steps, which run longer than a straight
/// line except along an axis; the way is scaled by how much longer the steps
/// would be with nothing in the way, so only the real detour remains.
fn detour(way: f64, listener: Vec3, source: Vec3) -> f64 {
    let straight = (source - listener).length();
    let [a, b, c] = {
        let d = source - listener;
        let mut d = [d.x.abs(), d.y.abs(), d.z.abs()];
        d.sort_by(|a, b| b.total_cmp(a));
        d
    };
    let steps = STEPS[2] * c + STEPS[1] * (b - c) + STEPS[0] * (a - b);
    let scale = if steps > 0.0 { straight / steps } else { 1.0 };
    (way * scale - straight).max(0.0)
}

/// What getting round costs, per band: in the open, diffraction over the
/// detour ([`diffraction`]); shut in, reflections round each turn
/// ([`ENCLOSED_TURN_DB`] per right angle of `bend`); in between, the two
/// blended in dB by `enclosure` (0 to 1).
fn round_loss(detour: f64, bend: f64, enclosure: f32) -> Bands {
    let enclosure = enclosure.clamp(0.0, 1.0);
    let right_angles = (bend / std::f64::consts::FRAC_PI_2) as f32;
    let open = diffraction(detour);
    std::array::from_fn(|band| {
        let open_db = -10.0 * open[band].log10();
        let enclosed_db = ENCLOSED_TURN_DB[band] * right_angles;
        let db = enclosure * enclosed_db + (1.0 - enclosure) * open_db;
        10f32.powf(-db / 10.0)
    })
}

/// What going round edges costs, per band, for a way `detour` blocks longer
/// than the straight line: ISO 9613-2's barrier attenuation (after Maekawa),
/// `10 log10(3 + 20 detour / wavelength)` dB, at most [`MAX_DIFFRACTION_DB`].
/// Lows, with their long waves, bend round far better than highs.
fn diffraction(detour: f64) -> Bands {
    WAVELENGTH.map(|wavelength| {
        let db = (10.0 * (3.0 + 20.0 * detour / f64::from(wavelength)).log10()) as f32;
        10f32.powf(-db.min(MAX_DIFFRACTION_DB) / 10.0)
    })
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
        let heard = hear(&world, None, LISTENER, SOURCE, 5, 1.0);
        assert_eq!(heard.gains, [1.0; BANDS]);
        assert_eq!(heard.heard_from, SOURCE);
    }

    #[test]
    fn a_wall_muffles_and_a_doorway_brings_the_sound_round() {
        // Straight through the wall.
        let through = hear(&wall(None), None, LISTENER, SOURCE, 5, 1.0);
        assert!(through.gains[1] < 0.05, "{through:?}");

        let open = wall(Some(4));
        let field = Field::build(&open, LISTENER, 16);
        let round = hear(&open, Some(&field), LISTENER, SOURCE, 5, 1.0);
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
        let off = hear(&wall(None), None, LISTENER, SOURCE, 0, 1.0);
        assert_eq!(off.gains, [1.0; BANDS]);
        assert_eq!(off.heard_from, SOURCE);
    }

    #[test]
    fn diffraction_follows_iso_9613() {
        // Grazing an edge (no detour) costs Maekawa's 4.8 dB in every band.
        let grazing = diffraction(0.0);
        assert!(
            grazing
                .iter()
                .all(|g| (10.0 * g.log10() + 4.77).abs() < 0.01),
            "{grazing:?}"
        );
        // A metre round: lows lose much less than highs.
        let metre = diffraction(1.0).map(|g| -10.0 * g.log10());
        assert!(
            (metre[0] - 12.45).abs() < 0.05 && (metre[1] - 17.87).abs() < 0.05,
            "{metre:?}"
        );
        assert!((metre[2] - 23.73).abs() < 0.05, "{metre:?}");
        // Far enough round, highs reach the cap.
        assert_eq!(-10.0 * diffraction(4.0)[2].log10(), MAX_DIFFRACTION_DB);
        // Farther round is never cheaper.
        assert!(diffraction(4.0)[0] < diffraction(1.0)[0]);
    }

    #[test]
    fn round_a_wall_in_the_open_loses_more_than_down_a_corridor() {
        let open = wall(Some(4));
        let field = Field::build(&open, LISTENER, 16);
        let outdoors = hear(&open, Some(&field), LISTENER, SOURCE, 5, 0.0);
        let indoors = hear(&open, Some(&field), LISTENER, SOURCE, 5, 1.0);
        assert!(
            outdoors.gains[1] < indoors.gains[1] / 2.0,
            "{outdoors:?} vs {indoors:?}"
        );
        assert!(
            outdoors.gains[2] < outdoors.gains[0],
            "highs bend less: {outdoors:?}"
        );
    }

    #[test]
    fn a_diagonal_way_with_nothing_in_it_is_no_detour() {
        let (listener, source) = (Vec3::new(0.5, 0.5, 0.5), Vec3::new(10.5, 4.5, 7.5));
        // As the field would count it: 4 corner steps, 3 edge steps, 3 face steps.
        let way = 4.0 * 1.7 + 3.0 * 1.4 + 3.0;
        assert!(detour(way, listener, source) < 1e-9);
        assert!(
            (detour(way + 2.0, listener, source) - 2.0 * (source - listener).length() / way).abs()
                < 1e-9
        );
    }
}
