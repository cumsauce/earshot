use earshot_world::{BANDS, Bands, Vec3, World};

use crate::trace_paths;

/// Speed of sound, in blocks (metres) per second.
pub const SPEED_OF_SOUND: f64 = 343.0;
/// `6 ln 10`: the natural-log drop of a 60 dB decay.
const SIXTY_DB: f64 = 13.815_510_557_964_274;
/// The shortest and longest decays reported, in seconds (EAX reverb's range).
const MIN_DECAY: f64 = 0.1;
const MAX_DECAY: f64 = 20.0;

/// What a listener's surroundings do to sound, from a path trace: the input
/// to the reverb (docs/PLAN.md, M3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Room {
    /// Seconds for the reverb to fall by 60 dB, per band (Eyring's formula
    /// over the traced mean free path and absorption).
    pub decay: Bands,
    /// Mean distance between reflections, in blocks: the size of the space.
    pub mean_free_path: f64,
    /// Share of rays whose first leg hits something: 1 shut in, about 0.5 on
    /// open ground, 0 in the sky.
    pub enclosure: f32,
    /// Mean distance from the listener to the first surface, in blocks.
    pub first_reflection: f64,
    /// Mean scattering of the surfaces hit.
    pub scattering: f32,
}

impl Room {
    /// Nothing around at all.
    pub const OPEN_AIR: Room = Room {
        decay: [MIN_DECAY as f32; BANDS],
        mean_free_path: 0.0,
        enclosure: 0.0,
        first_reflection: 0.0,
        scattering: 0.0,
    };
}

/// Measures the room around `listener` with `rays` paths of up to `bounces`
/// reflections and `reach` blocks per leg.
///
/// Each reflection loses its surface's absorption, and each escape counts as
/// a reflection off a fully absorbing surface: the open sky. Eyring's formula
/// then gives the decay from the mean free path `l` and mean absorption `a`:
/// `T60 = 6 ln 10 · l / (c · -ln(1 - a))`.
pub fn measure(
    world: &World,
    listener: Vec3,
    rays: u32,
    bounces: u32,
    reach: f64,
    seed: u64,
) -> Room {
    let mut absorbed = [0.0f64; BANDS];
    let (mut events, mut hits, mut firsts) = (0u32, 0u32, 0u32);
    let (mut free_path, mut first, mut scattering) = (0.0, 0.0, 0.0f64);
    trace_paths(world, listener, rays, bounces, reach, seed, |segment| {
        events += 1;
        if segment.escaped {
            absorbed.iter_mut().for_each(|a| *a += 1.0);
            return;
        }
        let length = (segment.to - segment.from).length();
        free_path += length;
        hits += 1;
        if segment.bounce == 0 {
            first += length;
            firsts += 1;
        }
        let acoustics = segment.material.acoustics();
        for (a, absorption) in absorbed.iter_mut().zip(acoustics.absorption) {
            *a += f64::from(absorption);
        }
        scattering += f64::from(acoustics.scattering);
    });
    if hits == 0 {
        return Room::OPEN_AIR;
    }
    let mean_free_path = free_path / f64::from(hits);
    let decay = absorbed.map(|a| {
        let mean = (a / f64::from(events)).clamp(1e-4, 1.0 - 1e-9);
        let t60 = SIXTY_DB * mean_free_path / (SPEED_OF_SOUND * -(1.0 - mean).ln());
        t60.clamp(MIN_DECAY, MAX_DECAY) as f32
    });
    Room {
        decay,
        mean_free_path,
        enclosure: firsts as f32 / rays.max(1) as f32,
        first_reflection: if firsts > 0 {
            first / f64::from(firsts)
        } else {
            0.0
        },
        scattering: (scattering / f64::from(hits)) as f32,
    }
}

#[cfg(test)]
mod tests {
    use earshot_world::Material;

    use super::*;

    /// A closed box of `material`, `size` blocks inside on each side.
    fn closed_box(material: Material, size: i32) -> World {
        let mut world = World::new();
        for a in -1..=size {
            for b in -1..=size {
                for wall in [-1, size] {
                    world.set_block(wall, a, b, material);
                    world.set_block(a, wall, b, material);
                    world.set_block(a, b, wall, material);
                }
            }
        }
        world
    }

    /// Eyring's decay for a box of side `size` with absorption `a`.
    fn eyring(size: f64, a: f64) -> f64 {
        let (volume, surface) = (size.powi(3), 6.0 * size * size);
        SIXTY_DB * 4.0 * volume / (SPEED_OF_SOUND * surface * -(1.0 - a).ln())
    }

    #[test]
    fn shoebox_decay_matches_eyring() {
        for material in [Material::Wood, Material::Cloth, Material::Soil] {
            let world = closed_box(material, 12);
            let room = measure(&world, Vec3::new(6.0, 6.0, 6.0), 2048, 24, 64.0, 1);
            for band in 0..BANDS {
                let expected = eyring(12.0, f64::from(material.acoustics().absorption[band]));
                let measured = f64::from(room.decay[band]);
                let error = (measured - expected).abs() / expected;
                assert!(
                    error < 0.2,
                    "{material:?} band {band}: {measured} vs {expected}"
                );
            }
            assert_eq!(room.enclosure, 1.0);
        }
    }

    #[test]
    fn bigger_and_harder_rooms_ring_longer() {
        let small = measure(
            &closed_box(Material::Rock, 4),
            Vec3::new(2.0, 2.0, 2.0),
            1024,
            16,
            64.0,
            1,
        );
        let large = measure(
            &closed_box(Material::Rock, 24),
            Vec3::new(12.0, 12.0, 12.0),
            1024,
            16,
            64.0,
            1,
        );
        let soft = measure(
            &closed_box(Material::Cloth, 24),
            Vec3::new(12.0, 12.0, 12.0),
            1024,
            16,
            64.0,
            1,
        );
        assert!(large.decay[1] > small.decay[1] * 3.0, "{small:?} {large:?}");
        assert!(soft.decay[1] < large.decay[1] / 5.0, "{soft:?}");
        assert!(large.mean_free_path > small.mean_free_path * 4.0);
        // Cloth swallows highs: they die faster than lows.
        assert!(soft.decay[2] < soft.decay[0]);
    }

    #[test]
    fn the_open_is_dry() {
        let mut ground = World::new();
        for x in -64..64 {
            for z in -64..64 {
                ground.set_block(x, -1, z, Material::Soil);
            }
        }
        let room = measure(&ground, Vec3::new(0.5, 1.6, 0.5), 1024, 8, 64.0, 1);
        assert!((room.enclosure - 0.5).abs() < 0.1, "{room:?}");
        assert!(room.decay[1] < 0.3, "{room:?}");
        assert_eq!(
            measure(&World::new(), Vec3::ZERO, 64, 4, 64.0, 1),
            Room::OPEN_AIR
        );
    }
}
