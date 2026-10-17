//! Acoustics over the mirrored world: how each sound reaches the listener
//! (straight through what is in the way, or round it along the shortest path
//! through the air), and the path tracer that reverb will build on
//! (docs/PLAN.md).

mod early;
mod eax;
mod engine;
mod ground;
mod hear;
mod ir;
mod pathing;
mod paths;
mod quality;
mod reverb;
mod rng;

pub use crate::early::{ECHO_REACH, Early, early_reflections, early_taps, echoes, edge_sources};
pub use crate::eax::{EaxReverb, eax_reverb};
pub use crate::engine::Engine;
pub use crate::hear::{Heard, hear, occlusion};
pub use crate::ir::{EdgeSource, IR_CHANNELS, Reflection, impulse_response, late_energy};
pub use crate::pathing::{Field, MAX_APERTURE, Route};
pub use crate::paths::{Segment, TraceStats, sphere_directions, trace_directions, trace_paths};
pub use crate::quality::{DiffractionStandard, Quality, Stats};
pub use crate::reverb::{Room, SPEED_OF_SOUND, measure, measure_threads, measure_with_early};
pub use crate::rng::Rng;
