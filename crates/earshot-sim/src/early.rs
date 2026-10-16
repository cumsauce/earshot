use earshot_world::{Material, Vec3, World, transmission};

use crate::{EdgeSource, Reflection, SPEED_OF_SOUND};

/// How far from the source a wall may be to give a first reflection, in
/// blocks.
const REACH: i32 = 16;
/// A leg of a reflection's way must let at least this much of the mid band
/// through to count.
const CLEAR: f32 = 0.5;
/// Reflections sooner than this after the direct sound, seconds, fuse with it
/// and are left to the room's reverb.
const FUSE: f32 = 0.005;
/// Steps a point this far off a wall face, to test the block either side.
const NUDGE: f64 = 0.01;
/// The narrowest gap an edge source is heard in: past it the edges of a doorway
/// fuse into a continuous leak.
const EDGE_FUSE_SECONDS: f64 = 0.001;
/// The most edge energy, summed across all edges of one parent reflection, per band:
/// a doorway with two clear vertical edges caps at 50% of the parent image source's
/// energy per band (Svensson's correction to the Svensson-edge sum).
const EDGE_TOTAL_CAP: f32 = 0.5;
/// The wavelengths used by the edge attenuation formula, in blocks (metres). About
/// 250 Hz, 1 kHz, 4 kHz at 343 m/s — same as `hear::WAVELENGTH`, but inlined here so
/// the formula doesn't depend on the hear module's privates.
const WAVELENGTH: [f64; 3] = [343.0 / 250.0, 343.0 / 1000.0, 343.0 / 4000.0];
/// The most edge attenuation, dB, applied per band (ISO 9613-2 cap).
const MAX_DIFFRACTION_DB: f32 = 25.0;

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
    let mut taps = early_taps(world, listener, source);
    // One copy of the sound a millisecond or two behind it (the ground under a
    // sound and its listener) only combs it, as a single delay line; the ear
    // fuses it with the direct sound, and the room's reverb has it.
    taps.retain(|t| t.delay >= FUSE);
    if taps.is_empty() {
        return Early::NONE;
    }
    let energy: f32 = taps.iter().map(|t| t.energy).sum();
    let sum = taps
        .iter()
        .fold(Vec3::ZERO, |sum, t| sum + t.direction * f64::from(t.energy));
    Early {
        energy,
        delay: taps.iter().map(|t| t.delay).fold(f32::INFINITY, f32::min),
        direction: sum * (1.0 / f64::from(energy)),
        count: taps.len() as u32,
    }
}

/// [`early_reflections`] one by one: each first reflection of a sound at
/// `source` heard at `listener`, from the point where it bounces, as late
/// as its way is longer than the direct one, with its energy relative to
/// the direct sound in the open.
pub fn early_taps(world: &World, listener: Vec3, source: Vec3) -> Vec<Reflection> {
    let direct = (source - listener).length().max(0.5);
    let mut taps = Vec::new();
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
            taps.push(Reflection {
                direction: (point - listener).normalized(),
                delay: ((way - direct) / SPEED_OF_SOUND) as f32,
                energy: mirrored * (direct / way).powi(2) as f32,
            });
        }
    }
    taps
}

/// For each first-order reflection in `taps`, gather up to 4 secondary sources at the
/// wall block's vertical edges (the doorway's doorframe). Each edge source is per-band,
/// with energy attenuated by the ISO 9613-2 excess-length formula (capped at 25 dB) and
/// weighted by the projection of the incoming and outgoing rays onto the wall's face
/// plane. The summed edge energy per band is capped at 50% of the parent image source's
/// energy (Svensson's edge sum).
pub fn edge_sources(
    world: &World,
    listener: Vec3,
    source: Vec3,
    taps: &[Reflection],
) -> Vec<EdgeSource> {
    let direct = (source - listener).length().max(0.5);
    let mut edges = Vec::new();
    for axis in 0..3 {
        for sign in [-1i32, 1] {
            let Some(plane) = wall_along(world, source, axis, sign) else {
                continue;
            };
            let (s, l) = (get(source, axis), get(listener, axis));
            if (l - plane) * (s - plane) <= 0.0 {
                continue;
            }
            let image = with(source, axis, 2.0 * plane - s);
            let t = (plane - l) / (get(image, axis) - l);
            let point = listener + (image - listener) * t;
            let behind = with(point, axis, plane + f64::from(sign) * NUDGE);
            let cell = [behind.x, behind.y, behind.z].map(|v| v.floor() as i32);
            let wall = world.block(cell[0], cell[1], cell[2]);
            if !wall.is_solid() {
                continue;
            }
            // The face normal points away from the wall (towards the listener).
            let mut normal = [0.0f64; 3];
            normal[axis] = -f64::from(sign);
            let normal = Vec3::new(normal[0], normal[1], normal[2]);
            let acoustics = wall.acoustics();
            // The wall block's 4 vertical edges: the corners of the block at the face
            // plane. For a face on the -x side (axis=0, sign=+1), the corners are at
            // x = behind.x, y ∈ {cell[1], cell[1]+1}, z ∈ {cell[2], cell[2]+1}. The
            // wall's x is `behind`; only the y and z corners vary.
            let cy = cell[1] as f64;
            let cz = cell[2] as f64;
            let corners_y = [cy, cy + 1.0];
            let corners_z = [cz, cz + 1.0];
            for &ey in &corners_y {
                for &ez in &corners_z {
                    let edge = with(behind, 1, ey);
                    let edge = with(edge, 2, ez);
                    // The line from listener to edge, and edge to source, must be clear.
                    if transmission(world, listener, edge)[1] < CLEAR {
                        continue;
                    }
                    if transmission(world, edge, source)[1] < CLEAR {
                        continue;
                    }
                    let two_way = (listener - edge).length() + (source - edge).length();
                    let detour = (two_way - direct).max(0.0);
                    // Per-band attenuation by the ISO 9613-2 excess-length formula.
                    let mut energy = [0.0f32; 3];
                    for b in 0..3 {
                        let wavelength = WAVELENGTH[b];
                        let db = (10.0 * (3.0 + 20.0 * detour / wavelength).log10()) as f32;
                        let db = db.min(MAX_DIFFRACTION_DB);
                        let factor = 10f32.powf(-db / 10.0);
                        // Cosines from the wall face: the projection of the propagation
                        // direction onto the face plane. For a listener directly in
                        // front of the wall, both cosines are 1 and the edge contributes
                        // maximally; at 90° (parallel to the wall), both are 0 and the
                        // edge contributes nothing.
                        let in_dir = (listener - edge).normalized();
                        let out_dir = (source - edge).normalized();
                        let cos_in = (in_dir.dot(normal)).abs() as f32;
                        let cos_out = (out_dir.dot(normal)).abs() as f32;
                        // 0.5 = half the energy goes around each side of the edge.
                        energy[b] =
                            0.5 * (1.0 - acoustics.absorption[1]) * cos_in * cos_out * factor;
                    }
                    edges.push(EdgeSource {
                        direction: (edge - listener).normalized(),
                        delay: (two_way / SPEED_OF_SOUND) as f32,
                        energy,
                    });
                }
            }
            // Also include the parent reflection's image source as an upper bound on
            // summed edge energy per band. Cap below.
            let _ = (point, image, acoustics);
        }
    }
    // Drop edge sources that fuse with the direct sound: first, so edges that
    // are dropped don't count towards the cap below and shrink the ones that
    // stay.
    edges.retain(|e| e.delay >= EDGE_FUSE_SECONDS as f32);
    // Cap the total edge energy per band at EDGE_TOTAL_CAP of the parent's image-source
    // energy. We use the parent's energy as a proxy: each parent reflection's mid-band
    // energy is the wall's mirrored share; the edge sum is a fraction of that. With no
    // parent taps there is no share to take a fraction of, so the edges keep what they
    // computed: scale against a full energy's worth instead of zero.
    for b in 0..3 {
        let total: f32 = edges.iter().map(|e| e.energy[b]).sum();
        let parent: f32 = taps.iter().map(|t| t.energy).sum();
        let parent = if parent > 0.0 { parent } else { 1.0 };
        if total > parent * EDGE_TOTAL_CAP && total > 0.0 {
            let scale = parent * EDGE_TOTAL_CAP / total;
            for e in &mut edges {
                e.energy[b] *= scale;
            }
        }
    }
    edges
}

/// How far echoes are looked for, in blocks: as far as the world is copied.
pub const ECHO_REACH: i32 = 128;
/// Echoes up to this order: a ravine's walls send a shout back and forth.
const ECHO_ORDER: u32 = 4;
/// Reflections arriving sooner than this after the sound fuse with it.
const FUSE_SECONDS: f64 = 0.005;
/// The narrowest gap a periodic series is heard in: past it the taps of a
/// back-and-forth between facing walls are distinct — flutter from about
/// 5 to 50 ms (a corridor 1 to 8 blocks across), then echoes.
const MIN_TAP_DELAY: f64 = FUSE_SECONDS;
/// A wall must be solid this far round the point a ray hits it to give a
/// clean echo, so a tree trunk or a pillar doesn't.
const WALL_SPAN: i32 = 2;

/// The echoes of a sound beside `listener` off the big walls along each axis
/// (cliffs, ravine sides, a cave's far end), up to [`ECHO_ORDER`] bounces
/// back and forth between two facing walls, from their image sources. The
/// same series read in the 5–50 ms window is flutter: a mineshaft corridor's
/// walls ring, a ravine's answer. Taps sooner than [`MIN_TAP_DELAY`] count
/// as nothing, and first ones within `traced` blocks are left out: the room
/// trace already has those.
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
        // Images of the listener: start off each wall, then alternate. The
        // first off a wall the trace already reached is the trace's own
        // first reflection; the ones after it are the corridor's.
        for first in 0..2 {
            let Some((d, r)) = walls[first] else {
                continue;
            };
            let (mut position, mut energy, mut wall) = (2.0 * d, r, first);
            for order in 1..=ECHO_ORDER {
                let delay = position / SPEED_OF_SOUND;
                if delay >= MIN_TAP_DELAY && (order > 1 || d > traced) {
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
        let (listener, source) = (Vec3::new(0.5, 5.5, 0.5), Vec3::new(8.5, 5.5, 0.5));
        let early = early_reflections(&world, listener, source);
        assert_eq!(early.count, 1, "{early:?}");
        // Image at y = -5.5: the way is sqrt(8² + 11²) = 13.6 against 8.
        let expected = ((8.0f64.powi(2) + 121.0).sqrt() - 8.0) / SPEED_OF_SOUND;
        assert!(
            (f64::from(early.delay) - expected).abs() < 1e-4,
            "{early:?}"
        );
        assert!(early.direction.y < -0.1, "from below: {early:?}");
        // Rough rock scatters 60 % of it, and the longer way loses some more.
        assert!(early.energy > 0.08 && early.energy < 0.4, "{early:?}");
        // Close above the ground, it comes too soon to be told apart.
        let low = early_reflections(&world, Vec3::new(0.5, 1.5, 0.5), Vec3::new(8.5, 1.5, 0.5));
        assert_eq!(low, Early::NONE);
    }

    #[test]
    fn soft_walls_reflect_less_and_open_air_not_at_all() {
        let (listener, source) = (Vec3::new(0.5, 5.5, 0.5), Vec3::new(8.5, 5.5, 0.5));
        let rock = early_reflections(&floor(Material::Rock), listener, source);
        let cloth = early_reflections(&floor(Material::Cloth), listener, source);
        assert!(cloth.energy < 0.7 * rock.energy, "{cloth:?} vs {rock:?}");
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
    fn a_parallel_wall_corridor_rings_in_flutter() {
        // A corridor 5 blocks across (faces at x = 0 and x = 4), the
        // listener 1.5 and 2.5 from them. The back-and-forth: 3 blocks
        // round trip is the trace's own first reflection; the rest arrive
        // as flutter at 8, 11 and 16 blocks.
        let mut world = World::new();
        for y in -6..8 {
            for z in -8..8 {
                world.set_block(-1, y, z, Material::Wood);
                world.set_block(4, y, z, Material::Wood);
            }
        }
        let listener = Vec3::new(1.5, 0.5, 0.5);
        let taps = echoes(&world, listener, 64.0);
        let delays: Vec<f64> = taps
            .iter()
            .map(|e| f64::from(e.delay) * SPEED_OF_SOUND)
            .collect();
        for expected in [8.0, 11.0, 16.0] {
            assert!(
                delays.iter().any(|d| (d - expected).abs() < 0.5),
                "no flutter tap at {expected} blocks: {delays:?}"
            );
        }
        assert!(
            taps.iter().all(|e| f64::from(e.delay) < 0.12),
            "a corridor flutters, it doesn't echo: {delays:?}"
        );
        // And rough stone scatters most of the ring away: its mirrored
        // share is (1−a)(1−s) with scattering 0.6 against wood's 0.35, so
        // the same corridor in rock flutters well under half as loud.
        let mut stone = World::new();
        for y in -6..8 {
            for z in -8..8 {
                stone.set_block(-1, y, z, Material::Rock);
                stone.set_block(4, y, z, Material::Rock);
            }
        }
        let energy = |list: &[Reflection]| list.iter().map(|t| t.energy).sum::<f32>();
        let rock = energy(&echoes(&stone, listener, 64.0));
        assert!(
            rock < energy(&taps) / 2.0,
            "rock {rock} vs wood {}",
            energy(&taps)
        );
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
        // A wall 5 blocks off answers within 50 ms — as flutter now, when
        // nothing else has covered it (the trace reached nowhere).
        let (world, listener) = ravine(200, 5.0);
        let untraced = echoes(&world, listener, 0.0);
        assert!(
            untraced.iter().any(|e| f64::from(e.delay) < 0.05),
            "{untraced:?}"
        );
        // But the room trace's own first reflections stand in for it: traced
        // that far, nothing is added a second time.
        let (world, listener) = ravine(200, 30.0);
        assert!(echoes(&world, listener, 64.0).is_empty());
        let (world, listener) = ravine(200, 5.0);
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

    #[test]
    fn a_floors_reflection_carries_its_image_sources_energy() {
        let world = floor(Material::Rock);
        let (listener, source) = (Vec3::new(0.5, 5.5, 0.5), Vec3::new(8.5, 5.5, 0.5));
        let taps = early_taps(&world, listener, source);
        assert_eq!(taps.len(), 1, "{taps:?}");
        let acoustics = Material::Rock.acoustics();
        let mirrored = (1.0 - acoustics.absorption[1]) * (1.0 - acoustics.scattering);
        // The image source is at y = -5.5: its way is sqrt(8² + 11²) = 13.6
        // against the direct 8.
        let way = (8.0f64.powi(2) + 11.0f64.powi(2)).sqrt();
        let expected = (f64::from(mirrored) * (8.0 / way).powi(2)) as f32;
        assert!(
            (taps[0].energy - expected).abs() < 1e-3 * expected,
            "{} vs {expected}",
            taps[0].energy
        );
        // From the point it bounces at, which lies between the two: down and
        // towards the source.
        assert!(
            taps[0].direction.y < -0.5 && taps[0].direction.x > 0.0,
            "from below: {taps:?}"
        );
    }

    #[test]
    fn a_floor_and_ceiling_answer_from_either_side() {
        let mut world = floor(Material::Rock);
        for x in -20..20 {
            for z in -20..20 {
                world.set_block(x, 9, z, Material::Rock);
            }
        }
        let (listener, source) = (Vec3::new(0.5, 3.5, 0.5), Vec3::new(8.5, 3.5, 0.5));
        let early = early_reflections(&world, listener, source);
        assert_eq!(early.count, 2, "{early:?}");
        // One from below and one from above, mostly cancelling: the listener
        // is nearer the floor, so it answers first.
        assert!(early.direction.y.abs() < 0.3, "{early:?}");
        assert!(
            early.direction.y < 0.0,
            "the floor is the stronger: {early:?}"
        );
        let down = ((8.0f64.powi(2) + 7.0f64.powi(2)).sqrt() - 8.0) / SPEED_OF_SOUND;
        assert!((f64::from(early.delay) - down).abs() < 1e-4, "{early:?}");
    }

    #[test]
    fn each_echo_carries_its_walls_share_over_its_distance_squared() {
        let (world, listener) = ravine(40, 10.0);
        let acoustics = Material::Rock.acoustics();
        let mirrored = (1.0 - acoustics.absorption[1]) * (1.0 - acoustics.scattering);
        let echoes = echoes(&world, listener, 0.0);
        let at = |blocks: f64| {
            echoes
                .iter()
                .find(|e| (f64::from(e.delay) * SPEED_OF_SOUND - blocks).abs() < 0.01)
                .unwrap_or_else(|| panic!("no echo at {blocks} blocks: {echoes:?}"))
        };
        // Off the near wall, the far one, and both in a row.
        assert!((at(20.0).energy - mirrored / 400.0).abs() < 1e-6);
        assert!((at(60.0).energy - mirrored / 3600.0).abs() < 1e-6);
        assert!((at(80.0).energy - mirrored * mirrored / 6400.0).abs() < 1e-7);
    }

    #[test]
    fn soft_walls_answer_more_faintly_than_hard_ones() {
        let mut cloth = World::new();
        for y in -10..10 {
            for z in -10..10 {
                cloth.set_block(-1, y, z, Material::Cloth);
                cloth.set_block(40, y, z, Material::Cloth);
            }
        }
        let listener = Vec3::new(10.0, 0.5, 0.5);
        let soft = echoes(&cloth, listener, 0.0);
        let (rock, listener) = ravine(40, 10.0);
        let hard = echoes(&rock, listener, 0.0);
        let first =
            |list: &[Reflection]| list.iter().map(|e| e.energy).fold(f32::INFINITY, f32::min);
        assert!(first(&soft) < 0.7 * first(&hard), "{soft:?} vs {hard:?}");
    }

    #[test]
    fn echoes_are_looked_for_as_far_as_the_world_is_copied() {
        let wall_at = |x: i32| {
            let mut world = World::new();
            for y in -10..10 {
                for z in -10..10 {
                    world.set_block(x, y, z, Material::Rock);
                }
            }
            world
        };
        // ECHO_REACH is 128: a wall there answers, one farther doesn't.
        assert!(!echoes(&wall_at(128), Vec3::new(0.5, 0.5, 0.5), 0.0).is_empty());
        assert!(echoes(&wall_at(129), Vec3::new(0.5, 0.5, 0.5), 0.0).is_empty());
    }

    /// A 3×3 rock wall in the YZ plane at x=4, with a 1×1 doorway in the middle
    /// (y=1, z=0). The source sits at (2, 0.5, 0.5), so the wall is behind it
    /// (in the +x direction); the doorway's two upper vertical edges are visible
    /// to a listener at (-2, 0.5, 0.5) and leak high-band energy.
    fn doorway_world() -> World {
        let mut world = World::new();
        for y in 0..=2 {
            for z in -1..=1 {
                if y == 1 && z == 0 {
                    continue;
                } // doorway at y=1, z=0
                world.set_block(4, y, z, Material::Rock);
            }
        }
        world
    }

    #[test]
    fn a_doorway_has_high_band_edge_diffraction() {
        let world = doorway_world();
        let listener = Vec3::new(-2.0, 0.5, 0.5);
        let source = Vec3::new(2.0, 0.5, 0.5);
        let taps = early_taps(&world, listener, source);
        let edges = edge_sources(&world, listener, source, &taps);
        let high: f32 = edges.iter().map(|e| e.energy[2]).sum();
        assert!(
            high > 0.0,
            "the doorway's edges leak high-band energy: {edges:?}"
        );
    }

    #[test]
    fn edge_sources_are_capped_at_half_the_image_source() {
        // A wider doorway: 5-tall wall at x=4, with a 3-block opening in the
        // middle (y ∈ {0, 1, 2} at z=0). The 4 visible edges sum to at most
        // 50% of the parent reflection's energy per band.
        let mut world = World::new();
        for y in 0..=4 {
            for z in -1..=1 {
                if (1..=3).contains(&y) {
                    continue;
                } // 3-block opening
                world.set_block(4, y, z, Material::Rock);
            }
        }
        let listener = Vec3::new(-2.0, 0.5, 0.5);
        let source = Vec3::new(2.0, 0.5, 0.5);
        let taps = early_taps(&world, listener, source);
        let edges = edge_sources(&world, listener, source, &taps);
        for b in 0..3 {
            let total: f32 = edges.iter().map(|e| e.energy[b]).sum();
            let parent: f32 = taps.iter().map(|t| t.energy).sum();
            assert!(
                total <= parent * EDGE_TOTAL_CAP + 1e-6,
                "band {b}: edge sum {total} exceeds 50% of parent {parent}"
            );
        }
    }

    #[test]
    fn edges_keep_their_energy_when_no_parent_taps_are_given() {
        // The impulse-response path asks for edges with no taps at all: the
        // parent sum is 0, and the edges must keep what they computed
        // instead of being scaled to nothing.
        let world = doorway_world();
        let listener = Vec3::new(-2.0, 0.5, 0.5);
        let source = Vec3::new(2.0, 0.5, 0.5);
        let edges = edge_sources(&world, listener, source, &[]);
        assert!(!edges.is_empty(), "{edges:?}");
        for e in &edges {
            let band: f32 = e.energy.iter().sum();
            assert!(band > 0.0, "an edge scaled to nothing: {edges:?}");
        }
    }

    #[test]
    fn edges_under_the_fuse_do_not_shrink_the_ones_that_stay() {
        // The same doorway heard from close by: the upper edge of the opening
        // is within the fuse and dropped, so it must not count towards the cap
        // on the edges that stay.
        let world = doorway_world();
        let listener = Vec3::new(3.9, 0.95, 0.05);
        let source = Vec3::new(3.9, 0.9, 0.05);
        // With no taps the cap is half a full energy's worth: the edges' own
        // sum is under that, so they come back as computed.
        let raw = edge_sources(&world, listener, source, &[]);
        assert!(!raw.is_empty(), "{raw:?}");
        let raw_sum: f32 = raw.iter().map(|e| e.energy[1]).sum();
        assert!(raw_sum < EDGE_TOTAL_CAP, "{raw:?}");
        // Taps whose energy puts the cap between the staying edges' sum and
        // theirs plus the dropped one's: counting only the staying edges
        // leaves them alone, counting the dropped one too would shrink them.
        let taps = [Reflection {
            direction: Vec3::ZERO,
            delay: 0.0,
            energy: 4.0 * raw_sum,
        }];
        let edges = edge_sources(&world, listener, source, &taps);
        assert!(
            edges
                .iter()
                .all(|e| f64::from(e.delay) >= EDGE_FUSE_SECONDS),
            "{edges:?}"
        );
        let sum: f32 = edges.iter().map(|e| e.energy[1]).sum();
        assert!(
            (sum - raw_sum).abs() <= 1e-6 * raw_sum,
            "{sum} vs {raw_sum}: {edges:?}"
        );
    }
}
