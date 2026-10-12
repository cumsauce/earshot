//! Acoustics over the mirrored world. M1 has the path tracer that later
//! milestones build reverb and pathing on (docs/PLAN.md); the occlusion of a
//! direct path is [`earshot_world::transmission`].

mod paths;
mod rng;

pub use crate::paths::{Segment, TraceStats, sphere_directions, trace_paths};
pub use crate::rng::Rng;
