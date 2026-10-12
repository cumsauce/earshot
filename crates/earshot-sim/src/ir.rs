use earshot_world::{BANDS, Vec3};

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

/// Channels in the impulse response: first-order ambisonics, FuMa order and
/// scaling (W, X, Y, Z), which is what `AL_FORMAT_BFORMAT3D_FLOAT32` takes.
pub const IR_CHANNELS: usize = 4;

/// The late field's directional channels against W: a diffuse field puts two
/// thirds of W's energy (with FuMa's 1/√2 on W) in each of X, Y and Z.
const DIFFUSE_XYZ: f32 = 0.816;
/// The most energy a response may carry, relative to the sound sent to it:
/// what the EAX reverb gives a shut-in room (its 0.32 gain, about -8 dB in
/// all). The diffuse-field level is right for a sound a block away, but a
/// small reverberant room comes out at twice the input's energy, and every
/// sound sends to the same reverb, so the convolution clipped.
pub const MAX_ENERGY: f32 = 0.16;
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
    sample_rate: u32,
    max_seconds: f32,
    seed: u64,
) -> Vec<f32> {
    let rate = sample_rate as f32;
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
            let decays = room.decay.map(|t60| (-6.907_755 * t / t60.max(0.05)).exp());
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
    for reflection in early {
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

    // Energy in W is half the field's (FuMa's 1/sqrt 2).
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

/// The reverberant energy relative to the direct sound at one block: the
/// diffuse field's `16π / R`, with the room constant `R = S·a / (1 - a)`.
/// The surface comes from the mean free path (a roughly cubic space of side
/// 1.5 × the mean free path), and the result is scaled by how enclosed the
/// listener is, since the open sky returns nothing.
pub fn late_energy(room: &Room) -> f32 {
    if room.mean_free_path <= 0.0 {
        return 0.0;
    }
    let side = 1.5 * room.mean_free_path;
    let surface = 6.0 * side * side;
    // Absorption back from the mid band's decay (Eyring, inverted).
    let t60 = f64::from(room.decay[1].max(0.05));
    let a = 1.0 - (-13.815_510_557_964_274 * room.mean_free_path / (SPEED_OF_SOUND * t60)).exp();
    let a = a.clamp(1e-3, 0.999);
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
mod tests {
    use super::*;

    fn room(t60: [f32; 3], mean_free_path: f64, enclosure: f32) -> Room {
        Room {
            decay: t60,
            mean_free_path,
            enclosure,
            first_reflection: mean_free_path / 2.0,
            scattering: 0.3,
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
        let ir = impulse_response(&room([1.0, 1.0, 1.0], 8.0, 1.0), &[], rate, 2.0, 1);
        assert_eq!(ir.len(), rate as usize * IR_CHANNELS);
        // 60 dB in a second: about 6 dB per 100 ms window.
        let windows = envelope(&ir, 4800);
        let drop = 10.0 * (windows[2] / windows[6]).log10();
        assert!((drop - 24.0).abs() < 4.0, "{drop} dB over 400 ms");
    }

    #[test]
    fn highs_die_first_when_the_room_absorbs_them() {
        let rate = 48_000;
        let ir = impulse_response(&room([2.0, 1.0, 0.3], 8.0, 1.0), &[], rate, 1.0, 2);
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
        let ir = impulse_response(&room([0.1; 3], 4.0, 0.2), &[echo], 48_000, 1.5, 4);
        let at = (0.4 * 48_000.0) as usize * IR_CHANNELS;
        assert!(
            ir.len() > at + IR_CHANNELS,
            "cut at {} samples",
            ir.len() / IR_CHANNELS
        );
        assert!(ir[at + 2] < 0.0, "the echo is there, from +x");
    }

    #[test]
    fn no_response_is_louder_than_the_eax_reverb() {
        // A small hard room is the worst: its diffuse field is twice the
        // input's energy before the cap.
        for (t60, mfp) in [(3.0, 5.0), (0.6, 3.0), (3.0, 20.0)] {
            let early = [Reflection {
                direction: Vec3::new(1.0, 0.0, 0.0),
                delay: 0.005,
                energy: 0.5,
            }];
            let ir = impulse_response(&room([t60; 3], mfp, 1.0), &early, 48_000, 1.5, 1);
            let energy = 2.0 * ir.iter().step_by(IR_CHANNELS).map(|s| s * s).sum::<f32>();
            assert!(
                energy <= MAX_ENERGY * 1.01,
                "{energy} for t60 {t60}, mfp {mfp}"
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
}
