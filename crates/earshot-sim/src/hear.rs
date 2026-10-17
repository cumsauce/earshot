use earshot_world::{BANDS, Bands, Vec3, World, flank_transmission};

use crate::Field;
use crate::ground::ground_gain;
use crate::quality::DiffractionStandard;

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
/// HJ 2.4-2021's cap on Maekawa's curve, in dB per band: a touch below
/// ISO's, since the two converge by the time the detour is that long.
const HJ24_MAX_DIFFRACTION_DB: f32 = 24.0;
/// The exponent in HJ 2.4's Maekawa fit: `db = 13·N^p`, with `p` chosen so
/// the curve lands near 20 dB at `N = 10` (Maekawa's published table).
const HJ24_POWER: f64 = 0.187;
/// Grid steps' lengths, in blocks: through a face, an edge and a corner, as
/// the pathing field counts them.
const STEPS: [f64; 3] = [1.0, 1.4, 1.7];
/// Above this much of every band straight through, the way round can't do
/// better, so it isn't looked for.
const CLEAR: f32 = 0.9;

/// How a sound reaches the listener.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heard {
    /// The share of the energy that arrives, per band, besides distance.
    /// Over open ground the direct share can rise above full: the
    /// ground's reflection meets the straight line and adds to it, up to
    /// the hard ground's double (see `crate::ground`).
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
        return flank_transmission(world, listener, source);
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
        let through = flank_transmission(world, listener, source + offset);
        for (sum, through) in sum.iter_mut().zip(through) {
            *sum += through;
        }
    }
    sum.map(|s| s / offsets.len() as f32)
}

/// How a sound at `source` reaches `listener`: straight through whatever is in
/// the way (over `rays` occlusion rays), or round it along `field`'s shortest
/// path. Each band takes the better of the two; the mid band decides where it
/// seems to come from, since that is where direction is judged. Round, it
/// loses what [`round_loss`] says for how `enclosure`d the listener's space is.
/// With no rays, nothing is in the way. The `diffraction` standard selects
/// which curve the way round an edge follows.
pub fn hear(
    world: &World,
    field: Option<&Field>,
    listener: Vec3,
    source: Vec3,
    rays: u32,
    enclosure: f32,
    diffraction: DiffractionStandard,
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
    // Outdoors the ground answers the sound that crosses it: its
    // reflection meets the straight line with a phase set by both
    // heights, which is exactly the excess attenuation the ground model
    // measures, so the direct bands take the ground's factor. The way
    // round is a different path with its own reflections, and is left
    // alone (docs/ground-reflection-design.md, "The fold").
    let direct = match ground_gain(world, listener, source, enclosure) {
        Some(factor) => std::array::from_fn(|band| direct[band] * factor[band]),
        None => direct,
    };
    let straight = Heard {
        gains: direct,
        heard_from: source,
        path_length,
        aperture: crate::MAX_APERTURE,
    };
    if direct.iter().all(|gain| *gain >= CLEAR) {
        return straight;
    }
    let Some(route) = field.and_then(|field| field.route(world, source)) else {
        return straight;
    };
    let extra = detour(route.length, listener, source);
    let around = round_loss(extra, route.bend, enclosure, diffraction);
    let round_is_better = around[1] > direct[1];
    // A route no longer than the straight run has gone round nothing, so
    // it is not a second path: its loss describes the same straight line
    // without the materials on it, and must not fill their bands back in.
    if !round_is_better && extra <= 0.0 {
        return straight;
    }
    // Every band takes the better of the two paths. A curtain, cloth or
    // leaves can pass the mid band while stopping the highs, and the way
    // round it is then the louder high band even when it loses in the mid.
    let mut gains = direct;
    for (gain, around) in gains.iter_mut().zip(around) {
        *gain = gain.max(around);
    }
    if !round_is_better {
        // The mid band carries where a sound seems to come from, so a
        // route that only wins in a muffled band does not move it.
        return Heard { gains, ..straight };
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
fn round_loss(detour: f64, bend: f64, enclosure: f32, standard: DiffractionStandard) -> Bands {
    let enclosure = enclosure.clamp(0.0, 1.0);
    let right_angles = (bend / std::f64::consts::FRAC_PI_2) as f32;
    let open = diffraction(detour, standard);
    std::array::from_fn(|band| {
        let open_db = -10.0 * open[band].log10();
        let enclosed_db = ENCLOSED_TURN_DB[band] * right_angles;
        let db = enclosure * enclosed_db + (1.0 - enclosure) * open_db;
        10f32.powf(-db / 10.0)
    })
}

/// What going round edges costs, per band, for a way `detour` blocks longer
/// than the straight line. With `standard = Iso9613_2`, ISO 9613-2's barrier
/// attenuation, `10 log10(3 + 20 detour / wavelength)` dB, at most
/// [`MAX_DIFFRACTION_DB`] (ISO's and GB/T 17247.2's caps sit there too).
/// With `standard = Hj24`, HJ 2.4-2021's Maekawa curve, fitted through the
/// tabulated points at `N = 1` and `N = 10`, capped at
/// [`HJ24_MAX_DIFFRACTION_DB`]. Lows, with their long waves, bend round far
/// better than highs.
fn diffraction(detour: f64, standard: DiffractionStandard) -> Bands {
    match standard {
        DiffractionStandard::Iso9613_2 => iso9613_diffraction(detour),
        DiffractionStandard::Hj24 => hj24_diffraction(detour),
    }
}

/// ISO 9613-2's barrier attenuation, per band, for a way `detour` blocks
/// longer than the straight line: `10 log10(3 + 20·detour/λ)` dB, at most
/// [`MAX_DIFFRACTION_DB`].
fn iso9613_diffraction(detour: f64) -> Bands {
    WAVELENGTH.map(|wavelength| {
        let db = (10.0 * (3.0 + 20.0 * detour / f64::from(wavelength)).log10()) as f32;
        10f32.powf(-db.min(MAX_DIFFRACTION_DB) / 10.0)
    })
}

/// HJ 2.4-2021's Maekawa curve, per band, for a way `detour` blocks longer
/// than the straight line: `min(24, 13·N^0.187)` dB where
/// `N = 2·detour/λ` is Maekawa's Fresnel number. The exponent is chosen so
/// `db(N = 1) = 13` and `db(N = 10) = 20` — Maekawa's published table.
/// No detour is no loss: zero, not Maekawa's 5 dB discontinuity, so the
/// curve joins smoothly onto straight-through sound.
fn hj24_diffraction(detour: f64) -> Bands {
    WAVELENGTH.map(|wavelength| {
        let n = 2.0 * detour / f64::from(wavelength);
        let db = if n <= 0.0 {
            0.0
        } else {
            (13.0 * n.powf(HJ24_POWER)).min(HJ24_MAX_DIFFRACTION_DB as f64)
        };
        10f32.powf(-(db as f32) / 10.0)
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
    use earshot_world::{Material, transmission};

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
        let heard = hear(
            &world,
            None,
            LISTENER,
            SOURCE,
            5,
            1.0,
            DiffractionStandard::Iso9613_2,
        );
        assert_eq!(heard.gains, [1.0; BANDS]);
        assert_eq!(heard.heard_from, SOURCE);
    }

    #[test]
    fn a_wall_muffles_and_a_doorway_brings_the_sound_round() {
        // Straight through the wall.
        let through = hear(
            &wall(None),
            None,
            LISTENER,
            SOURCE,
            5,
            1.0,
            DiffractionStandard::Iso9613_2,
        );
        assert!(through.gains[1] < 0.05, "{through:?}");

        let open = wall(Some(4));
        let field = Field::build(&open, LISTENER, 16);
        let round = hear(
            &open,
            Some(&field),
            LISTENER,
            SOURCE,
            5,
            1.0,
            DiffractionStandard::Iso9613_2,
        );
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
    fn a_wall_between_two_offset_people_leaks_along_it() {
        // A sealed wall with a listener and a source each touching one of
        // its faces and offset along it: the mass law all but silences a
        // line that spends its whole length inside the wall, but sound in
        // a structure conducts along it and comes out by the far face.
        let world = wall(None);
        let listener = Vec3::new(5.0, 0.5, 0.5);
        let source = Vec3::new(6.0, 0.5, 8.5);
        let heard = occlusion(&world, listener, source, 1);
        let mass_law = transmission(&world, listener, source);
        assert!(heard[1] > mass_law[1], "{heard:?} against {mass_law:?}");
        // Above an audibility floor, so the test proves the path is
        // present rather than merely non-zero.
        assert!(heard[1] > 1e-4, "{heard:?}");
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
    fn the_ground_lifts_grazing_sound_over_hard_ground() {
        // A rock floor at y = -1 under open sky: rock has no entry in the
        // flow-resistivity table, so it is hard ground and mirrors the
        // source outright.
        let mut world = World::new();
        for x in -40..40 {
            for z in -40..40 {
                world.set_block(x, -1, z, Material::Rock);
            }
        }
        let listener = Vec3::new(0.5, 0.5, 0.5);
        let near = Vec3::new(20.5, 0.5, 0.5);
        // Both ends a block above the floor, 20 blocks apart: the direct
        // and mirrored paths nearly agree in length, so over rigid ground
        // they add and the mid band lands above the free-field value.
        let free = occlusion(&world, listener, near, 5);
        let grazing = hear(
            &world,
            None,
            listener,
            near,
            5,
            0.0,
            DiffractionStandard::Iso9613_2,
        );
        assert!(grazing.gains[1] > free[1], "{grazing:?} against {free:?}");
        // Shut in, the same floor is the room's business and adds nothing.
        let shut_in = hear(
            &world,
            None,
            listener,
            near,
            5,
            1.0,
            DiffractionStandard::Iso9613_2,
        );
        assert_eq!(shut_in.gains, free, "{shut_in:?}");
        // A high source ten blocks up: the mirrored path is now half a
        // wavelength longer than the straight one at the mid band, the two
        // fight, and the ground takes sound away instead of adding it.
        let high = Vec3::new(20.5, 10.5, 0.5);
        let overhead = hear(
            &world,
            None,
            listener,
            high,
            5,
            0.0,
            DiffractionStandard::Iso9613_2,
        );
        assert!(overhead.gains[1] < free[1], "{overhead:?} against {free:?}");
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
        let off = hear(
            &wall(None),
            None,
            LISTENER,
            SOURCE,
            0,
            1.0,
            DiffractionStandard::Iso9613_2,
        );
        assert_eq!(off.gains, [1.0; BANDS]);
        assert_eq!(off.heard_from, SOURCE);
    }

    #[test]
    fn diffraction_follows_iso_9613() {
        // Grazing an edge (no detour) costs Maekawa's 4.8 dB in every band.
        let grazing = diffraction(0.0, DiffractionStandard::Iso9613_2);
        assert!(
            grazing
                .iter()
                .all(|g| (10.0 * g.log10() + 4.77).abs() < 0.01),
            "{grazing:?}"
        );
        // A metre round: lows lose much less than highs.
        let metre = diffraction(1.0, DiffractionStandard::Iso9613_2).map(|g| -10.0 * g.log10());
        assert!(
            (metre[0] - 12.45).abs() < 0.05 && (metre[1] - 17.87).abs() < 0.05,
            "{metre:?}"
        );
        assert!((metre[2] - 23.73).abs() < 0.05, "{metre:?}");
        // Far enough round, highs reach the cap.
        assert_eq!(
            -10.0 * diffraction(4.0, DiffractionStandard::Iso9613_2)[2].log10(),
            MAX_DIFFRACTION_DB
        );
        // Farther round is never cheaper.
        assert!(
            diffraction(4.0, DiffractionStandard::Iso9613_2)[0]
                < diffraction(1.0, DiffractionStandard::Iso9613_2)[0]
        );
    }

    #[test]
    fn diffraction_follows_hj24_maekawa() {
        use DiffractionStandard::Hj24;
        // No detour: zero loss in every band.
        let grazing = diffraction(0.0, Hj24).map(|g| -10.0 * g.log10());
        assert!(grazing.iter().all(|db| *db < 0.05), "{grazing:?}");
        // N = 1 means `detour = wavelength / 2`; for the low band
        // (wavelength = 1.372 blocks) that's 0.686 blocks. The HJ 2.4 /
        // Maekawa tabulated point at N = 1 is 13 dB. ISO 9613-2 gives the
        // same detour only 11 dB, so HJ 2.4 is ~2 dB steeper here (the
        // audible difference).
        let band1 = diffraction(f64::from(WAVELENGTH[0]) * 0.5, Hj24);
        let iso_band1 = diffraction(
            f64::from(WAVELENGTH[0]) * 0.5,
            DiffractionStandard::Iso9613_2,
        );
        let hj_db = -10.0 * band1[0].log10();
        let iso_db = -10.0 * iso_band1[0].log10();
        assert!(
            (hj_db - 13.0).abs() < 0.5,
            "{hj_db} vs 13.0 (ISO: {iso_db})"
        );
        assert!(
            hj_db > iso_db,
            "HJ 2.4 is steeper at N = 1: {hj_db} vs {iso_db}"
        );
        // N = 10 means `detour = 5 * wavelength`. Maekawa's table gives 20 dB.
        let big = diffraction(f64::from(WAVELENGTH[0]) * 5.0, Hj24);
        let big_db = -10.0 * big[0].log10();
        assert!((big_db - 20.0).abs() < 0.5, "{big_db} vs 20.0");
        // Far enough round, every band caps at HJ 2.4's 24 dB.
        let cap = diffraction(100.0, Hj24).map(|g| -10.0 * g.log10());
        assert!(
            cap.iter()
                .all(|db| (*db - HJ24_MAX_DIFFRACTION_DB).abs() < 0.05),
            "{cap:?}"
        );
        // The audible difference between the two curves: HJ 2.4 starts at
        // 0 dB at the edge (Maekawa's table has a discontinuity there that
        // this smooth fit ignores) and catches up to ISO once N grows. At
        // very small N — grazing — HJ 2.4 lets noticeably more sound
        // round than ISO's flat 4.77 dB offset.
        let almost_grazing_iso = diffraction(
            f64::from(WAVELENGTH[0]) * 0.001,
            DiffractionStandard::Iso9613_2,
        );
        let almost_grazing_hj = diffraction(f64::from(WAVELENGTH[0]) * 0.001, Hj24);
        let iso_grazing_db = -10.0 * almost_grazing_iso[0].log10();
        let hj_grazing_db = -10.0 * almost_grazing_hj[0].log10();
        assert!(
            hj_grazing_db < iso_grazing_db - 0.5,
            "at grazing HJ 2.4 should be gentler than ISO: {hj_grazing_db} vs {iso_grazing_db}"
        );
    }

    #[test]
    fn round_a_wall_in_the_open_loses_more_than_down_a_corridor() {
        let open = wall(Some(4));
        let field = Field::build(&open, LISTENER, 16);
        let outdoors = hear(
            &open,
            Some(&field),
            LISTENER,
            SOURCE,
            5,
            0.0,
            DiffractionStandard::Iso9613_2,
        );
        let indoors = hear(
            &open,
            Some(&field),
            LISTENER,
            SOURCE,
            5,
            1.0,
            DiffractionStandard::Iso9613_2,
        );
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

    #[test]
    fn gains_stay_between_silence_and_full_however_it_gets_there() {
        let mut rng = crate::Rng::new(0x9e37_79b9_7f4a_7c15);
        let mut world = World::new();
        for _ in 0..1200 {
            let at =
                [rng.next_f64(), rng.next_f64(), rng.next_f64()].map(|v| (v * 30.0 - 15.0) as i32);
            world.set_block(at[0], at[1], at[2], Material::Rock);
        }
        let listener = Vec3::new(0.5, 0.5, 0.5);
        world.set_block(0, 0, 0, Material::Air);
        let field = Field::build(&world, listener, 12);
        for _ in 0..400 {
            let source = Vec3::new(
                rng.next_f64() * 28.0 - 14.0,
                rng.next_f64() * 28.0 - 14.0,
                rng.next_f64() * 28.0 - 14.0,
            );
            for enclosure in [0.0f32, 0.5, 1.0] {
                let heard = hear(
                    &world,
                    Some(&field),
                    listener,
                    source,
                    5,
                    enclosure,
                    DiffractionStandard::Iso9613_2,
                );
                for gain in heard.gains {
                    // The direct bands carry the ground's factor outdoors,
                    // and hard ground's mirror can double them, so the
                    // bound is silence to that ceiling rather than to free
                    // field alone; the round path never exceeds full.
                    assert!((0.0..=2.0).contains(&gain), "{heard:?}");
                }
                assert!(
                    [heard.heard_from.x, heard.heard_from.y, heard.heard_from.z]
                        .iter()
                        .all(|v| v.is_finite()),
                    "{heard:?}"
                );
                assert!(
                    heard.path_length + 1e-9 >= (source - listener).length(),
                    "{heard:?}"
                );
                assert!(
                    (0.0..=crate::MAX_APERTURE).contains(&heard.aperture),
                    "{heard:?}"
                );
            }
        }
    }

    #[test]
    fn any_ray_count_but_one_and_nine_is_taken_as_five() {
        let world = wall(None);
        let at = Vec3::new(9.5, 0.5, 1.4);
        let five = occlusion(&world, LISTENER, at, 5);
        for rays in [0, 2, 3, 4, 6, 7, 8] {
            assert_eq!(occlusion(&world, LISTENER, at, rays), five, "{rays} rays");
        }
        // A source where the listener is: nothing in the way at all.
        assert_eq!(occlusion(&world, LISTENER, LISTENER, 5), [1.0; BANDS]);
        let coincident = hear(
            &world,
            None,
            LISTENER,
            LISTENER,
            5,
            1.0,
            DiffractionStandard::Iso9613_2,
        );
        assert_eq!(coincident.gains, [1.0; BANDS]);
        assert_eq!(coincident.heard_from, LISTENER);
    }

    /// Rock ground at y = -1 and a cloth curtain on it at x = 5, z from -10
    /// to 10, y from 0 to 9, with a two high doorway at z = `door`.
    fn curtain(door: i32) -> World {
        let mut world = World::new();
        for x in -16..=16 {
            for z in -16..=16 {
                world.set_block(x, -1, z, Material::Rock);
            }
        }
        for y in 0..=9 {
            for z in -10..=10 {
                if (0..2).contains(&y) && z == door {
                    continue;
                }
                world.set_block(5, y, z, Material::Cloth);
            }
        }
        world
    }

    #[test]
    fn the_high_band_hears_the_way_round_a_curtain() {
        // Cloth passes the mid band far better than the highs, and the hole
        // keeps the way round short. The high band should arrive that way
        // even though the mid band still comes straight through, and the
        // source's direction should stay put because the mid band prefers
        // the straight path.
        let world = curtain(-1);
        let field = Field::build(&world, LISTENER, 16);
        let enclosure = 0.5;
        let direct = occlusion(&world, LISTENER, SOURCE, 5);
        let direct = match ground_gain(&world, LISTENER, SOURCE, enclosure) {
            Some(factor) => std::array::from_fn(|band| direct[band] * factor[band]),
            None => direct,
        };
        let route = field.route(&world, SOURCE).unwrap();
        let around = round_loss(
            detour(route.length, LISTENER, SOURCE),
            route.bend,
            enclosure,
            DiffractionStandard::Iso9613_2,
        );
        // The scene's point: the curtain is hardest on the highs, and the
        // way round carries more high band than the direct line does.
        assert!(direct[1] > 5.0 * direct[2], "{direct:?}");
        assert!(around[2] > direct[2], "{direct:?} against {around:?}");
        let heard = hear(
            &world,
            Some(&field),
            LISTENER,
            SOURCE,
            5,
            enclosure,
            DiffractionStandard::Iso9613_2,
        );
        // No band may be left below a path that was available.
        for band in 0..BANDS {
            assert!(
                heard.gains[band] >= direct[band].max(around[band]) - 1e-6,
                "band {band}: {heard:?} against {direct:?} and {around:?}"
            );
        }
        assert_eq!(heard.heard_from, SOURCE, "{heard:?}");
    }

    #[test]
    fn a_clear_mid_band_does_not_stop_the_highs_coming_round() {
        // A single leaf block stops the highs more than the middles, and a
        // low source over hard ground lifts the mid band to nearly clear.
        // The mid gate alone would hide the way round the leaf, which is
        // louder in the high band.
        let mut world = World::new();
        for x in -50..=50 {
            for z in -50..=50 {
                world.set_block(x, -1, z, Material::Rock);
            }
        }
        world.set_block(5, 0, 0, Material::Leaves);
        let listener = Vec3::new(0.5, 2.5, 0.5);
        let source = Vec3::new(6.5, 0.5, 0.5);
        let field = Field::build(&world, listener, 16);
        let enclosure = 0.6;
        let direct = occlusion(&world, listener, source, 5);
        let direct = match ground_gain(&world, listener, source, enclosure) {
            Some(factor) => std::array::from_fn(|band| direct[band] * factor[band]),
            None => direct,
        };
        // The scene must be one the mid gate closes on, or it proves
        // nothing about the gate.
        assert!(direct[1] >= CLEAR, "{direct:?}");
        let route = field.route(&world, source).unwrap();
        let around = round_loss(
            detour(route.length, listener, source),
            route.bend,
            enclosure,
            DiffractionStandard::Iso9613_2,
        );
        assert!(around[2] > direct[2], "{direct:?} against {around:?}");
        let heard = hear(
            &world,
            Some(&field),
            listener,
            source,
            5,
            enclosure,
            DiffractionStandard::Iso9613_2,
        );
        // No band may be left below a path that was available.
        for band in 0..BANDS {
            assert!(
                heard.gains[band] >= direct[band].max(around[band]) - 1e-6,
                "band {band}: {heard:?} against {direct:?} and {around:?}"
            );
        }
    }

    #[test]
    fn going_round_costs_more_the_longer_and_sharper_the_way() {
        let quarter = std::f64::consts::FRAC_PI_4;
        for enclosure in [0.0f32, 0.5, 1.0] {
            let near = round_loss(0.5, quarter, enclosure, DiffractionStandard::Iso9613_2);
            let far = round_loss(4.0, quarter, enclosure, DiffractionStandard::Iso9613_2);
            let sharp = round_loss(
                0.5,
                std::f64::consts::FRAC_PI_2,
                enclosure,
                DiffractionStandard::Iso9613_2,
            );
            for band in 0..BANDS {
                assert!(far[band] <= near[band], "band {band}: {far:?} vs {near:?}");
                assert!(
                    sharp[band] <= near[band],
                    "band {band}: {sharp:?} vs {near:?}"
                );
            }
        }
    }
}
