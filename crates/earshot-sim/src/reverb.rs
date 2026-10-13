use earshot_world::{BANDS, Bands, Vec3, World};

use crate::{Reflection, Segment, sphere_directions, trace_directions};

/// Speed of sound, in blocks (metres) per second.
pub const SPEED_OF_SOUND: f64 = 343.0;
/// `6 ln 10`: the natural-log drop of a 60 dB decay.
const SIXTY_DB: f64 = 13.815_510_557_964_274;
/// The shortest and longest decays reported, in seconds (EAX reverb's range).
const MIN_DECAY: f64 = 0.1;
/// About a cathedral's: past it a space is heard as echoes, not a longer
/// tail.
const MAX_DECAY: f64 = 8.0;
/// The air's own absorption of energy, per metre (Np/m), per band: about 1,
/// 5 and 25 dB a kilometre at 250 Hz, 1 kHz and 4 kHz (ISO 9613-1, 20 °C,
/// 50 % humidity). In a big space it shortens the highs' decay.
const AIR: [f64; BANDS] = [0.000_25, 0.001_15, 0.005_8];

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
    /// The volume of the space, m³, from how far the rays go before they
    /// first hit something, V = (4π/3)·mean(r³), counting a ray that escapes
    /// as reaching the trace's reach: huge out in the open, however near the
    /// ground is. 0 when unknown.
    pub volume: f64,
}

impl Room {
    /// Nothing around at all.
    pub const OPEN_AIR: Room = Room {
        decay: [MIN_DECAY as f32; BANDS],
        mean_free_path: 0.0,
        enclosure: 0.0,
        first_reflection: 0.0,
        scattering: 0.0,
        volume: 1e9,
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
    measure_with_early(world, listener, rays, bounces, reach, seed).0
}

/// [`measure`], plus the early reflections each ray's first bounce makes:
/// from the direction it went, after the trip to the wall and back, with the
/// energy of its share of the wall's image source,
/// `(1 - a) · cos² θ / (2r)² · 12 / rays` (the 12 makes a wall's rays add up
/// to its image source; see the test).
pub fn measure_with_early(
    world: &World,
    listener: Vec3,
    rays: u32,
    bounces: u32,
    reach: f64,
    seed: u64,
) -> (Room, Vec<Reflection>) {
    let directions = sphere_directions(rays);
    let threads = measure_threads(rays);
    let chunk = directions.len().div_ceil(threads).max(1);
    let tally = std::thread::scope(|scope| {
        let parts: Vec<_> = directions
            .chunks(chunk)
            .enumerate()
            .map(|(i, part)| {
                scope.spawn(move || {
                    let mut tally = Tally::default();
                    trace_directions(
                        world,
                        listener,
                        part,
                        bounces,
                        reach,
                        seed + i as u64,
                        |s| {
                            tally.add(&s, rays);
                        },
                    );
                    tally
                })
            })
            .collect();
        parts
            .into_iter()
            .map(|part| part.join().expect("a room trace thread panicked"))
            .fold(Tally::default(), Tally::merge)
    });
    tally.room(rays)
}

/// How many threads a room measurement of `rays` uses: one for small
/// traces, where starting threads would cost more than it saves, else up to
/// four.
pub fn measure_threads(rays: u32) -> usize {
    if rays < 2048 {
        return 1;
    }
    std::thread::available_parallelism().map_or(1, |n| n.get().min(4))
}

/// What a room trace adds up, per thread.
#[derive(Debug, Default)]
struct Tally {
    early: Vec<Reflection>,
    absorbed: [f64; BANDS],
    events: u32,
    hits: u32,
    firsts: u32,
    free_path: f64,
    first: f64,
    /// Sum of the first legs' lengths cubed, escapes at full reach.
    cubes: f64,
    scattering: f64,
}

impl Tally {
    fn add(&mut self, segment: &Segment, rays: u32) {
        self.events += 1;
        let length = (segment.to - segment.from).length();
        if segment.bounce == 0 {
            self.cubes += length.powi(3);
        }
        if segment.escaped {
            self.absorbed.iter_mut().for_each(|a| *a += 1.0);
            return;
        }
        self.free_path += length;
        self.hits += 1;
        let acoustics = segment.material.acoustics();
        if segment.bounce == 0 {
            self.first += length;
            self.firsts += 1;
            let direction = (segment.to - segment.from).normalized();
            let cos = direction.dot(segment.normal).abs() as f32;
            let trip = 2.0 * length;
            self.early.push(Reflection {
                direction,
                delay: (trip / SPEED_OF_SOUND) as f32,
                energy: (1.0 - acoustics.absorption[1]) * cos * cos / (trip * trip).max(1.0) as f32
                    * 12.0
                    / rays.max(1) as f32,
            });
        }
        for (a, absorption) in self.absorbed.iter_mut().zip(acoustics.absorption) {
            *a += f64::from(absorption);
        }
        self.scattering += f64::from(acoustics.scattering);
    }

    fn merge(mut self, other: Tally) -> Tally {
        self.early.extend(other.early);
        for (a, b) in self.absorbed.iter_mut().zip(other.absorbed) {
            *a += b;
        }
        self.events += other.events;
        self.hits += other.hits;
        self.firsts += other.firsts;
        self.free_path += other.free_path;
        self.first += other.first;
        self.cubes += other.cubes;
        self.scattering += other.scattering;
        self
    }

    fn room(self, rays: u32) -> (Room, Vec<Reflection>) {
        if self.hits == 0 {
            return (Room::OPEN_AIR, self.early);
        }
        let mean_free_path = self.free_path / f64::from(self.hits);
        let mut band = 0;
        let decay = self.absorbed.map(|a| {
            let mean = (a / f64::from(self.events)).clamp(1e-4, 1.0 - 1e-9);
            // Eyring's, with the air's absorption over each free path.
            let loss = -(1.0 - mean).ln() + 4.0 * AIR[band] * mean_free_path;
            band += 1;
            let t60 = SIXTY_DB * mean_free_path / (SPEED_OF_SOUND * loss);
            t60.clamp(MIN_DECAY, MAX_DECAY) as f32
        });
        let room = Room {
            decay,
            mean_free_path,
            enclosure: self.firsts as f32 / rays.max(1) as f32,
            first_reflection: if self.firsts > 0 {
                self.first / f64::from(self.firsts)
            } else {
                0.0
            },
            scattering: (self.scattering / f64::from(self.hits)) as f32,
            volume: 4.0 / 3.0 * std::f64::consts::PI * self.cubes / f64::from(rays.max(1)),
        };
        (room, self.early)
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

    /// Eyring's decay for a box of side `size` with absorption `a`, and the
    /// air's `air` (Np/m) over its mean free path 4V/S.
    fn eyring(size: f64, a: f64, air: f64) -> f64 {
        let (volume, surface) = (size.powi(3), 6.0 * size * size);
        let path = 4.0 * volume / surface;
        SIXTY_DB * path / (SPEED_OF_SOUND * (-(1.0 - a).ln() + 4.0 * air * path))
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn shoebox_decay_matches_eyring() {
        for material in [Material::Wood, Material::Cloth, Material::Soil] {
            let world = closed_box(material, 12);
            let room = measure(&world, Vec3::new(6.0, 6.0, 6.0), 2048, 24, 64.0, 1);
            for band in 0..BANDS {
                let expected = eyring(
                    12.0,
                    f64::from(material.acoustics().absorption[band]),
                    AIR[band],
                );
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
    fn a_walls_early_reflections_add_up_to_its_image_source() {
        // One big rock wall 5 blocks in front: its image source is 10 away.
        let mut world = World::new();
        for y in -40..40 {
            for z in -40..40 {
                world.set_block(5, y, z, Material::Rock);
            }
        }
        let (_, early) = measure_with_early(&world, Vec3::new(0.5, 0.5, 0.5), 4096, 0, 64.0, 1);
        let total: f32 = early.iter().map(|r| r.energy).sum();
        let image = (1.0 - Material::Rock.acoustics().absorption[1]) / 81.0;
        assert!((total / image - 1.0).abs() < 0.35, "{total} vs {image}");
        let first = early.iter().map(|r| r.delay).fold(f32::MAX, f32::min);
        assert!((first - 9.0 / 343.0).abs() < 0.002, "{first}");
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
