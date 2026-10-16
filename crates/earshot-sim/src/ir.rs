use earshot_world::{BANDS, Bands, Vec3};

use crate::{Rng, Room, SPEED_OF_SOUND};

/// One early reflection, as the listener hears it from a sound beside it:
/// from the wall it bounced off, after the trip there and back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reflection {
    /// Where it comes from, a unit vector in world coordinates.
    pub direction: Vec3,
    /// When it arrives, in seconds.
    pub delay: f32,
    /// Its energy relative to the direct sound at one block, mid band.
    pub energy: f32,
}

/// A sound's secondary source at a doorway's edge: the small share of the sound that
/// bends around the wall's vertical edge (Maekawa / ISO 9613-2 edge correction). Per-band
/// energy, relative to the direct sound at one block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeSource {
    /// Where it comes from, a unit vector in world coordinates.
    pub direction: Vec3,
    /// When it arrives, in seconds.
    pub delay: f32,
    /// Its per-band energy relative to the direct sound at one block.
    pub energy: Bands,
}

/// Channels in the impulse response: first-order ambisonics, FuMa order and
/// scaling (W, X, Y, Z), which is what `AL_FORMAT_BFORMAT3D_FLOAT32` takes.
pub const IR_CHANNELS: usize = 4;

/// The late field's directional channels against W: a diffuse field puts two
/// thirds of W's energy (with FuMa's 1/√2 on W) in each of X, Y and Z.
const DIFFUSE_XYZ: f32 = 0.816;
/// The most energy a response may carry, relative to the sound sent to it:
/// a room's reverb is never louder than the sound itself a block away.
/// (It was held to 0.08 while the responses were uploaded as 16-bit data by
/// mistake, which made every one a loud burst of noise whatever its level.)
pub const MAX_ENERGY: f32 = 1.0;
/// Fades the end in the last this many seconds, so a cut tail doesn't click.
const FADE: f32 = 0.05;
/// Crossovers between the low, mid and high bands, in hertz.
const LOW_MID: f32 = 500.0;
const MID_HIGH: f32 = 2000.0;

/// A room impulse response to convolve sounds with, for a sound next to the
/// listener: the early reflections from where the walls are, then a diffuse
/// tail decaying at the room's per-band rates. Interleaved W, X, Y, Z in
/// world axes (X towards -z, Y towards -x, Z up, as OpenAL's B-format has
/// them for a listener facing -z); the effect's orientation turns it with
/// the listener. At most `max_seconds` long, `sample_rate` samples a second.
pub fn impulse_response(
    room: &Room,
    early: &[Reflection],
    edge: &[EdgeSource],
    sample_rate: u32,
    max_seconds: f32,
    seed: u64,
) -> Vec<f32> {
    let rate = sample_rate as f32;
    // A response is cut at `max_seconds`: a room that rings longer has its
    // tail steepened to be 40 dB down by then, rather than stopping loud.
    let mut room = *room;
    room.decay = room.decay.map(|t60| t60.min(1.5 * max_seconds));
    room.early_decay = room.early_decay.map(|t60| t60.min(1.5 * max_seconds));
    let room = &room;
    // Long enough for the room's decay, and for the last echo to be heard.
    let last_echo = early
        .iter()
        .map(|r| r.delay + FADE + 0.02)
        .fold(0.0, f32::max);
    let length = (room.decay[1].max(last_echo).min(max_seconds).max(0.1) * rate) as usize;
    let mut out = vec![0.0f32; length * IR_CHANNELS];

    // The diffuse tail: three bands of decorrelated noise per channel, each
    // fading at its band's decay, starting after one mean free path.
    let onset = ((room.mean_free_path / SPEED_OF_SOUND) as f32 * rate) as usize;
    let level = late_energy(room).sqrt();
    // Two slopes: the tail falls at the traced early decay until the knee
    // — where that slope reaches 10 dB — then at the room's own decay. A
    // coupled room's fast antechamber gives way to the live space behind
    // it; equal slopes collapse to the one exponential this has always
    // been, exp(a) · exp(b) = exp(a + b).
    let early_t60 = room.early_decay.map(|t60| t60.max(0.05));
    let late_t60 = room.decay.map(|t60| t60.max(0.05));
    let knee: [f32; 3] = std::array::from_fn(|band| early_t60[band] / 6.0);
    let mut rng = Rng::new(seed);
    for channel in 0..IR_CHANNELS {
        let weight = if channel == 0 {
            std::f32::consts::FRAC_1_SQRT_2
        } else {
            DIFFUSE_XYZ * std::f32::consts::FRAC_1_SQRT_2
        };
        let mut bands = Bands3::new(rate);
        for i in onset.min(length)..length {
            let t = (i - onset) as f32 / rate;
            let noise = gaussian(&mut rng);
            let [low, mid, high] = bands.split(noise);
            let decays: [f32; 3] = std::array::from_fn(|band| {
                if t <= knee[band] {
                    (-6.907_755 * t / early_t60[band]).exp()
                } else {
                    (-6.907_755 * knee[band] / early_t60[band]).exp()
                        * (-6.907_755 * (t - knee[band]) / late_t60[band]).exp()
                }
            });
            out[i * IR_CHANNELS + channel] =
                level * weight * (low * decays[0] + mid * decays[1] + high * decays[2]);
        }
    }
    // The tail's noise has unit power per sample; scale it so its energy per
    // second of decay matches `late_energy`.
    let norm = (2.0 * 6.907_755 / (room.decay[1].max(0.05) * rate)).sqrt();
    for sample in &mut out {
        *sample *= norm;
    }

    // The early reflections, as taps from their directions.
    for reflection in &merge(early) {
        let i = (reflection.delay * rate) as usize;
        if i >= length {
            continue;
        }
        let a = reflection.energy.sqrt();
        let d = reflection.direction;
        let at = i * IR_CHANNELS;
        out[at] += a * std::f32::consts::FRAC_1_SQRT_2;
        out[at + 1] += a * -d.z as f32;
        out[at + 2] += a * -d.x as f32;
        out[at + 3] += a * d.y as f32;
    }

    // The edge sources: small taps at the wall's diffracting edges, in
    // the same form as the early reflections (broadband click, B-format
    // from the edge's direction). Per-band energy summed; the diffuse
    // tail provides the per-band decay. Fused with the direct sound by
    // `edge_sources()` itself (drop below EDGE_FUSE_SECONDS).
    for source in edge {
        let i = (source.delay * rate) as usize;
        if i >= length {
            continue;
        }
        let a = source.energy.iter().sum::<f32>().sqrt();
        let d = source.direction;
        let at = i * IR_CHANNELS;
        out[at] += a * std::f32::consts::FRAC_1_SQRT_2;
        out[at + 1] += a * -d.z as f32;
        out[at + 2] += a * -d.x as f32;
        out[at + 3] += a * d.y as f32;
    }

    // Energy in W is half the field's (FuMa's 1/√2).
    let energy = 2.0 * out.iter().step_by(IR_CHANNELS).map(|s| s * s).sum::<f32>();
    if energy > MAX_ENERGY {
        let scale = (MAX_ENERGY / energy).sqrt();
        for sample in &mut out {
            *sample *= scale;
        }
    }

    let fade = ((FADE * rate) as usize).min(length);
    for i in length - fade..length {
        let g = (length - i) as f32 / fade as f32;
        for channel in 0..IR_CHANNELS {
            out[i * IR_CHANNELS + channel] *= g;
        }
    }
    out
}

/// Within this long, reflections from about the same direction are one.
const MERGE_SECONDS: f32 = 0.001;

/// The early reflections as the reflections they are, not one per ray: the
/// rays that hit the same wall each carry a share of one reflection's
/// energy, and arrive together. Added as taps one by one, they would sum in
/// amplitude, not energy: at low frequencies, where they're in phase, a
/// thousand taps from one wall gave the response some 30 dB of bass. So each
/// reflection's share is summed by energy, per millisecond and direction
/// (which way along the axis it mostly comes from), into one tap at their
/// energy-weighted time, from their energy-weighted direction.
fn merge(early: &[Reflection]) -> Vec<Reflection> {
    let mut merged: std::collections::HashMap<(u32, u8), (f32, Vec3, f32)> =
        std::collections::HashMap::new();
    for r in early {
        if !(r.energy > 0.0 && r.delay.is_finite() && r.delay >= 0.0) {
            continue;
        }
        let d = r.direction;
        let axis = [d.x.abs(), d.y.abs(), d.z.abs()];
        let main: u8 = if axis[0] >= axis[1] && axis[0] >= axis[2] {
            0
        } else if axis[1] >= axis[2] {
            1
        } else {
            2
        };
        let sign = u8::from([d.x, d.y, d.z][usize::from(main)] < 0.0);
        let key = ((r.delay / MERGE_SECONDS) as u32, main * 2 + sign);
        let entry = merged.entry(key).or_insert((0.0, Vec3::ZERO, 0.0));
        entry.0 += r.energy;
        entry.1 = entry.1 + d * f64::from(r.energy);
        entry.2 += r.delay * r.energy;
    }
    merged
        .into_iter()
        .map(|(_, (energy, direction, delay))| Reflection {
            direction: direction.normalized(),
            delay: delay / energy,
            energy,
        })
        .collect()
}

/// The reverberant energy relative to the direct sound at one block. The
/// diffuse field holds the power going in for as long as it lingers: the
/// mean free path over the share each reflection takes (escapes included)
/// is how far sound goes before it's gone, so the energy is 4π·(that) / V
/// against the direct sound's at a block. In a closed room this is Sabine's
/// 16π/R; in the open, where V is huge (the rays escape), next to nothing,
/// however near the ground is. With no volume measured, a roughly cubic space
/// of side 1.5 × the mean free path, scaled by how enclosed it is.
pub fn late_energy(room: &Room) -> f32 {
    if room.mean_free_path <= 0.0 {
        return 0.0;
    }
    // Absorption back from the mid band's decay (Eyring, inverted).
    let t60 = f64::from(room.decay[1].max(0.05));
    let a = 1.0 - (-13.815_510_557_964_274 * room.mean_free_path / (SPEED_OF_SOUND * t60)).exp();
    let a = a.clamp(1e-3, 0.999);
    if room.volume > 0.0 {
        let lingers = room.mean_free_path / a;
        return (4.0 * std::f64::consts::PI * lingers / room.volume).clamp(0.0, 4.0) as f32;
    }
    let side = 1.5 * room.mean_free_path;
    let surface = 6.0 * side * side;
    let r = surface * a / (1.0 - a);
    let energy = (16.0 * std::f64::consts::PI / r).clamp(0.0, 4.0) as f32;
    energy * room.enclosure * room.enclosure
}

/// A unit-variance Gaussian sample (Box–Muller).
fn gaussian(rng: &mut Rng) -> f32 {
    let u = rng.next_f64().max(1e-12);
    let v = rng.next_f64();
    ((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()) as f32
}

/// Splits a signal into low, mid and high bands at the crossovers, each
/// low-pass two one-pole stages (12 dB per octave); the three add back to
/// the input.
struct Bands3 {
    low_coefficient: f32,
    high_coefficient: f32,
    low: [f32; 2],
    below_high: [f32; 2],
}

impl Bands3 {
    fn new(rate: f32) -> Self {
        let coefficient = |hz: f32| 1.0 - (-std::f32::consts::TAU * hz / rate).exp();
        Bands3 {
            low_coefficient: coefficient(LOW_MID),
            high_coefficient: coefficient(MID_HIGH),
            low: [0.0; 2],
            below_high: [0.0; 2],
        }
    }

    fn split(&mut self, x: f32) -> [f32; BANDS] {
        let low = low_pass(&mut self.low, self.low_coefficient, x);
        let below_high = low_pass(&mut self.below_high, self.high_coefficient, x);
        [low, below_high - low, x - below_high]
    }
}

/// Two one-pole low-pass stages in a row.
fn low_pass(stages: &mut [f32; 2], coefficient: f32, x: f32) -> f32 {
    let mut value = x;
    for stage in stages {
        *stage += coefficient * (value - *stage);
        value = *stage;
    }
    value
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn many_rays_off_one_wall_dont_boost_the_bass() {
        let room = Room {
            decay: [2.0, 2.0, 1.0],
            early_decay: [2.0, 2.0, 1.0],
            mean_free_path: 6.0,
            enclosure: 1.0,
            first_reflection: 1.0,
            scattering: 0.5,
            volume: 0.0,
        };
        // 4000 rays' shares of one reflection off a wall a block away.
        let early: Vec<Reflection> = (0..4000)
            .map(|i| Reflection {
                direction: Vec3::new(1.0, 0.001 * (i % 7) as f64, 0.0).normalized(),
                delay: 0.0058 + 0.000_000_1 * (i % 13) as f32,
                energy: 0.2 / 4000.0,
            })
            .collect();
        let ir = impulse_response(&room, &early, &[], 48_000, 1.5, 1);
        let w: Vec<f32> = ir.iter().step_by(IR_CHANNELS).copied().collect();
        let energy: f32 = w.iter().map(|s| s * s).sum();
        // The response's gain at 0 Hz against its overall (energy) gain.
        let dc: f32 = w.iter().sum::<f32>().abs();
        assert!(dc < 4.0 * energy.sqrt(), "bass {dc} vs {}", energy.sqrt());
    }
}

#[cfg(test)]
mod tests {
    use earshot_world::Material;

    use super::*;

    fn room(t60: [f32; 3], mean_free_path: f64, enclosure: f32) -> Room {
        Room {
            decay: t60,
            early_decay: t60,
            mean_free_path,
            enclosure,
            first_reflection: mean_free_path / 2.0,
            scattering: 0.3,
            volume: 0.0,
        }
    }

    /// Energy of channel 0 in successive windows of `window` samples.
    fn envelope(ir: &[f32], window: usize) -> Vec<f32> {
        ir.chunks(window * IR_CHANNELS)
            .map(|chunk| chunk.iter().step_by(IR_CHANNELS).map(|s| s * s).sum())
            .collect()
    }

    #[test]
    fn the_tail_decays_at_the_rooms_rate() {
        let rate = 48_000;
        let ir = impulse_response(&room([1.0, 1.0, 1.0], 8.0, 1.0), &[], &[], rate, 2.0, 1);
        assert_eq!(ir.len(), rate as usize * IR_CHANNELS);
        // 60 dB in a second: about 6 dB per 100 ms window.
        let windows = envelope(&ir, 4800);
        let drop = 10.0 * (windows[2] / windows[6]).log10();
        assert!((drop - 24.0).abs() < 4.0, "{drop} dB over 400 ms");
    }

    #[test]
    fn highs_die_first_when_the_room_absorbs_them() {
        let rate = 48_000;
        let ir = impulse_response(&room([2.0, 1.0, 0.3], 8.0, 1.0), &[], &[], rate, 1.0, 2);
        // Low and high band energy of W early (50–250 ms) and late (500–900 ms).
        let mut bands = Bands3::new(rate as f32);
        let mut energy = [[0.0f32; 2]; 2];
        for (i, &w) in ir.iter().step_by(IR_CHANNELS).enumerate() {
            let [low, _, high] = bands.split(w);
            let t = i as f32 / rate as f32;
            let window = if (0.05..0.25).contains(&t) {
                0
            } else if (0.5..0.9).contains(&t) {
                1
            } else {
                continue;
            };
            energy[window][0] += low * low;
            energy[window][1] += high * high;
        }
        let low_fall = energy[0][0] / energy[1][0];
        let high_fall = energy[0][1] / energy[1][1];
        assert!(
            high_fall > 4.0 * low_fall,
            "low fell {low_fall}×, high {high_fall}×"
        );
    }

    #[test]
    fn a_late_echo_makes_the_response_long_enough_to_hold_it() {
        // Open ground decays in a tenth of a second, but a cliff answers later.
        let echo = Reflection {
            direction: Vec3::new(1.0, 0.0, 0.0),
            delay: 0.4,
            energy: 0.01,
        };
        let ir = impulse_response(&room([0.1; 3], 4.0, 0.2), &[echo], &[], 48_000, 1.5, 4);
        let at = (0.4 * 48_000.0) as usize * IR_CHANNELS;
        assert!(
            ir.len() > at + IR_CHANNELS,
            "cut at {} samples",
            ir.len() / IR_CHANNELS
        );
        assert!(ir[at + 2] < 0.0, "the echo is there, from +x");
    }

    #[test]
    fn the_tail_sits_at_sabines_level_for_a_shoebox() {
        // Sabine's reverberant level — 16π/R, R = S·a/(1 − a) — against a
        // measured rock box: the response's absolute level was pinned only
        // to itself before this. The old 4× air bug inflated the absorption
        // the decay implies and dropped this ratio to 0.65.
        let size = 12;
        let mut world = earshot_world::World::new();
        for a in -1..=size {
            for b in -1..=size {
                for wall in [-1, size] {
                    world.set_block(wall, a, b, Material::Rock);
                    world.set_block(a, wall, b, Material::Rock);
                    world.set_block(a, b, wall, Material::Rock);
                }
            }
        }
        let room = crate::measure(&world, Vec3::new(6.0, 6.0, 6.0), 2048, 24, 64.0, 1);
        assert_eq!(room.enclosure, 1.0);
        let a = f64::from(Material::Rock.acoustics().absorption[1]);
        let surface = 6.0 * f64::from(size).powi(2);
        let sabine = 16.0 * std::f64::consts::PI / (surface * a / (1.0 - a));
        let ratio = f64::from(late_energy(&room)) / sabine;
        assert!(
            (0.75..=1.25).contains(&ratio),
            "the tail is {ratio} of Sabine's {sabine} s level"
        );
    }

    #[test]
    fn the_tail_carries_the_rooms_diffuse_energy() {
        // Short enough to fit the response whole, so none is cut off.
        for (t60, mfp) in [(0.6, 3.0), (0.8, 8.0), (1.0, 20.0)] {
            let r = room([t60; 3], mfp, 1.0);
            let ir = impulse_response(&r, &[], &[], 48_000, 1.5, 1);
            // Energy in W is half the field's (FuMa's 1/√2).
            let energy = 2.0 * ir.iter().step_by(IR_CHANNELS).map(|s| s * s).sum::<f32>();
            let wanted = late_energy(&r).min(MAX_ENERGY);
            assert!(
                (energy / wanted - 1.0).abs() < 0.25,
                "{energy} vs {wanted} for t60 {t60}, mfp {mfp}"
            );
        }
    }

    #[test]
    fn early_reflections_land_where_and_when_they_should() {
        let reflection = Reflection {
            direction: Vec3::new(1.0, 0.0, 0.0),
            delay: 0.01,
            energy: 0.25,
        };
        let ir = impulse_response(
            &room([0.5, 0.5, 0.5], 4.0, 1.0),
            &[reflection],
            &[],
            48_000,
            1.0,
            3,
        );
        let at = 480 * IR_CHANNELS;
        // From +x: in B-format that is Y = -1 (to the right).
        let [_, x, y, z] = [ir[at], ir[at + 1], ir[at + 2], ir[at + 3]];
        assert!(
            y < 0.0 && y.abs() > 4.0 * x.abs().max(z.abs()),
            "{:?}",
            &ir[at..at + 4]
        );
    }

    #[test]
    fn open_air_has_no_tail_and_enclosed_halls_have_a_loud_one() {
        assert_eq!(late_energy(&Room::OPEN_AIR), 0.0);
        let hall = late_energy(&room([3.0, 3.0, 3.0], 20.0, 1.0));
        let field = late_energy(&room([3.0, 3.0, 3.0], 20.0, 0.5));
        assert!(hall > 0.0 && field < hall / 3.0);
        let bands = Bands3::new(48_000.0).split(1.0);
        assert!((bands.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_same_seed_makes_the_same_response_and_a_different_one_rebalances() {
        let r = room([1.5, 1.0, 0.6], 6.0, 1.0);
        let a = impulse_response(&r, &[], &[], 48_000, 1.5, 5);
        let b = impulse_response(&r, &[], &[], 48_000, 1.5, 5);
        assert_eq!(a, b);
        let c = impulse_response(&r, &[], &[], 48_000, 1.5, 6);
        assert_ne!(a, c);
        let energy = |ir: &[f32]| 2.0 * ir.iter().step_by(IR_CHANNELS).map(|s| s * s).sum::<f32>();
        assert!(
            (energy(&a) / energy(&c) - 1.0).abs() < 0.1,
            "{} vs {}",
            energy(&a),
            energy(&c)
        );
    }

    #[test]
    fn every_sample_is_finite_the_length_bounded_and_the_end_faded() {
        let r = room([2.0, 1.0, 0.5], 8.0, 1.0);
        let ir = impulse_response(&r, &[], &[], 48_000, 1.5, 1);
        assert!(ir.iter().all(|s| s.is_finite()));
        // The mid decay of a second sets the length.
        assert_eq!(ir.len(), 48_000 * IR_CHANNELS);
        let short = impulse_response(&r, &[], &[], 48_000, 0.25, 1);
        assert_eq!(short.len(), 12_000 * IR_CHANNELS);
        // The fade-out at the end lands near silence.
        let peak = ir.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        let last = ir[ir.len() - IR_CHANNELS];
        assert!(last.abs() < 2e-3 * peak, "{last} after a peak of {peak}");
    }

    #[test]
    fn a_tail_bigger_than_its_room_is_held_to_max_energy() {
        let r = Room {
            decay: [4.0; 3],
            early_decay: [4.0; 3],
            mean_free_path: 10.0,
            enclosure: 1.0,
            first_reflection: 5.0,
            scattering: 0.3,
            volume: 1.0,
        };
        assert!(late_energy(&r) >= 4.0, "the formula caps at 4 itself");
        let ir = impulse_response(&r, &[], &[], 48_000, 1.5, 1);
        let energy = 2.0 * ir.iter().step_by(IR_CHANNELS).map(|s| s * s).sum::<f32>();
        assert!(energy <= MAX_ENERGY * 1.000_1, "{energy}");
    }

    #[test]
    fn each_band_of_the_tail_decays_at_its_own_measured_rate() {
        let rate = 48_000.0;
        let r = room([3.0, 1.5, 0.75], 8.0, 1.0);
        let ir = impulse_response(&r, &[], &[], 48_000, 1.5, 8);
        // Band energies of W early (100–200 ms) and late (600–700 ms); the
        // windows are half a second apart.
        let mut bands = Bands3::new(rate);
        let mut windows = [[0.0f32; 3]; 2];
        for (i, &w) in ir.iter().step_by(IR_CHANNELS).enumerate() {
            let t = i as f32 / rate;
            let window = if (0.1..0.2).contains(&t) {
                0
            } else if (0.6..0.7).contains(&t) {
                1
            } else {
                continue;
            };
            for (energy, band) in windows[window].iter_mut().zip(bands.split(w)) {
                *energy += band * band;
            }
        }
        let measured: Vec<f32> = r
            .decay
            .iter()
            .enumerate()
            .map(|(band, _)| {
                let drop = 10.0 * (windows[0][band] / windows[1][band]).log10();
                60.0 * 0.5 / drop
            })
            .collect();
        for (measured_t60, t60) in measured.iter().zip(r.decay) {
            // The split's 12 dB/oct crossovers leak the neighbouring bands'
            // rates into each measurement, so half the rate is as close as
            // this method gets; the ordering below is the sharp check.
            assert!(
                (measured_t60 / t60 - 1.0).abs() < 0.5,
                "{measured_t60} against {t60}"
            );
        }
        assert!(
            measured[0] >= measured[1] && measured[1] >= measured[2],
            "each band decays at its own rate: {measured:?}"
        );
    }

    #[test]
    fn the_tail_falls_steeply_for_the_early_decay_then_settles() {
        let rate = 48_000.0;
        let r = Room {
            decay: [2.0; 3],
            early_decay: [0.3; 3],
            mean_free_path: 8.0,
            enclosure: 1.0,
            first_reflection: 4.0,
            scattering: 0.3,
            volume: 0.0,
        };
        let ir = impulse_response(&r, &[], &[], 48_000, 1.5, 8);
        let window = |from: f32, to: f32| {
            let mut energy = 0.0f32;
            for (i, &sample) in ir.iter().step_by(IR_CHANNELS).enumerate() {
                let t = i as f32 / rate;
                if t >= to {
                    break;
                }
                if t >= from {
                    energy += sample * sample;
                }
            }
            energy
        };
        // The tail starts at the mean free path (8/343 ≈ 23 ms). Under the
        // 0.3 s early decay it loses 10 dB by 50 ms, so these windows sit
        // well under 0.4; at the old single 2 s rate the pair would sit
        // near 0.8 instead.
        let steep = window(0.030, 0.040);
        let knee = window(0.060, 0.075);
        assert!(knee < 0.4 * steep, "{knee} against {steep}");
        // Past the knee the late rate rules: the windows are half a second
        // apart, so their drop extrapolates to the 2 s decay.
        let drop = 10.0 * (window(0.40, 0.50) / window(0.90, 1.00)).log10();
        let measured = 60.0 * 0.5 / drop;
        assert!((measured / 2.0 - 1.0).abs() < 0.5, "{measured}");
    }

    /// The knee in the inverted sign: `early_decay` 2 s over `decay` 0.5 s
    /// — the traced early rate slower than the room's own. The tail falls
    /// gently to the knee at 2/6 ≈ 333 ms (where that slope reaches 10 dB),
    /// then steeply at the room's rate, continuous across the knee and
    /// never rising, whatever the sign of early − decay.
    #[test]
    fn an_early_decay_slower_than_the_rooms_still_falls_over_the_knee() {
        let r = Room {
            decay: [0.5; 3],
            early_decay: [2.0; 3],
            mean_free_path: 8.0,
            enclosure: 1.0,
            first_reflection: 4.0,
            scattering: 0.3,
            volume: 0.0,
        };
        let ir = impulse_response(&r, &[], &[], 48_000, 1.5, 8);
        assert!(ir.iter().all(|s| s.is_finite()));
        // W-channel energy in 50 ms windows, from just past the tail's
        // onset (8/343 ≈ 23 ms) to the response's end: nine windows, four
        // of them under the 333 ms knee and the rest past it. Noise's
        // share of each window is ~3% while the envelope itself falls 30%
        // per window before the knee and 75% after it, so a single rise
        // would be the envelope, not the sampling.
        let samples = ir.len() / IR_CHANNELS;
        let start = (0.03 * 48_000.0) as usize;
        let window = (0.05 * 48_000.0) as usize;
        let windows: Vec<f32> = (start..samples)
            .step_by(window)
            .map(|from| {
                let to = (from + window).min(samples);
                ir[from * IR_CHANNELS..to * IR_CHANNELS]
                    .iter()
                    .step_by(IR_CHANNELS)
                    .map(|s| s * s)
                    .sum()
            })
            .collect();
        assert!(windows.len() >= 8, "{} windows", windows.len());
        for pair in windows.windows(2) {
            assert!(
                pair[1] <= pair[0],
                "the envelope rose across the knee: {windows:?}"
            );
        }
        // The response stays inside the energy envelope the other tests
        // hold it to: under MAX_ENERGY even with the slow early slope
        // lingering on top of the room's rate.
        let energy = 2.0 * ir.iter().step_by(IR_CHANNELS).map(|s| s * s).sum::<f32>();
        assert!(energy <= MAX_ENERGY * 1.000_1, "{energy}");
        let wanted = late_energy(&r);
        // The slow early slope lingers: the tail's norm is scaled to the
        // room's own 0.5 s decay, so this two-slope response carries a
        // few times the single-rate figure — measured 3.69× — and the
        // MAX_ENERGY rescale still never engages.
        assert!(
            (3.0..=4.5).contains(&(energy / wanted)),
            "{energy} against the room's {wanted}"
        );
    }

    /// `impulse_response` clamps `early_decay` to 1.5 × its own
    /// `max_seconds` before shaping the tail: an absurd input must come
    /// out identical to one already at the clamp, not flatten the whole
    /// response into a roar for the MAX_ENERGY rescale to crush.
    #[test]
    fn an_absurd_early_decay_is_clamped_to_the_responses_horizon() {
        let base = Room {
            decay: [1.0; 3],
            early_decay: [1.0; 3],
            mean_free_path: 8.0,
            enclosure: 1.0,
            first_reflection: 4.0,
            scattering: 0.3,
            volume: 0.0,
        };
        let max_seconds = 1.0;
        let at_the_clamp = Room {
            early_decay: [1.5 * max_seconds; 3],
            ..base
        };
        let extreme = Room {
            early_decay: [f32::MAX; 3],
            ..base
        };
        let held = impulse_response(&extreme, &[], &[], 48_000, max_seconds, 9);
        let clamped = impulse_response(&at_the_clamp, &[], &[], 48_000, max_seconds, 9);
        assert!(held.iter().all(|s| s.is_finite()));
        assert_eq!(held, clamped);
        let energy = 2.0 * held.iter().step_by(IR_CHANNELS).map(|s| s * s).sum::<f32>();
        assert!(energy <= MAX_ENERGY * 1.000_1, "{energy}");
    }

    #[test]
    fn reflections_ahead_and_above_land_on_their_channels() {
        // B-format with the listener facing -z: what is ahead is +X, what is
        // above is +Z (the +x test, on Y, is above).
        let ahead = Reflection {
            direction: Vec3::new(0.0, 0.0, -1.0),
            delay: 0.02,
            energy: 0.25,
        };
        let above = Reflection {
            direction: Vec3::new(0.0, 1.0, 0.0),
            delay: 0.03,
            energy: 0.25,
        };
        let ir = impulse_response(
            &room([0.5; 3], 4.0, 1.0),
            &[ahead, above],
            &[],
            48_000,
            1.0,
            3,
        );
        let tap = |delay: f32| {
            let at = (delay * 48_000.0) as usize * IR_CHANNELS;
            [ir[at], ir[at + 1], ir[at + 2], ir[at + 3]]
        };
        let ahead = tap(0.02);
        // W is swamped by the diffuse tail's noise; the directional channels
        // against each other say where it comes from.
        assert!(
            ahead[1] > 4.0 * ahead[2].abs().max(ahead[3].abs()),
            "ahead is X: {ahead:?}"
        );
        let above = tap(0.03);
        assert!(
            above[3] > 4.0 * above[1].abs().max(above[2].abs()),
            "above is Z: {above:?}"
        );
    }

    #[test]
    fn impulse_response_with_edge_sources_adds_a_tap() {
        // Doorway geometry: a 3x3 rock wall with a doorway at (4, 1, 0).
        // The first reflection is the image source off x=4; the edge
        // sources are the upper vertical edges of the wall block at
        // (4, 0, 0), visible through the doorway. The same geometry the
        // early.rs tests use, so the test setup is known to produce at
        // least one edge.
        use crate::early::{early_taps, edge_sources};
        let mut world = earshot_world::World::new();
        for y in 0..3 {
            for z in 0..3 {
                if y == 1 && z == 0 {
                    continue;
                }
                world.set_block(4, y, z, earshot_world::Material::Rock);
            }
        }
        let listener = Vec3::new(-2.0, 0.5, 0.5);
        let source = Vec3::new(2.0, 0.5, 0.5);
        let room = Room {
            decay: [0.1; 3],
            early_decay: [0.1; 3],
            mean_free_path: 1.0,
            enclosure: 1.0,
            first_reflection: 1.0,
            scattering: 0.3,
            volume: 0.0,
        };
        let taps = early_taps(&world, listener, source);
        let edges = edge_sources(&world, listener, source, &taps);
        assert!(
            !edges.is_empty(),
            "test setup: doorway should have at least one edge source"
        );
        let with_edges = impulse_response(&room, &taps, &edges, 48_000, 0.05, 1);
        let without_edges = impulse_response(&room, &taps, &[], 48_000, 0.05, 1);
        // Sum W channel energy in a 1 ms window around the first edge's
        // delay. With edges the window has the edge's tap; without, it
        // has only the diffuse tail (which is small at 0.02 s with a
        // 100 ms decay).
        let edge_delay = edges[0].delay;
        let window_start = ((edge_delay - 0.0005).max(0.0) * 48_000.0) as usize;
        let window_end = ((edge_delay + 0.0005) * 48_000.0) as usize;
        let energy = |ir: &[f32]| -> f32 {
            let start = window_start * IR_CHANNELS;
            let end = (window_end + 1) * IR_CHANNELS;
            ir[start..end.min(ir.len())]
                .iter()
                .step_by(IR_CHANNELS)
                .map(|x| x * x)
                .sum()
        };
        let with = energy(&with_edges);
        let without = energy(&without_edges);
        assert!(
            with > without,
            "with edges should add energy at the edge delay: {with} vs {without}"
        );
    }
}
