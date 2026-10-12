//! Acoustics over the mirrored world: how each sound reaches the listener
//! (straight through what is in the way, or round it along the shortest path
//! through the air), and the path tracer that reverb will build on
//! (docs/PLAN.md).

mod engine;
mod hear;
mod pathing;
mod paths;
mod rng;

pub use crate::engine::{Engine, FIELD_HALF};
pub use crate::hear::{Heard, hear, occlusion};
pub use crate::pathing::{Field, Route};
pub use crate::paths::{Segment, TraceStats, sphere_directions, trace_paths};
pub use crate::rng::Rng;
