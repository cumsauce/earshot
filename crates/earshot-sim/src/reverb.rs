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
/// The air's own absorption of energy, per metre, per band: about 1, 5 and
/// 25 dB a kilometre at 250 Hz, 1 kHz and 4 kHz (ISO 9613-1, 20 °C, 50 %
/// humidity). Eyring's loss takes it as `m · l`, the textbook Knudsen form
/// `T60 = 24 ln 10 · V / (c · (−S ln(1 − ā) + 4 m V))`: divide through by
/// `S` and `4V/S` is the mean free path. In a big space it shortens the
/// highs' decay.
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
    let directions = sphere_directions(rays, seed);
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
        // What a crossing through the surface takes besides the absorption,
        // when the way behind is open: a leaky wall drains the room, a
        // leaf-walled treehouse is nearly outdoors, and a rug over rock
        // keeps its room's bass.
        let leak = 1.0
            - if segment.leaky {
                acoustics.transmission[1] * segment.fill
            } else {
                0.0
            };
        if segment.bounce == 0 {
            self.first += length;
            self.firsts += 1;
            let direction = (segment.to - segment.from).normalized();
            let cos = direction.dot(segment.normal).abs() as f32;
            let trip = 2.0 * length;
            self.early.push(Reflection {
                direction,
                delay: (trip / SPEED_OF_SOUND) as f32,
                energy: (1.0 - acoustics.absorption[1]) * leak * cos * cos
                    / (trip * trip).max(1.0) as f32
                    * 12.0
                    / rays.max(1) as f32,
            });
        }
        for ((band, a), absorption) in self
            .absorbed
            .iter_mut()
            .enumerate()
            .zip(acoustics.absorption)
        {
            // Per band: what the surface soaks up, plus what gets through
            // it and never comes back — where the way behind is open.
            let absorption = f64::from(absorption);
            let through = if segment.leaky {
                f64::from(acoustics.transmission[band]).powf(f64::from(segment.fill))
            } else {
                0.0
            };
            *a += absorption + (1.0 - absorption) * through;
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
            let loss = -(1.0 - mean).ln() + AIR[band] * mean_free_path;
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

    /// Eyring's decay for a box of side `size` built of `material` in
    /// `band`, with the air's `air` (Np/m) over its mean free path 4V/S.
    /// `leaky` folds the wall's transmission in as a loss per hit; the trace
    /// recovers only part of it, since rays that graze along a wall layer
    /// leave late — so the shoebox test holds the decay between the leaky
    /// and the sealed formulas.
    fn eyring(size: f64, material: Material, band: usize, air: f64) -> f64 {
        let acoustics = material.acoustics();
        let a = f64::from(acoustics.absorption[band]);
        let (volume, surface) = (size.powi(3), 6.0 * size * size);
        let path = 4.0 * volume / surface;
        let loss = |a: f64| SIXTY_DB * path / (SPEED_OF_SOUND * (-((1.0 - a).ln()) + air * path));
        let leaky = a + (1.0 - a) * f64::from(acoustics.transmission[band]);
        loss(leaky)
    }

    /// Eyring's decay with no leak at all: the sealed-box figure.
    fn eyring_sealed(size: f64, material: Material, band: usize, air: f64) -> f64 {
        let a = f64::from(material.acoustics().absorption[band]);
        let (volume, surface) = (size.powi(3), 6.0 * size * size);
        let path = 4.0 * volume / surface;
        SIXTY_DB * path / (SPEED_OF_SOUND * (-((1.0 - a).ln()) + air * path))
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn shoebox_decay_matches_eyring() {
        for material in [
            Material::Wood,
            Material::Cloth,
            Material::Soil,
            Material::Rock,
            Material::Glass,
            Material::Metal,
            Material::Snow,
        ] {
            let world = closed_box(material, 12);
            let room = measure(&world, Vec3::new(6.0, 6.0, 6.0), 2048, 24, 64.0, 1);
            for band in 0..BANDS {
                let measured = f64::from(room.decay[band]);
                let least = eyring(12.0, material, band, AIR[band]);
                let most = eyring_sealed(12.0, material, band, AIR[band]).max(least);
                let (low, high) = (least.min(most), most.max(least));
                assert!(
                    measured >= 0.85 * low && measured <= 1.15 * high,
                    "{material:?} band {band}: {measured} outside {low}..{high}"
                );
            }
            assert_eq!(room.enclosure, 1.0);
        }
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn a_rectangular_room_sizes_itself_by_4v_over_s() {
        // 16 × 8 × 4 inside: V = 512, S = 448, so the mean free path is
        // 4V/S = 4.57, not the two thirds of any one side.
        let mut world = World::new();
        for x in -1..=16 {
            for y in -1..=8 {
                for z in -1..=4 {
                    let inside = (0..16).contains(&x) && (0..8).contains(&y) && (0..4).contains(&z);
                    if !inside {
                        world.set_block(x, y, z, Material::Rock);
                    }
                }
            }
        }
        let room = measure(&world, Vec3::new(8.0, 4.0, 2.0), 2048, 24, 64.0, 3);
        let path = 4.0 * 16.0 * 8.0 * 4.0 / 448.0;
        assert!(
            (room.mean_free_path - path).abs() < 0.1 * path,
            "{} vs {path}",
            room.mean_free_path
        );
        for band in 0..BANDS {
            let absorption = f64::from(Material::Rock.acoustics().absorption[band]);
            let loss = -(1.0 - absorption).ln() + AIR[band] * path;
            let expected = SIXTY_DB * path / (SPEED_OF_SOUND * loss);
            let measured = f64::from(room.decay[band]);
            assert!(
                (measured - expected).abs() < 0.25 * expected,
                "band {band}: {measured} vs {expected}"
            );
        }
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn the_air_takes_the_highs_of_a_big_hall() {
        // Metal barely absorbs, so in a hall 40 blocks across the air's own
        // absorption is what ends the highs: Eyring with the air's term. The
        // low and mid bands of such a hall outrun MAX_DECAY and clamp.
        let big = measure(
            &closed_box(Material::Metal, 40),
            Vec3::new(20.0, 20.0, 20.0),
            4096,
            32,
            64.0,
            1,
        );
        for band in 0..BANDS {
            let expected = eyring(40.0, Material::Metal, band, AIR[band]).min(MAX_DECAY);
            let measured = f64::from(big.decay[band]);
            assert!(
                (measured - expected).abs() < 0.25 * expected,
                "band {band}: {measured} vs {expected}"
            );
        }
    }

    /// The air coefficient read back out of closed metal boxes: sealed and
    /// uniform, so the wall term is exact and everything beyond it is air.
    /// The Eyring tests can't see a wrong air coefficient — their helper
    /// would repeat it — so this derives it from the decay directly.
    #[test]
    fn the_air_absorbs_at_the_rate_the_table_documents() {
        let metal = Material::Metal.acoustics();
        let implied = |t60: f32, mfp: f64| {
            SIXTY_DB / (SPEED_OF_SOUND * f64::from(t60))
                - -(1.0 - f64::from(metal.absorption[2])).ln() / mfp
        };
        // A 40-block hall, where the high band's loss is four fifths air.
        let hall = measure(
            &closed_box(Material::Metal, 40),
            Vec3::new(20.0, 20.0, 20.0),
            4096,
            32,
            64.0,
            1,
        );
        let air = implied(hall.decay[2], hall.mean_free_path);
        assert!(
            (air - AIR[2]).abs() < 0.2 * AIR[2],
            "high band, big hall: implied {air:.6} against {}",
            AIR[2]
        );
        // A 12-block box pins the small end: its mfp is short enough that
        // the decay stays unclamped while the air term is still a fifth of
        // the high band's loss.
        let box12 = measure(
            &closed_box(Material::Metal, 12),
            Vec3::new(6.0, 6.0, 6.0),
            4096,
            24,
            64.0,
            1,
        );
        let air = implied(box12.decay[2], box12.mean_free_path);
        assert!(
            (air - AIR[2]).abs() < 0.2 * AIR[2],
            "high band, small box: implied {air:.6} against {}",
            AIR[2]
        );
    }

    #[test]
    fn the_scattering_measured_is_what_the_surfaces_scatter() {
        let room = measure(
            &closed_box(Material::Cloth, 10),
            Vec3::new(5.0, 5.0, 5.0),
            1024,
            16,
            64.0,
            2,
        );
        let wanted = Material::Cloth.acoustics().scattering;
        assert!((room.scattering - wanted).abs() < 0.05, "{room:?}");
    }

    #[test]
    fn the_first_reflection_and_volume_size_the_space() {
        let room = measure(
            &closed_box(Material::Rock, 12),
            Vec3::new(6.0, 6.0, 6.0),
            2048,
            0,
            64.0,
            4,
        );
        // From the centre of a 12-cube the walls are 6 away along an axis and
        // 10.4 across the corner; the mean first hit lies between.
        assert!(
            room.first_reflection > 5.5 && room.first_reflection < 10.5,
            "{room:?}"
        );
        // The volume estimate is the room's order of magnitude, not the open
        // sky's, and not a shoebox's.
        assert!(room.volume > 500.0 && room.volume < 2000.0, "{room:?}");
    }

    #[test]
    fn the_same_seed_measures_the_same_room_twice() {
        let world = closed_box(Material::Rock, 12);
        let at = Vec3::new(6.0, 6.0, 6.0);
        // 2048 rays takes the threaded path, so this holds for the split
        // trace too.
        let a = measure_with_early(&world, at, 2048, 16, 64.0, 42);
        let b = measure_with_early(&world, at, 2048, 16, 64.0, 42);
        assert_eq!(a.0, b.0);
        assert_eq!(a.1, b.1);
        // A different seed is a different sample of the same room: nearby,
        // not identical.
        let c = measure_with_early(&world, at, 2048, 16, 64.0, 43).0;
        for band in 0..BANDS {
            let spread =
                (f64::from(c.decay[band] - a.0.decay[band]) / f64::from(a.0.decay[band])).abs();
            assert!(spread < 0.1, "band {band} moved {spread}");
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
