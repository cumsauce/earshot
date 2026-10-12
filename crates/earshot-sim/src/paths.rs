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
    /// Which leg of its path it is: 0 leaves the listener.
    pub bounce: u32,
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
/// lattice).
pub fn sphere_directions(count: u32) -> Vec<Vec3> {
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    (0..count)
        .map(|i| {
            let y = 1.0 - 2.0 * (f64::from(i) + 0.5) / f64::from(count);
            let radius = (1.0 - y * y).sqrt();
            let angle = golden * f64::from(i);
            Vec3::new(angle.cos() * radius, y, angle.sin() * radius)
        })
        .collect()
}

/// Traces `rays` paths from `origin`, evenly spread, with up to `bounces`
/// reflections each and at most `reach` blocks per leg, and hands every leg
/// to `segment`. A reflection is mirror-like, or with the surface's
/// scattering probability diffuse (cosine-weighted), and loses the surface's
/// mid-band absorption.
pub fn trace_paths(
    world: &World,
    origin: Vec3,
    rays: u32,
    bounces: u32,
    reach: f64,
    seed: u64,
    mut segment: impl FnMut(Segment),
) -> TraceStats {
    let mut rng = Rng::new(seed);
    let mut stats = TraceStats {
        rays,
        ..TraceStats::default()
    };
    let mut hit_lengths = 0.0;
    let mut hits = 0u32;
    for start in sphere_directions(rays) {
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
                    bounce: leg,
                });
                break;
            };
            segment(Segment {
                from,
                to: hit.point,
                energy,
                escaped: false,
                material: hit.material,
                bounce: leg,
            });
            hit_lengths += hit.distance;
            hits += 1;
            let acoustics = hit.material.acoustics();
            energy *= 1.0 - acoustics.absorption[1];
            // Starting inside a block gives no face to leave from.
            if hit.normal == Vec3::ZERO || energy < MIN_ENERGY {
                break;
            }
            dir = if rng.next_f64() < f64::from(acoustics.scattering) {
                diffuse(hit.normal, &mut rng)
            } else {
                dir - hit.normal * (2.0 * dir.dot(hit.normal))
            };
            from = hit.point + hit.normal * SURFACE_OFFSET;
        }
    }
    if hits > 0 {
        stats.mean_free_path = hit_lengths / f64::from(hits);
    }
    stats
}

/// A cosine-weighted direction in the hemisphere around `normal`.
fn diffuse(normal: Vec3, rng: &mut Rng) -> Vec3 {
    let (u, v) = (rng.next_f64(), rng.next_f64());
    let radius = u.sqrt();
    let angle = std::f64::consts::TAU * v;
    let (x, y, z) = (radius * angle.cos(), radius * angle.sin(), (1.0 - u).sqrt());
    // Any two axes perpendicular to the normal.
    let helper = if normal.x.abs() < 0.9 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let tangent = cross(helper, normal).normalized();
    let bitangent = cross(normal, tangent);
    (tangent * x + bitangent * y + normal * z).normalized()
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
        let directions = sphere_directions(1000);
        let mut sum = Vec3::ZERO;
        for d in &directions {
            assert!((d.length() - 1.0).abs() < 1e-9);
            sum = sum + *d;
        }
        assert!(sum.length() < 1.0, "{sum:?}");
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

    #[test]
    fn diffuse_directions_leave_the_surface() {
        let mut rng = Rng::new(3);
        let normal = Vec3::new(0.0, 1.0, 0.0);
        for _ in 0..1000 {
            let d = diffuse(normal, &mut rng);
            assert!(d.dot(normal) >= 0.0);
            assert!((d.length() - 1.0).abs() < 1e-9);
        }
    }
}
