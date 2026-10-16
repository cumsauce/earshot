//! The blocks around the listener as acoustics sees them.
//!
//! Java mirrors the game's chunk sections into a [`World`], one byte of
//! [`Material`] per block, and the simulation walks rays through it with
//! [`walk`] (a 3D DDA), so no triangle meshes or BVH are needed. One block is
//! one metre.

mod material;
mod trace;
mod vec3;
mod world;

pub use crate::material::{Acoustics, BANDS, Bands, Material};
pub use crate::trace::{Hit, MAX_REACH_METRES, Step, cast, transmission, walk};
pub use crate::vec3::Vec3;
pub use crate::world::{SECTION_BLOCKS, Section, SectionPos, World};
