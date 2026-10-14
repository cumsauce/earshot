use crate::{Room, SPEED_OF_SOUND};

/// EFX's EAX reverb parameters (`AL_EAXREVERB_*`), each within its range in
/// `efx.h`. The reflection and late reverb pans are left centred.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EaxReverb {
    pub density: f32,
    pub diffusion: f32,
    pub gain: f32,
    pub gain_hf: f32,
    pub gain_lf: f32,
    pub decay_time: f32,
    pub decay_hf_ratio: f32,
    pub decay_lf_ratio: f32,
    pub reflections_gain: f32,
    pub reflections_delay: f32,
    pub late_reverb_gain: f32,
    pub late_reverb_delay: f32,
    pub echo_time: f32,
    pub echo_depth: f32,
}

impl EaxReverb {
    /// The fields in a fixed order, for the C ABI.
    pub fn to_array(self) -> [f32; 14] {
        [
            self.density,
            self.diffusion,
            self.gain,
            self.gain_hf,
            self.gain_lf,
            self.decay_time,
            self.decay_hf_ratio,
            self.decay_lf_ratio,
            self.reflections_gain,
            self.reflections_delay,
            self.late_reverb_gain,
            self.late_reverb_delay,
            self.echo_time,
            self.echo_depth,
        ]
    }
}

/// Overall reverb level when fully shut in: EFX's default room level.
const SHUT_IN_GAIN: f32 = 0.32;
/// A mean free path this long (blocks) or more gives full modal density.
const DENSE_PATH: f64 = 16.0;
/// Outdoors the tail is short whatever the rays found bouncing among trunks
/// and leaves, seconds: a wood doesn't ring as a room does.
const OPEN_DECAY: f32 = 0.6;
/// Spaces whose mean free path passes this (blocks) start to echo.
const ECHO_PATH: f64 = 20.0;

/// EAX reverb settings for `room`. The decay and its high and low ratios
/// come straight from the measured per-band decays. The level grows with
/// how enclosed the listener is (squared, so open ground is nearly dry).
/// Early reflections come sooner and louder the nearer the walls, the late
/// reverb after one mean free path, and a big enclosed space echoes.
pub fn eax_reverb(room: &Room) -> EaxReverb {
    let [low, mid, high] = room.decay;
    let enclosure = room.enclosure.clamp(0.0, 1.0);
    let echo = ((room.mean_free_path - ECHO_PATH) / 60.0).clamp(0.0, 0.5) as f32 * enclosure;
    EaxReverb {
        density: (room.mean_free_path / DENSE_PATH).clamp(0.05, 1.0) as f32,
        diffusion: (0.3 + room.scattering).clamp(0.0, 1.0),
        gain: SHUT_IN_GAIN * enclosure * enclosure,
        gain_hf: 0.89,
        gain_lf: 1.0,
        // Shut in, the room's own decay; under open sky, drawn towards a short
        // outdoor tail (never longer than measured).
        decay_time: (mid * enclosure + OPEN_DECAY * (1.0 - enclosure))
            .min(mid)
            .clamp(0.1, 20.0),
        decay_hf_ratio: (high / mid).clamp(0.1, 2.0),
        decay_lf_ratio: (low / mid).clamp(0.1, 2.0),
        reflections_gain: (enclosure / (1.0 + room.first_reflection as f32 / 8.0)).clamp(0.0, 3.16),
        reflections_delay: (2.0 * room.first_reflection / SPEED_OF_SOUND).clamp(0.0, 0.3) as f32,
        late_reverb_gain: (1.26 * enclosure).clamp(0.0, 10.0),
        late_reverb_delay: (room.mean_free_path / SPEED_OF_SOUND).clamp(0.0, 0.1) as f32,
        echo_time: (2.0 * room.mean_free_path / SPEED_OF_SOUND).clamp(0.075, 0.25) as f32,
        echo_depth: echo,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(decay: [f32; 3], mean_free_path: f64, enclosure: f32) -> Room {
        Room {
            decay,
            mean_free_path,
            enclosure,
            first_reflection: mean_free_path / 2.0,
            scattering: 0.3,
            volume: 0.0,
        }
    }

    #[test]
    fn every_value_is_in_range() {
        for r in [
            Room::OPEN_AIR,
            room([0.5, 0.4, 0.2], 3.0, 1.0),
            room([30.0, 25.0, 40.0], 200.0, 1.0),
            room([1.0, 1.0, 1.0], 10.0, 0.5),
        ] {
            let e = eax_reverb(&r);
            let ranges = [
                (e.density, 0.0, 1.0),
                (e.diffusion, 0.0, 1.0),
                (e.gain, 0.0, 1.0),
                (e.decay_time, 0.1, 20.0),
                (e.decay_hf_ratio, 0.1, 2.0),
                (e.decay_lf_ratio, 0.1, 2.0),
                (e.reflections_gain, 0.0, 3.16),
                (e.reflections_delay, 0.0, 0.3),
                (e.late_reverb_gain, 0.0, 10.0),
                (e.late_reverb_delay, 0.0, 0.1),
                (e.echo_time, 0.075, 0.25),
                (e.echo_depth, 0.0, 1.0),
            ];
            for (value, min, max) in ranges {
                assert!(
                    (min..=max).contains(&value),
                    "{value} outside {min}..{max}: {e:?}"
                );
            }
        }
    }

    #[test]
    fn open_ground_is_nearly_dry_and_a_closed_hall_rings() {
        let open = eax_reverb(&room([0.3, 0.2, 0.15], 6.0, 0.5));
        let hall = eax_reverb(&room([3.0, 2.5, 1.5], 25.0, 1.0));
        assert!(open.gain < hall.gain / 3.0);
        assert!(hall.decay_time == 2.5 && hall.decay_hf_ratio < 1.0 && hall.decay_lf_ratio > 1.0);
        assert!(hall.echo_depth > 0.0 && open.echo_depth == 0.0);
        assert!(hall.late_reverb_delay > open.late_reverb_delay);
    }
}
