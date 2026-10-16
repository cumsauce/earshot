use earshot_world::{Material, Vec3, World, cast};

use crate::Rng;

/// Where a path leaves a surface, just off it, so the next leg doesn't hit
/// the same face again.
const SURFACE_OFFSET: f64 = 1e-4;
/// A path whose energy falls below this stops: it is 60 dB down.
const MIN_ENERGY: f32 = 1e-6;

/// One leg of a traced path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub from: Vec3,
    pub to: Vec3,
    /// Mid-band energy at the start of the leg: 1 at the listener, less
    /// after each reflection.
    pub energy: f32,
    /// Nothing within reach: the leg went off into open air.
    pub escaped: bool,
    /// What it ended on; air when it escaped.
    pub material: Material,
    /// How much of what it ended on is there: the share a crossing through
    /// it takes, which the room loses besides the absorption — but only
    /// where the way behind is open (`leaky`).
    pub fill: f32,
    /// Whether the block behind the hit face is open, so a crossing really
    /// leaves the room. A rug over rock keeps its room's bass.
    pub leaky: bool,
    /// Which leg of its path it is: 0 leaves the listener.
    pub bounce: u32,
    /// The outward normal of the face it ended on; zero when it escaped.
    pub normal: Vec3,
}

/// What a path trace found.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TraceStats {
    pub rays: u32,
    pub segments: u32,
    /// Rays whose very first leg met nothing: a measure of open sky.
    pub escaped_first: u32,
    /// Legs that met nothing.
    pub escaped: u32,
    /// Mean length of the legs that ended on a surface: about the size of
    /// the space.
    pub mean_free_path: f64,
}

/// `count` unit directions spread evenly over the sphere (a Fibonacci
/// lattice), turned by a rotation derived from `seed`: a fixed lattice
/// would line up with the same world axes every trace, so the same wall
/// would always get the same ray pattern. Turning it with the seed
/// decorrelates the pattern from the room, which cuts the variance of an
/// average over traces.
pub fn sphere_directions(count: u32, seed: u64) -> Vec<Vec3> {
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    // Splitmix64: three uniforms from the seed, no dependency on the sim's
    // integer RNG (which seeds itself differently).
    let mut h = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut next = move || {
        h = h.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = h;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    };
    // A random axis (by the uniform-in-square sphere map) and angle turn
    // the lattice as one rigid pattern — the balance survives.
    let (u, v) = (next(), next());
    let axis_z = 1.0 - 2.0 * u;
    let radius = (1.0 - axis_z * axis_z).max(0.0).sqrt();
    let angle = 2.0 * std::f64::consts::PI * v;
    let axis = Vec3::new(radius * angle.cos(), radius * angle.sin(), axis_z);
    let turn = 2.0 * std::f64::consts::PI * next();
    let (c, s) = (turn.cos(), turn.sin());
    (0..count)
        .map(|i| {
            let y = 1.0 - 2.0 * (f64::from(i) + 0.5) / f64::from(count);
            let radius = (1.0 - y * y).sqrt();
            let angle = golden * f64::from(i);
            let d = Vec3::new(angle.cos() * radius, y, angle.sin() * radius);
            // Rodrigues' rotation of `d` about `axis` by `turn`.
            let cross = Vec3::new(
                axis.y * d.z - axis.z * d.y,
                axis.z * d.x - axis.x * d.z,
                axis.x * d.y - axis.y * d.x,
            );
            let dot = axis.dot(d);
            axis * (dot * (1.0 - c)) + d * c + cross * s
        })
        .collect()
}

/// Traces `rays` paths from `origin`, evenly spread, with up to `bounces`
/// reflections each and at most `reach` blocks per leg, and hands every leg
/// to `segment`. A reflection follows Vorländer's directional scattering
/// model — specular with probability (1 - s), or a forward-scattered lobe
/// `cos(θ)^p` around the mirror direction with probability s — and loses
/// the surface's mid-band absorption.
pub fn trace_paths(
    world: &World,
    origin: Vec3,
    rays: u32,
    bounces: u32,
    reach: f64,
    seed: u64,
    segment: impl FnMut(Segment),
) -> TraceStats {
    trace_directions(
        world,
        origin,
        &sphere_directions(rays, seed),
        bounces,
        reach,
        seed,
        segment,
    )
}

/// [`trace_paths`], with the rays starting along `directions`: for splitting
/// one trace between threads.
pub fn trace_directions(
    world: &World,
    origin: Vec3,
    directions: &[Vec3],
    bounces: u32,
    reach: f64,
    seed: u64,
    mut segment: impl FnMut(Segment),
) -> TraceStats {
    let mut rng = Rng::new(seed);
    let mut stats = TraceStats {
        rays: directions.len() as u32,
        ..TraceStats::default()
    };
    let mut hit_lengths = 0.0;
    let mut hits = 0u32;
    for &start in directions {
        let (mut from, mut dir, mut energy) = (origin, start, 1.0f32);
        for leg in 0..=bounces {
            stats.segments += 1;
            let Some(hit) = cast(world, from, dir, reach) else {
                stats.escaped += 1;
                stats.escaped_first += u32::from(leg == 0);
                segment(Segment {
                    from,
                    to: from + dir * reach,
                    energy,
                    escaped: true,
                    material: Material::Air,
                    fill: 1.0,
                    leaky: true,
                    bounce: leg,
                    normal: Vec3::ZERO,
                });
                break;
            };
            // Whether the way behind the face is open, so a crossing really
            // leaves the room. A leaf-walled treehouse is nearly outdoors; a
            // rug over rock keeps its room's bass.
            let past = from + dir * (hit.exit + 1e-6);
            let leaky = !world
                .block(
                    past.x.floor() as i32,
                    past.y.floor() as i32,
                    past.z.floor() as i32,
                )
                .is_solid();
            segment(Segment {
                from,
                to: hit.point,
                energy,
                escaped: false,
                material: hit.material,
                fill: hit.fill,
                leaky,
                bounce: leg,
                normal: hit.normal,
            });
            hit_lengths += hit.distance;
            hits += 1;
            let acoustics = hit.material.acoustics();
            // The wall lets the ray through with its transmissivity — when
            // the way behind is open — and the ray leaves the room. What it
            // takes besides the absorption is already on the tally, so the
            // encounter's expected loss comes out right whatever the draw.
            let through = if leaky {
                acoustics.transmission[1] * hit.fill
            } else {
                0.0
            };
            if rng.next_f64() < f64::from(through) {
                break;
            }
            energy *= 1.0 - acoustics.absorption[1];
            // Starting inside a block gives no face to leave from.
            if hit.normal == Vec3::ZERO || energy < MIN_ENERGY {
                break;
            }
            dir = scatter(dir, hit.normal, acoustics.scattering, &mut rng);
            from = hit.point + hit.normal * SURFACE_OFFSET;
        }
    }
    if hits > 0 {
        stats.mean_free_path = hit_lengths / f64::from(hits);
    }
    stats
}

/// Vorländer's directional scattering BRDF: with probability (1 - s) the
/// reflection is specular, with probability s it scatters off a forward
/// lobe `cos(θ_axis)^p`. Both the lobe's axis and its sharpness vary
/// continuously with `s`:
///
/// - At `s = 0` the axis is the mirror direction and `p = 11` puts all the
///   lobe's mass on the mirror peak — but the Bernoulli short-circuits the
///   lobe entirely and returns the mirror direction.
/// - At `s = 1` the axis is the wall normal and `p = 1` makes the lobe
///   `cos(θ_n)` over the upper hemisphere, exactly a Lambertian. The
///   Bernoulli never short-circuits.
/// - In between the axis interpolates `mirror * (1 - s) + n * s` and
///   `p = 1 + 10 * (1 - s)` flattens the lobe, so a rough surface spreads
///   energy along the mirror–normal line and a hard one keeps it close to
///   the mirror.
///
/// The lobe sample is restricted to the wall's upper hemisphere by
/// rejection: when a sample dips below the wall plane, another pair of
/// uniforms is drawn. The rejection count varies with `s` and the angle of
/// incidence, but the direction returned is deterministic in the seed.
fn scatter(incoming: Vec3, normal: Vec3, s: f32, rng: &mut Rng) -> Vec3 {
    let mirror = incoming - normal * (2.0 * incoming.dot(normal));
    let s = s.clamp(0.0, 1.0);
    // Probability (1 - s): specular reflection.
    if rng.next_f64() >= f64::from(s) {
        return mirror;
    }
    // Lobe axis: mirror at s = 0, wall normal at s = 1. The two are
    // perpendicular for a flat wall, so the interpolation is well-defined.
    let axis = if s >= 1.0 {
        normal
    } else {
        (mirror * f64::from(1.0 - s) + normal * f64::from(s)).normalized()
    };
    // Sharpness: Lambertian (`p = 1`) at `s = 1`, forward spike (`p = 11`)
    // at `s = 0`.
    let p = 1.0 + 10.0 * (1.0 - s);
    let pp1 = f64::from(p) + 1.0;
    // Sample `cos^p(θ_axis)` via inverse CDF: U = 1 - cos(θ)^(p + 1) ->
    // cos(θ) = (1 - U)^(1 / (p + 1)).
    let helper = if axis.x.abs() < 0.9 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let tangent = cross(helper, axis).normalized();
    let bitangent = cross(axis, tangent);
    loop {
        let (u, v) = (rng.next_f64(), rng.next_f64());
        let cos_theta = (1.0 - u).powf(1.0 / pp1);
        let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
        let phi = std::f64::consts::TAU * v;
        let (x, y, z) = (sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta);
        let d = (tangent * x + bitangent * y + axis * z).normalized();
        if d.dot(normal) >= 0.0 {
            return d;
        }
    }
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

    /// A hollow rock box, inside from 0 to `size` on every axis.
    fn rock_box(size: i32) -> World {
        let mut world = World::new();
        for a in -1..=size {
            for b in -1..=size {
                for wall in [-1, size] {
                    world.set_block(wall, a, b, Material::Rock);
                    world.set_block(a, wall, b, Material::Rock);
                    world.set_block(a, b, wall, Material::Rock);
                }
            }
        }
        world
    }

    #[test]
    fn directions_are_unit_and_balanced() {
        let directions = sphere_directions(1000, 1);
        let mut sum = Vec3::ZERO;
        for d in &directions {
            assert!((d.length() - 1.0).abs() < 1e-9);
            sum = sum + *d;
        }
        assert!(sum.length() < 1.0, "{sum:?}");
    }

    /// The lattice turns with the seed, so two traces with different seeds
    /// sample different parts of the room: the same wall doesn't get the
    /// same ray pattern every measurement.
    #[test]
    fn a_seed_turns_the_lattice_and_stays_balanced() {
        let (a, b) = (sphere_directions(256, 1), sphere_directions(256, 2));
        assert!(
            a.iter().zip(&b).any(|(p, q)| (*p - *q).length() > 0.1),
            "different seeds gave the same lattice"
        );
        // And each is still balanced: the rotation can't have skewed it.
        for lattice in [&a, &b] {
            let sum: Vec3 = lattice.iter().copied().fold(Vec3::ZERO, |s, d| s + d);
            assert!(sum.length() < 1.0, "unbalanced lattice: {sum:?}");
        }
    }

    #[test]
    fn nothing_comes_back_in_the_open() {
        let stats = trace_paths(&World::new(), Vec3::ZERO, 64, 8, 32.0, 1, |s| {
            assert!(s.escaped)
        });
        assert_eq!(stats.escaped_first, 64);
        assert_eq!(stats.segments, 64);
    }

    #[test]
    fn a_closed_box_keeps_every_ray_and_sizes_itself() {
        let world = rock_box(10);
        let origin = Vec3::new(5.0, 5.0, 5.0);
        let mut legs = 0;
        let stats = trace_paths(&world, origin, 256, 8, 64.0, 7, |segment| {
            legs += 1;
            assert!(!segment.escaped);
            for p in [segment.from, segment.to] {
                for axis in 0..3 {
                    assert!((-1e-3..=10.0 + 1e-3).contains(&p.axis(axis)), "{p:?}");
                }
            }
        });
        assert_eq!(stats.escaped, 0);
        assert_eq!(legs, 256 * 9);
        // A cube's mean free path is 4V/S: two thirds of its side.
        assert!(
            (stats.mean_free_path - 10.0 * 2.0 / 3.0).abs() < 1.5,
            "{}",
            stats.mean_free_path
        );
    }

    /// A wall with `s = 0` only ever reflects the mirror direction.
    #[test]
    fn a_pure_specular_wall_always_mirrors() {
        let mut rng = Rng::new(13);
        let normal = Vec3::new(0.0, 1.0, 0.0);
        let incoming = Vec3::new(0.4, -1.0, 0.5).normalized();
        let mirror = incoming - normal * (2.0 * incoming.dot(normal));
        for _ in 0..1000 {
            let out = scatter(incoming, normal, 0.0, &mut rng);
            assert!((out - mirror).length() < 1e-9, "{out:?} vs {mirror:?}");
        }
    }

    /// A wall with any `s` always reflects into the upper hemisphere, never
    /// back into the wall: the lobe is rejection-sampled so the directional
    /// scatter stays above the surface.
    #[test]
    fn no_scattering_dips_below_the_wall() {
        let mut rng = Rng::new(17);
        let normal = Vec3::new(0.0, 1.0, 0.0);
        let incoming = Vec3::new(0.6, -1.0, 0.4).normalized();
        for s in [0.0_f32, 0.2, 0.5, 0.7, 1.0] {
            for _ in 0..2000 {
                let out = scatter(incoming, normal, s, &mut rng);
                assert!(
                    out.dot(normal) >= -1e-12,
                    "s={s}: {out:?} dipped below the wall"
                );
                assert!((out.length() - 1.0).abs() < 1e-9, "s={s}: {out:?}");
            }
        }
    }

    /// A wall with `s = 0.5` reflects the mirror direction about half the
    /// time — the Bernoulli step of the Vorländer mixture.
    #[test]
    fn half_scattered_wall_is_half_mirror() {
        let mut rng = Rng::new(19);
        let normal = Vec3::new(0.0, 1.0, 0.0);
        let incoming = Vec3::new(0.3, -1.0, 0.4).normalized();
        let mirror = incoming - normal * (2.0 * incoming.dot(normal));
        let mut mirrors = 0;
        let n = 4000;
        for _ in 0..n {
            let out = scatter(incoming, normal, 0.5, &mut rng);
            if (out - mirror).length() < 1e-9 {
                mirrors += 1;
            }
        }
        let fraction = mirrors as f64 / n as f64;
        assert!(
            (fraction - 0.5).abs() < 0.05,
            "got {fraction} mirror at s=0.5, expected ~0.5"
        );
    }

    /// A wall with `s = 0.3` has a sharp forward lobe (`p ≈ 8`): non-mirror
    /// samples concentrate near the mirror direction, not back toward the
    /// incoming ray. The mean dot product with the mirror axis is well above
    /// what an isotropic sphere sample around the wall normal would give.
    #[test]
    fn a_low_scattered_wall_scatters_near_the_mirror_direction() {
        let mut rng = Rng::new(23);
        let normal = Vec3::new(0.0, 1.0, 0.0);
        let incoming = Vec3::new(0.3, -1.0, 0.4).normalized();
        let mirror = incoming - normal * (2.0 * incoming.dot(normal));
        let mut sum_forward = 0.0;
        let n = 4000;
        for _ in 0..n {
            let out = scatter(incoming, normal, 0.3, &mut rng);
            sum_forward += out.dot(mirror);
        }
        let mean = sum_forward / n as f64;
        // An isotropic lobe over the upper hemisphere gives a mean dot
        // product with the mirror axis of around 0.25 (cosines 0°..90°
        // averaged against a Lambertian weight); a sharp `cos^8` lobe
        // pushes the mean well above that. Assert a directional bias.
        assert!(mean > 0.4, "lobe not forward enough at s=0.3: mean {mean}");
    }

    /// Determinism per seed: same inputs, same direction sequence. The
    /// rejection loop's draw count depends on the seeded uniforms, so the
    /// output is reproducible without preserving the exact draw count.
    #[test]
    fn scatter_is_deterministic_per_seed() {
        let mut a = Rng::new(101);
        let mut b = Rng::new(101);
        let normal = Vec3::new(0.0, 1.0, 0.0);
        let incoming = Vec3::new(0.3, -1.0, 0.4).normalized();
        for _ in 0..200 {
            let da = scatter(incoming, normal, 0.4, &mut a);
            let db = scatter(incoming, normal, 0.4, &mut b);
            assert_eq!(
                da, db,
                "Vorländer scattering must be deterministic per seed"
            );
        }
    }

    #[test]
    fn every_bounce_pays_its_surfaces_absorption() {
        let world = rock_box(10);
        // What one bounce leaves is the surface's share: walls that let rays
        // through take them out of the room at the tally, not off the weight.
        let survivor = 1.0 - Material::Rock.acoustics().absorption[1];
        trace_paths(
            &world,
            Vec3::new(5.0, 5.0, 5.0),
            64,
            12,
            64.0,
            11,
            |segment| {
                let expected = survivor.powi(segment.bounce as i32);
                assert!(
                    (segment.energy - expected).abs() < 1e-6 * expected,
                    "bounce {}: {} vs {expected}",
                    segment.bounce,
                    segment.energy
                );
            },
        );
    }

    #[test]
    fn no_bounces_leaves_one_leg_per_ray() {
        let world = rock_box(10);
        let mut legs = 0u32;
        let stats = trace_paths(
            &world,
            Vec3::new(5.0, 5.0, 5.0),
            128,
            0,
            64.0,
            5,
            |segment| {
                assert_eq!(segment.bounce, 0);
                assert!(!segment.escaped);
                legs += 1;
            },
        );
        assert_eq!(legs, 128);
        assert_eq!(stats.segments, 128);
        assert_eq!(stats.escaped, 0);
        assert_eq!(stats.escaped_first, 0);
    }

    #[test]
    fn legs_after_a_bounce_leave_the_surface_they_hit() {
        // A rock floor and nothing else: every leg past its first bounce
        // follows one off that floor, so it goes into the open half-space.
        let mut world = World::new();
        for x in -30..30 {
            for z in -30..30 {
                world.set_block(x, -1, z, Material::Rock);
            }
        }
        trace_paths(
            &world,
            Vec3::new(0.5, 4.5, 0.5),
            256,
            2,
            32.0,
            9,
            |segment| {
                if segment.bounce > 0 {
                    let dir = segment.to - segment.from;
                    assert!(dir.y > -1e-9, "went back into the floor: {segment:?}");
                }
            },
        );
    }
}
