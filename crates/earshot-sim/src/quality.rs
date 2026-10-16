use std::time::Duration;

/// Which curve diffraction round an edge follows, when sound takes the long
/// way round an obstacle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiffractionStandard {
    /// ISO 9613-2's barrier attenuation, `10 log10(3 + 20·detour/λ)` dB,
    /// capped at 25 dB. The default.
    #[default]
    Iso9613_2,
    /// HJ 2.4-2021's Maekawa curve: `min(24, 13·N^0.187)` dB, where
    /// `N = 2·detour/λ` is Maekawa's Fresnel number. About 3 dB steeper than
    /// ISO around `N = 1`, so a doorway lets a little more sound round at
    /// grazing angles, and a lot less at deep shadow.
    Hj24,
}

/// How much the engine traces: what a preset sets (docs/PLAN.md,
/// "Performance budget").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quality {
    /// Rays per measurement of the room around the listener, at most; 0
    /// measures none, so there is no reverb.
    pub room_rays: u32,
    /// Bounces per ray.
    pub room_bounces: u32,
    /// How often the room is measured.
    pub room_every: Duration,
    /// Rays per source through what is in the way: 1, 5 or 9 (more blur
    /// block edges more smoothly); 0 turns occlusion off.
    pub occlusion_rays: u32,
    /// How far the pathing field reaches each way from the listener, in
    /// blocks; 0 turns sound round corners off.
    pub field_half: i32,
    /// How far room rays reach, in blocks: past it they count as escaped.
    /// Larger where the world has a ceiling (the Nether's caverns).
    pub room_reach: f64,
    /// The share of one core the room measurement may take on average.
    /// Above it, the measurement uses fewer rays, down to a sixteenth.
    pub budget: f32,
    /// Which diffraction curve the way round an edge follows.
    pub diffraction: DiffractionStandard,
}

impl Quality {
    /// What the engine starts with, before the game sets a preset.
    pub const DEFAULT: Quality = Quality {
        room_rays: 1024,
        room_bounces: 12,
        room_every: Duration::from_millis(200),
        occlusion_rays: 5,
        field_half: 24,
        room_reach: 64.0,
        budget: 1.0,
        diffraction: DiffractionStandard::Iso9613_2,
    };

    /// Whether every value is one the engine can work with.
    pub fn is_valid(&self) -> bool {
        self.room_rays <= 1 << 16
            && self.room_bounces <= 64
            && (Duration::from_millis(20)..=Duration::from_secs(5)).contains(&self.room_every)
            && matches!(self.occlusion_rays, 0 | 1 | 5 | 9)
            && (0..=64).contains(&self.field_half)
            && (8.0..=256.0).contains(&self.room_reach)
            && self.budget > 0.0
            && self.budget <= 8.0
    }
}

impl Default for Quality {
    fn default() -> Self {
        Quality::DEFAULT
    }
}

/// What the engine is spending, for the debug command and the settings.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    /// Rays in the latest room measurement, after the budget.
    pub room_rays: u32,
    /// The room measurement's share of one core, smoothed.
    pub room_cost: f32,
    /// How long the latest pathing field took to build.
    pub field_build: Duration,
    /// How far that field reaches each way, in blocks; 0 when there is none.
    pub field_half: i32,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn the_bounds_are_checked_one_step_past_each_edge() {
        assert!(Quality::DEFAULT.is_valid());
        for broken in [
            Quality {
                room_rays: (1 << 16) + 1,
                ..Quality::DEFAULT
            },
            Quality {
                room_bounces: 65,
                ..Quality::DEFAULT
            },
            Quality {
                room_every: Duration::from_millis(19),
                ..Quality::DEFAULT
            },
            Quality {
                room_every: Duration::from_millis(5001),
                ..Quality::DEFAULT
            },
            Quality {
                occlusion_rays: 2,
                ..Quality::DEFAULT
            },
            Quality {
                field_half: -1,
                ..Quality::DEFAULT
            },
            Quality {
                field_half: 65,
                ..Quality::DEFAULT
            },
            Quality {
                room_reach: 7.9,
                ..Quality::DEFAULT
            },
            Quality {
                room_reach: 256.1,
                ..Quality::DEFAULT
            },
            Quality {
                budget: 0.0,
                ..Quality::DEFAULT
            },
            Quality {
                budget: 8.1,
                ..Quality::DEFAULT
            },
        ] {
            assert!(!broken.is_valid(), "{broken:?}");
        }
        // The edges themselves are all fine.
        for edge in [
            Quality {
                room_rays: 1 << 16,
                ..Quality::DEFAULT
            },
            Quality {
                room_bounces: 64,
                ..Quality::DEFAULT
            },
            Quality {
                room_every: Duration::from_millis(20),
                ..Quality::DEFAULT
            },
            Quality {
                room_every: Duration::from_secs(5),
                ..Quality::DEFAULT
            },
            Quality {
                occlusion_rays: 9,
                ..Quality::DEFAULT
            },
            Quality {
                field_half: 64,
                ..Quality::DEFAULT
            },
            Quality {
                room_reach: 256.0,
                ..Quality::DEFAULT
            },
            Quality {
                budget: 8.0,
                ..Quality::DEFAULT
            },
        ] {
            assert!(edge.is_valid(), "{edge:?}");
        }
    }
}
