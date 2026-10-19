use earshot_world::{BANDS, Bands, Vec3, World};

use crate::{Reflection, Segment, sphere_directions, trace_directions};

/// Speed of sound, in blocks (metres) per second.
pub const SPEED_OF_SOUND: f64 = 343.0;

/// Speed of sound in air at `celsius`, in blocks (metres) per second: the
/// textbook `331.3 + 0.606·T` (T in °C), which is why a 40 °C desert carries
/// sound about 2 % faster than a snowy biome at −10 °C. Both ends clamp, and
/// each for the same reason: a reading no real weather produces must not
/// reach the timing maths. The temperature clamps to the band weather stays
/// in (−60 to 60 °C), and the result to 300..=370 m/s, so nothing calling
/// this can ever divide by a zero or negative speed. Room measurement keeps
/// the fixed [`SPEED_OF_SOUND`]; the timing paths (echo and reflection
/// delays) take this instead.
pub fn speed_of_sound(celsius: f64) -> f64 {
    (331.3 + 0.606 * celsius.clamp(-60.0, 60.0)).clamp(300.0, 370.0)
}
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
/// Seconds per bin of the traced energy decay curve: a millisecond, fine
/// enough to resolve the early part of a decay (the fit reads the first
/// 10 dB) while a 4 s horizon covers the longest decay reported.
const HIST_DT: f64 = 0.001;
/// Bins in the traced energy decay curve: 4.096 s. The first −10 dB point
/// of any decay this code reports (≤ 8 s) lands within 1.33 s, so the fit
/// never runs off the end.
const HIST_BINS: usize = 4096;

/// What a listener's surroundings do to sound, from a path trace: the input
/// to the reverb (docs/PLAN.md, M3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Room {
    /// Seconds for the reverb to fall by 60 dB, per band (Eyring's formula
    /// over the traced mean free path and absorption).
    pub decay: Bands,
    /// Seconds for the early part of the decay — the first 10 dB read off
    /// the traced energy decay curve — per band. Shorter than [`decay`]
    /// where the room is coupled: a dead antechamber falls fast before the
    /// live space behind it takes over the tail.
    pub early_decay: Bands,
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
        early_decay: [MIN_DECAY as f32; BANDS],
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
    measure_with_parts(
        world,
        listener,
        rays,
        bounces,
        reach,
        seed,
        measure_parts(rays),
    )
}

/// How many parts a measurement of `rays` is split into: a fixed function
/// of the ray count alone, never of the machine, so the same world and
/// seed split the same way on every CPU. Four parts once the trace is
/// worth splitting, one before it.
fn measure_parts(rays: u32) -> usize {
    if rays < 2048 { 1 } else { 4 }
}

/// [`measure_with_early`], with the ray budget split into `parts` traces
/// instead of whatever the ray count calls for: the seam the parts
/// equality test measures through. Each part traces a fixed slice of the
/// directions with its own seed, derived from the part's index, so what a
/// part traces depends on neither the thread count nor the core count.
/// Threads only choose how many parts run at once, and the parts are
/// always merged in index order, so the room comes out identical whether
/// the parts run one after another or side by side, on any machine.
fn measure_with_parts(
    world: &World,
    listener: Vec3,
    rays: u32,
    bounces: u32,
    reach: f64,
    seed: u64,
    parts: usize,
) -> (Room, Vec<Reflection>) {
    let directions = sphere_directions(rays, seed);
    // Never more parts than rays, and never none: an empty trace is one
    // part holding nothing.
    let parts = parts.clamp(1, directions.len().max(1));
    let chunk = directions.len().div_ceil(parts).max(1);
    let workers = measure_threads(rays).clamp(1, parts);
    // Declared outside the scope: the scope body's own locals are dropped
    // before the scope joins its threads, so what the workers share must
    // be born outside the body.
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let next = &next;
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut traced = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(rest) = directions.get(i * chunk..) else {
                            break;
                        };
                        if rest.is_empty() {
                            break;
                        }
                        let part = &rest[..rest.len().min(chunk)];
                        let mut tally = Tally::new(bounces);
                        trace_directions(
                            world,
                            listener,
                            part,
                            bounces,
                            reach,
                            // Wrapped so a seed at the top of the range
                            // measures the same in debug and release
                            // builds, where overflow panics or not differs.
                            seed.wrapping_add(i as u64),
                            |s| {
                                tally.add(&s, rays);
                            },
                        );
                        traced.push((i, tally));
                    }
                    traced
                })
            })
            .collect();
        let mut traced: Vec<(usize, Tally)> = handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("a room trace thread panicked"))
            .collect();
        // Merging in part order keeps the sum the room is read off free of
        // any trace of which thread ran which part.
        traced.sort_by_key(|&(i, _)| i);
        traced
            .into_iter()
            .map(|(_, tally)| tally)
            .fold(Tally::new(bounces), |total, part| total.merge(&part))
    })
    .room(rays)
}

/// How many threads a room measurement of `rays` uses at once: one for
/// small traces, where starting threads would cost more than it saves,
/// else up to four. It only picks the pace. The split itself is
/// [`measure_parts`], fixed by the ray count, so a machine's cores never
/// change what gets traced or with which seed.
pub fn measure_threads(rays: u32) -> usize {
    if rays < 2048 {
        return 1;
    }
    std::thread::available_parallelism().map_or(1, |n| n.get().min(4))
}

/// What a room trace adds up, per thread.
#[derive(Debug)]
struct Tally {
    early: Vec<Reflection>,
    absorbed: [f64; BANDS],
    events: u32,
    hits: u32,
    firsts: u32,
    /// Every hit leg's length, in the order the legs were traced. Held as
    /// the ordered list and summed once at the end rather than summed as
    /// they arrive: a running float sum depends on the order its terms
    /// were added in, and which part a leg's sum lands in depends on how
    /// the ray budget was split, which must not change the room measured.
    legs: Vec<f64>,
    /// The hit first legs' lengths, in trace order, for the same reason.
    first_legs: Vec<f64>,
    /// The first legs' lengths cubed, escapes at full reach, in trace
    /// order, for the same reason.
    first_cubes: Vec<f64>,
    scattering: f64,
    /// The trace's bounce cap: rays reaching it are cut off with energy
    /// still travelling, which [`Tally::room`] continues analytically.
    bounces: u32,
    /// Seconds along the ray being tallied, since it left the listener.
    elapsed: f64,
    /// Per-band energy still riding the current ray: 1.0 at birth, dropped
    /// at each surface by what the surface took.
    band_energy: [f64; BANDS],
    /// The energy decay curve: the ray energy present in each one
    /// millisecond bin, per band — what the early decay is read off. Boxed:
    /// a room is measured on the FFI thread, whose Java stack is a fraction
    /// of a megabyte, and this is 96 KB on its own.
    hist: Box<[[f64; HIST_BINS]; BANDS]>,
    /// Energy the bounce cap would have carried on, at the bin the ray
    /// reached the cap: the curve's tail after the trace's horizon. Boxed
    /// with `hist`.
    orphan: Box<[[f64; HIST_BINS]; BANDS]>,
}

impl Default for Tally {
    fn default() -> Self {
        Tally {
            early: Vec::new(),
            absorbed: [0.0; BANDS],
            events: 0,
            hits: 0,
            firsts: 0,
            legs: Vec::new(),
            first_legs: Vec::new(),
            first_cubes: Vec::new(),
            scattering: 0.0,
            bounces: 0,
            elapsed: 0.0,
            band_energy: [0.0; BANDS],
            hist: Box::new([[0.0; HIST_BINS]; BANDS]),
            orphan: Box::new([[0.0; HIST_BINS]; BANDS]),
        }
    }
}

impl Tally {
    fn new(bounces: u32) -> Tally {
        Tally {
            bounces,
            ..Tally::default()
        }
    }

    fn add(&mut self, segment: &Segment, rays: u32) {
        self.events += 1;
        let length = (segment.to - segment.from).length();
        if segment.bounce == 0 {
            self.first_cubes.push(length.powi(3));
            self.elapsed = 0.0;
            self.band_energy = [1.0; BANDS];
        }
        let leg_start = self.elapsed;
        let leg_end = leg_start + length / SPEED_OF_SOUND;
        if segment.escaped {
            // The ray leaves for good: its energy is gone with it, not
            // deposited anywhere in the room's decay curve.
            self.absorbed.iter_mut().for_each(|a| *a += 1.0);
            self.elapsed = leg_end;
            return;
        }
        // The air takes its own share of the energy as the ray travels this
        // leg. The curve's slope must fall by that share too: `Tally::room`
        // adds it as `AIR · mean_free_path` to the Eyring loss, so a curve
        // that kept the air's energy would read an early decay longer than
        // the room's own by the air's share of the loss. Not added to
        // `absorbed` — that term is the air's, counted once there.
        for (energy, air) in self.band_energy.iter_mut().zip(AIR) {
            *energy *= (-air * length).exp();
        }
        // Every bin the leg spans holds this ray's energy, but only for
        // the part of the bin the leg actually covers: a bin is a one
        // millisecond window, 0.343 m of travel, and a leg in a small room
        // can be shorter, starting or ending inside one. Counting every bin
        // a leg touched at full weight counted the bin where one leg ends
        // and the next begins twice (the next leg's first bin is this leg's
        // last), which bowed the curve up wherever the legs were short. In
        // a room measured from a listener against a wall it inflated the
        // peak at bin 0 and dragged the early decay under the room's own.
        // Each bin instead gets the leg's share of it: the fraction of the
        // bin the leg spans, so a ray's legs end to end add up to its
        // energy over a bin exactly once, however they line up with the
        // bin edges, and a leg wholly inside one bin still leaves the
        // ray's energy there for the time it truly occupies.
        let first_bin = (leg_start / HIST_DT) as usize;
        let last_bin = (leg_end / HIST_DT) as usize;
        for bin in first_bin..=last_bin {
            if bin >= HIST_BINS {
                break;
            }
            let bin_from = (bin as f64 * HIST_DT).max(leg_start);
            let bin_to = ((bin + 1) as f64 * HIST_DT).min(leg_end);
            let share = (bin_to - bin_from) / HIST_DT;
            if share <= 0.0 {
                continue;
            }
            for (hist, energy) in self.hist.iter_mut().zip(self.band_energy) {
                hist[bin] += energy * share;
            }
        }
        self.elapsed = leg_end;
        self.legs.push(length);
        self.hits += 1;
        let acoustics = segment.material.acoustics();
        // What a crossing through the surface takes besides the absorption,
        // when the way behind is open: a leaky wall drains the room, a
        // leaf-walled treehouse is nearly outdoors, and a rug over rock
        // keeps its room's bass.
        let leak = 1.0
            - if segment.leaky {
                f64::from(acoustics.transmission[1])
                    .powf(f64::from(segment.thickness) * f64::from(segment.fill))
            } else {
                0.0
            };
        let leak = leak as f32;
        if segment.bounce == 0 {
            self.first_legs.push(length);
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
                f64::from(acoustics.transmission[band])
                    .powf(f64::from(segment.thickness) * f64::from(segment.fill))
            } else {
                0.0
            };
            let loss = absorption + (1.0 - absorption) * through;
            *a += loss;
            self.band_energy[band] *= 1.0 - loss;
        }
        self.scattering += f64::from(acoustics.scattering);
        // At the bounce cap, keep the still-travelling energy: where the
        // trace stops, the room's own decay continues it (Tally::room).
        if segment.bounce == self.bounces && self.band_energy[1] >= 1e-6 {
            let bin = (self.elapsed / HIST_DT) as usize;
            if bin < HIST_BINS {
                for (orphan, energy) in self.orphan.iter_mut().zip(self.band_energy) {
                    orphan[bin] += energy;
                }
            }
        }
    }

    /// Adds `other` into `self`. Borrows `self` and takes `other` by value:
    /// the histograms are boxed, so only the box pointers move, and the
    /// running total stays put on the stack rather than being built afresh by
    /// value in a fold.
    fn merge(mut self, other: &Tally) -> Tally {
        self.early.extend(other.early.iter().copied());
        for (a, b) in self.absorbed.iter_mut().zip(other.absorbed) {
            *a += b;
        }
        self.events += other.events;
        self.hits += other.hits;
        self.firsts += other.firsts;
        self.legs.extend(other.legs.iter().copied());
        self.first_legs.extend(other.first_legs.iter().copied());
        self.first_cubes.extend(other.first_cubes.iter().copied());
        self.scattering += other.scattering;
        for band in 0..BANDS {
            for bin in 0..HIST_BINS {
                self.hist[band][bin] += other.hist[band][bin];
                self.orphan[band][bin] += other.orphan[band][bin];
            }
        }
        self
    }

    fn room(self, rays: u32) -> (Room, Vec<Reflection>) {
        if self.hits == 0 {
            return (Room::OPEN_AIR, self.early);
        }
        // The held lists sum here, once, in trace order: however the
        // budget was split into parts, the merged list holds the same
        // legs in the same order, so these totals are the same numbers.
        let free_path: f64 = self.legs.iter().sum();
        let first: f64 = self.first_legs.iter().sum();
        let cubes: f64 = self.first_cubes.iter().sum();
        let mean_free_path = free_path / f64::from(self.hits);
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
            early_decay: [0, 1, 2].map(|band| {
                // Where the bounce cap stopped a ray, continue its energy
                // under the room's own decay: the tail the trace would have
                // seen had it gone on.
                let q = (-SIXTY_DB * HIST_DT / f64::from(decay[band])).exp();
                let mut carried = 0.0;
                let mut curve = [0.0; HIST_BINS];
                for ((curve, hist), orphan) in curve
                    .iter_mut()
                    .zip(&self.hist[band])
                    .zip(&self.orphan[band])
                {
                    carried = carried * q + orphan;
                    *curve = hist + carried;
                }
                fit_early_decay(&curve, decay[band])
            }),
            mean_free_path,
            enclosure: self.firsts as f32 / rays.max(1) as f32,
            first_reflection: if self.firsts > 0 {
                first / f64::from(self.firsts)
            } else {
                0.0
            },
            scattering: (self.scattering / f64::from(self.hits)) as f32,
            volume: 4.0 / 3.0 * std::f64::consts::PI * cubes / f64::from(rays.max(1)),
        };
        (room, self.early)
    }
}

/// Reads the early decay — seconds for 60 dB at the rate of the curve's
/// first 10 dB — off an energy decay curve of `HIST_BINS` one-millisecond
/// bins holding the ray energy at each moment, falling back to `fallback`
/// where the curve never drops a decibel (or drops so fast no slope can be
/// fit).
fn fit_early_decay(hist: &[f64], fallback: f32) -> f32 {
    // The curve's own peak, not bin 0's: from a listener against a wall the
    // first legs can be shorter than a bin, so the energy keeps arriving for
    // a bin or two and the curve peaks past bin 0. Reading bin 0 as the peak
    // there makes the fit start on a rising curve and biases its slope.
    let peak = hist.iter().copied().fold(0.0, f64::max);
    if peak <= 0.0 || !peak.is_finite() {
        return fallback;
    }
    // Where the curve reaches its peak: the fit window is read from there on,
    // since before it the energy is still rising.
    let peak_bin = hist
        .iter()
        .position(|&value| value == peak)
        .unwrap_or_default();
    let level = |bin: usize| {
        let value = hist[bin];
        // A bin that isn't a number has no level: treat it as silence rather
        // than let `f64::max`'s NaN-swallowing turn it into −3076 dB.
        if !value.is_finite() {
            return f64::NEG_INFINITY;
        }
        10.0 * (value / peak).max(f64::MIN_POSITIVE).log10()
    };
    // The fit runs from the first −1 dB, past the ramp of rays still
    // starting out, to the first −10 dB: ISO 3382's window for EDT.
    let (Some(first), Some(last)) = (
        (peak_bin..HIST_BINS).find(|&bin| level(bin) <= -1.0),
        (peak_bin..HIST_BINS).find(|&bin| level(bin) <= -10.0),
    ) else {
        return fallback;
    };
    let n = (last - first + 1) as f64;
    let mut sum_t = 0.0;
    let mut sum_db = 0.0;
    let mut sum_tt = 0.0;
    let mut sum_td = 0.0;
    for bin in first..=last {
        let t = bin as f64 * HIST_DT;
        let db = level(bin);
        sum_t += t;
        sum_db += db;
        sum_tt += t * t;
        sum_td += t * db;
    }
    // Least squares: the curve's slope in decibels a second, always down.
    let denom = n * sum_tt - sum_t * sum_t;
    let slope = (n * sum_td - sum_t * sum_db) / denom;
    if !slope.is_finite() || slope >= 0.0 {
        return fallback;
    }
    (60.0 / -slope).clamp(MIN_DECAY, MAX_DECAY) as f32
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

    #[test]
    fn speed_of_sound_matches_the_textbook_curve() {
        // The textbook line c = 331.3 + 0.606·T: still air at 0 °C, and
        // 343.42 m/s at 20 °C, the room-air default within a tenth of a
        // percent of the fixed SPEED_OF_SOUND.
        assert_eq!(speed_of_sound(0.0), 331.3);
        assert!(
            (speed_of_sound(20.0) - 343.42).abs() < 1e-9,
            "{}",
            speed_of_sound(20.0)
        );
        // Past where real weather goes the temperature clamps, so a desert
        // or an arctic reading can never run the curve away.
        assert_eq!(speed_of_sound(60.0), speed_of_sound(1e9));
        assert_eq!(speed_of_sound(-60.0), speed_of_sound(-1e9));
        // The result stays inside a sane band however absurd the input, so
        // a division by it can never see a zero or a negative speed.
        for celsius in [-1e9, -60.0, 0.0, 20.0, 60.0, 1e9] {
            let speed = speed_of_sound(celsius);
            assert!(
                (300.0..=370.0).contains(&speed),
                "speed at {celsius} °C: {speed}"
            );
        }
        // The cold end falls to the band's floor: the curve alone would
        // give 294.94 m/s at −60 °C.
        assert_eq!(speed_of_sound(-60.0), 300.0);
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

    /// A tally must stay small enough to live on the stack of whatever
    /// thread measures a room: `earshot_room_at` runs on a Java thread with
    /// the JVM's default stack (a fraction of a megabyte), and a `Tally` by
    /// value in `measure_with_early`'s fold, plus another for the running
    /// total, is what overflowed it and crashed the process inside the
    /// library, its frames smeared past the guard page.
    #[test]
    fn a_tally_small_enough_for_the_stacks_that_measure_rooms() {
        assert!(
            std::mem::size_of::<Tally>() <= 4096,
            "Tally is {} bytes: the histograms must be boxed, not inline",
            std::mem::size_of::<Tally>()
        );
    }

    /// A room measured on a JVM-sized stack: `earshot_room_at` runs on a Java
    /// thread with the JVM's default stack, a fraction of a megabyte, so
    /// measuring a room there must not need more than that. With the
    /// histograms inline (192 KB each, by value through [`Tally::merge`]) this
    /// overflowed the stack and the process died inside the library; boxed,
    /// it fits with room to spare. Skipped where threads can't take a small
    /// stack, since there is nothing to check there.
    #[test]
    fn a_room_measures_on_a_jvm_sized_stack() {
        // The JVM's default is usually 512 KB-1 MB; 256 KB is the small end,
        // and the point of the test is that it fits at all.
        let measured = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let world = closed_box(Material::Rock, 12);
                measure(&world, Vec3::new(6.0, 6.0, 6.0), 512, 8, 64.0, 1)
            });
        let Ok(handle) = measured else {
            return;
        };
        let room = handle
            .join()
            .expect("the measurement thread overflowed its stack");
        assert!(room.decay[1] > 0.1, "{room:?}");
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

    /// The parts a measurement's ray budget is split into must not change
    /// the room it measures: whole or split into four, the same world and
    /// seed give back the same room, field for field. With no bounces
    /// every leg is recorded before the scatter and transmission draws
    /// can steer a ray, so each part traces the same paths either way,
    /// and what is left to get right is the split and the merge: same
    /// slices of the directions, same seeds from the part index, same
    /// sum. The old code split by the thread count, so a two-core and a
    /// four-core machine measured different rooms; this pins that away.
    #[test]
    fn the_parts_of_a_measurement_do_not_change_the_room() {
        let world = closed_box(Material::Rock, 12);
        let at = Vec3::new(6.0, 6.0, 6.0);
        let whole = measure_with_parts(&world, at, 2048, 0, 64.0, 42, 1);
        let split = measure_with_parts(&world, at, 2048, 0, 64.0, 42, 4);
        // A sealed box shuts in every ray: the comparison must be between
        // two real rooms, not two open-air defaults.
        assert_eq!(whole.0.enclosure, 1.0, "{:?}", whole.0);
        assert_eq!(whole.0, split.0);
        assert_eq!(whole.1, split.1);
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

    /// A dead chamber opening through a wide arch onto a live hall: the
    /// curve's first 10 dB is the chamber's own steep fall, while the
    /// mean-absorption Eyring figure behind it is the mix, held up by the
    /// hall. An `early_decay` that merely repeats `decay` fails the coupled
    /// case below — that equality is the placeholder this test exists to
    /// break.
    #[test]
    fn a_dead_chamber_falls_faster_than_the_mix_it_opens_onto() {
        // A rock hall with a cloth chamber inside it; `arch` cuts a wide
        // opening through the shared wall, so the hall's rays flood in.
        let couplet = |arch: bool| {
            let mut world = closed_box(Material::Rock, 20);
            for a in 3..=12 {
                for b in 3..=12 {
                    for w in [3, 12] {
                        world.set_block(w, a, b, Material::Cloth);
                        world.set_block(a, w, b, Material::Cloth);
                        world.set_block(a, b, w, Material::Cloth);
                    }
                }
            }
            if arch {
                // Six of the wall's ten rows by eight of its ten columns.
                for y in 5..=10 {
                    for z in 4..=11 {
                        world.set_block(3, y, z, Material::Air);
                    }
                }
            }
            world
        };
        let at = Vec3::new(8.0, 8.0, 8.0);
        let sealed = measure(&couplet(false), at, 2048, 16, 64.0, 1);
        // Sealed, one material in sight: a single slope, and the fit lands
        // on Eyring's figure for it.
        assert!(
            f64::from(sealed.early_decay[1]) > 0.8 * f64::from(sealed.decay[1]),
            "sealed {sealed:?}"
        );
        let open = measure(&couplet(true), at, 2048, 16, 64.0, 1);
        // Open, the cloth drains the early curve while the hall's slow
        // energy — and Eyring's mean over it — keeps the mix ringing.
        // Seeded actuals: early 0.515 against decay 0.660 (ratio 0.780),
        // so the bound sits at 0.85: clear of the placeholder's 1.0, with
        // the measured separation to spare.
        assert!(
            f64::from(open.early_decay[1]) < 0.85 * f64::from(open.decay[1]),
            "coupled {open:?}"
        );
    }

    /// One material everywhere: the traced curve is a single slope, so the
    /// early fit lands on Eyring's figure — a flat head and sampling noise
    /// move it only a little. Seeded actuals: early 4.621 against decay
    /// 4.135 (ratio 1.117), where the twice-counted end bins read 4.528
    /// (1.095): the bound sits at 0.2 with the measured 0.117 to spare.
    #[test]
    fn a_uniform_rooms_early_decay_is_its_decay() {
        let room = measure(
            &closed_box(Material::Rock, 12),
            Vec3::new(6.0, 6.0, 6.0),
            2048,
            24,
            64.0,
            1,
        );
        let early = f64::from(room.early_decay[1]);
        let late = f64::from(room.decay[1]);
        assert!((early / late - 1.0).abs() < 0.2, "{early} against {late}");
        for band in 0..BANDS {
            assert!(
                (0.1..=8.0).contains(&f64::from(room.early_decay[band])),
                "{room:?}"
            );
        }
    }

    /// A listener pressed against the wall of a small room hears the room a
    /// listener in its centre hears. In a room whose legs are shorter than
    /// one bin's 0.343 m of travel, counting every bin a leg touched at
    /// full weight counted the shared end bins twice nearly everywhere,
    /// and from the wall, where half the first legs fall inside a single
    /// bin, it inflated the peak at bin 0; the early decay read under the
    /// room's own: seeded actuals 0.854 (centre) and 0.890 (wall) before
    /// the fix, 0.999 and 1.001 with each bin holding only the legs'
    /// share of it. Both sides of the room must land within 0.1 of the
    /// Eyring figure.
    #[test]
    fn a_listener_against_the_wall_hears_the_room() {
        let world = closed_box(Material::Rock, 2);
        let mid_ratio = |at: Vec3| {
            let room = measure(&world, at, 1024, 16, 64.0, 1);
            let early = f64::from(room.early_decay[1]);
            let late = f64::from(room.decay[1]);
            (early / late, room)
        };
        let (centre, centre_room) = mid_ratio(Vec3::new(1.0, 1.0, 1.0));
        let (wall, wall_room) = mid_ratio(Vec3::new(0.02, 1.0, 1.0));
        assert!(
            (centre - 1.0).abs() < 0.1,
            "centre: {centre} in {centre_room:?}"
        );
        assert!((wall - 1.0).abs() < 0.1, "wall: {wall} in {wall_room:?}");
    }

    /// The fit reads the slope of the curve's first ten decibels: of a pure
    /// curve, its own rate; of a two-slope curve, the steep part before the
    /// knee, never the tail.
    #[test]
    fn the_early_decay_fits_the_first_ten_decibels_of_the_curve() {
        let rays = 2048.0;
        let mut hist = vec![0.0; HIST_BINS];
        let fill = |hist: &mut [f64], curve: &dyn Fn(f64) -> f64| {
            for (bin, e) in hist.iter_mut().enumerate() {
                *e = rays * curve(bin as f64 * HIST_DT);
            }
        };
        // A pure 2 s energy curve, read back within a fifth — energy, so
        // the curve falls `6 ln 10` over the T60, not `3 ln 10`. The
        // fallback is out of range on purpose, so a stub can't pass by
        // accident.
        fill(&mut hist, &|t| (-SIXTY_DB * t / 2.0).exp());
        let pure = fit_early_decay(&hist, 9.0);
        assert!((pure - 2.0).abs() < 0.2 * 2.0, "{pure}");
        // Two slopes: 0.3 s until −10 dB, then 2 s after. The early decay
        // is the steep part, not the tail.
        let knee = 0.3 / 6.0;
        fill(&mut hist, &|t| {
            let db = if t <= knee {
                -10.0 * t / knee
            } else {
                -10.0 - 60.0 * (t - knee) / 2.0
            };
            // Power decibels: e^(db·ln 10 / 10), not e^(db / 10).
            10.0f64.powf(db / 10.0)
        });
        let steep = fit_early_decay(&hist, 9.0);
        assert!((steep - 0.3).abs() < 0.2 * 0.3, "{steep}");
    }

    /// The fallback the fit hands back where there is no slope to fit: a
    /// curve that never falls 10 dB inside the histogram's 4.096 s horizon
    /// — flat, or so slow it only slips past −1 dB — has no ISO 3382
    /// window, and `fit_early_decay` returns the Eyring decay it was given
    /// (the sentinel `Tally::room` passes in). So does an empty curve with
    /// no peak at all.
    #[test]
    fn a_curve_that_never_drops_ten_decibels_falls_back_to_eyrings_decay() {
        let fallback = 3.5;
        let flat = vec![2048.0; HIST_BINS];
        assert_eq!(fit_early_decay(&flat, fallback), fallback);
        // −0.434 dB/s: it passes −1 dB at 2.3 s but reaches only −1.8 dB
        // by the horizon, so the −10 dB end of the fit window never lands.
        let mut slow = vec![0.0; HIST_BINS];
        for (bin, e) in slow.iter_mut().enumerate() {
            *e = 2048.0 * (-0.1 * bin as f64 * HIST_DT).exp();
        }
        assert_eq!(fit_early_decay(&slow, fallback), fallback);
        let empty = vec![0.0; HIST_BINS];
        assert_eq!(fit_early_decay(&empty, fallback), fallback);
    }

    /// A decay that is flat and then falls off a cliff: 60 dB inside one
    /// histogram bin. Both ends of the fit's window land on the same bin,
    /// so the least squares slope there is 0/0, and the answer must still
    /// be a usable number: finite, positive, and no shorter than the bin
    /// it was read off.
    #[test]
    fn the_early_decay_of_a_one_bin_cliff_is_finite() {
        let fallback = 0.5;
        let mut hist = vec![2048.0; HIST_BINS];
        let cliff = HIST_BINS / 2;
        for (bin, e) in hist.iter_mut().enumerate().skip(cliff) {
            // 60 dB down in the one bin, and nothing after it.
            *e = if bin == cliff { 2048.0 * 1e-6 } else { 0.0 };
        }
        let fitted = fit_early_decay(&hist, fallback);
        assert!(fitted.is_finite() && fitted > 0.0, "{fitted}");
        assert!(
            f64::from(fitted) >= HIST_DT,
            "{fitted} against a bin of {HIST_DT}"
        );
    }

    /// Open ground with rays escaping over the rim: whatever the histogram
    /// of hits and escapes comes to, the reported early decay is a real
    /// EAX-range number — finite and clamped into 0.1..=8.0 s — never a
    /// NaN or a sentinel outside the band.
    #[test]
    fn an_open_ground_rooms_early_decay_stays_sane_where_rays_escape() {
        let mut ground = World::new();
        for x in -64..64 {
            for z in -64..64 {
                ground.set_block(x, -1, z, Material::Soil);
            }
        }
        let room = measure(&ground, Vec3::new(0.5, 1.6, 0.5), 2048, 8, 64.0, 7);
        for &early in &room.early_decay {
            assert!(early.is_finite(), "{room:?}");
            assert!((0.1..=8.0).contains(&early), "{room:?}");
        }
    }

    /// A sealed metal box at a high bounce cap: metal absorbs so little
    /// that rays reach the cap with per-band energy still riding (mid band
    /// ≥ 1e-6), so `Tally::add` parks it in the orphan bins and
    /// `Tally::room` continues it under the room's own decay before the
    /// fit. The early decay that comes out is sane, and the same seed
    /// measures the same room twice.
    #[test]
    fn a_sealed_metal_box_at_the_bounce_cap_keeps_a_sane_deterministic_early_decay() {
        let world = closed_box(Material::Metal, 12);
        let at = Vec3::new(6.0, 6.0, 6.0);
        let a = measure(&world, at, 2048, 64, 64.0, 5);
        let b = measure(&world, at, 2048, 64, 64.0, 5);
        assert_eq!(a, b);
        for &early in &a.early_decay {
            assert!(early.is_finite(), "{a:?}");
            assert!((0.1..=8.0).contains(&early), "{a:?}");
        }
    }

    /// A leg deposits its energy in the decay curve once, spread over the bins it covers: the shares of the bins a
    /// leg spans add up to the leg's time in bins, not to one whole bin per bin touched. The old count put the
    /// leg's whole energy in every bin it touched, so a leg ending where the next begins was counted twice in the
    /// shared bin, and short legs (small rooms, a listener against a wall) were boosted the most.
    #[test]
    fn a_leg_deposits_its_energy_in_the_bins_it_covers_once() {
        for length in [0.05, 0.343, 5.0] {
            let mut tally = Tally::new(1);
            // The ray's energy as it arrives at this leg: one from the listener, less after each surface crossed.
            tally.band_energy = [1.0; BANDS];
            tally.add(
                &Segment {
                    from: Vec3::new(0.0, 0.0, 0.0),
                    to: Vec3::new(0.0, 0.0, length),
                    energy: 1.0,
                    escaped: false,
                    material: Material::Rock,
                    fill: 1.0,
                    leaky: false,
                    bounce: 1,
                    normal: Vec3::new(0.0, 0.0, 1.0),
                    thickness: 1.0,
                },
                1,
            );
            let leg_bins = (length / SPEED_OF_SOUND) / HIST_DT;
            for (band, air) in AIR.iter().enumerate() {
                let deposited: f64 = tally.hist[band].iter().sum();
                // The leg's energy falls by the air's absorption over it, so
                // the total is the leg's time in bins, less that share.
                let air = (-air * length).exp();
                assert!(
                    (deposited - leg_bins * air).abs() < 1e-9,
                    "a {length} m leg in band {band} is {leg_bins} bins long but deposited {deposited} against {leg_bins} less the air's {air}"
                );
            }
        }
    }
}
