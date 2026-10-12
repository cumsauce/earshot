use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::Material;

/// Blocks in a section, 16 × 16 × 16, in Minecraft's own order: index
/// `(y << 8) | (z << 4) | x` with each coordinate 0 to 15.
pub const SECTION_BLOCKS: usize = 16 * 16 * 16;

/// A section's position: block coordinates divided by 16, rounded down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SectionPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectionPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        SectionPos { x, y, z }
    }

    /// The section that holds block `(x, y, z)`.
    pub const fn of_block(x: i32, y: i32, z: i32) -> Self {
        SectionPos::new(x >> 4, y >> 4, z >> 4)
    }
}

/// One mirrored section: a material id per block.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    blocks: Box<[u8; SECTION_BLOCKS]>,
}

impl Section {
    /// The material at `(x, y, z)` within the section, each 0 to 15.
    #[inline]
    pub fn get(&self, x: i32, y: i32, z: i32) -> Material {
        Material::from_id(self.blocks[index(x, y, z)])
    }

    fn is_empty(&self) -> bool {
        self.blocks.iter().all(|&id| id == Material::Air.id())
    }
}

#[inline]
fn index(x: i32, y: i32, z: i32) -> usize {
    (((y & 15) << 8) | ((z & 15) << 4) | (x & 15)) as usize
}

/// The mirrored blocks. Sections that are all air, or not mirrored yet, are
/// simply absent, and read as air: unknown blocks never occlude.
#[derive(Debug, Default, Clone)]
pub struct World {
    sections: HashMap<SectionPos, Section, BuildHasherDefault<PosHasher>>,
}

impl World {
    pub fn new() -> Self {
        World::default()
    }

    /// Replaces a whole section with `blocks`, material ids in section
    /// order.
    pub fn set_section(&mut self, pos: SectionPos, blocks: &[u8; SECTION_BLOCKS]) {
        let section = Section {
            blocks: Box::new(*blocks),
        };
        if section.is_empty() {
            self.sections.remove(&pos);
        } else {
            self.sections.insert(pos, section);
        }
    }

    pub fn remove_section(&mut self, pos: SectionPos) {
        self.sections.remove(&pos);
    }

    /// Changes one block, as when the game sets it.
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, material: Material) {
        let pos = SectionPos::of_block(x, y, z);
        match self.sections.get_mut(&pos) {
            Some(section) => {
                section.blocks[index(x, y, z)] = material.id();
                if material == Material::Air && section.is_empty() {
                    self.sections.remove(&pos);
                }
            }
            None if material != Material::Air => {
                let mut blocks = Box::new([Material::Air.id(); SECTION_BLOCKS]);
                blocks[index(x, y, z)] = material.id();
                self.sections.insert(pos, Section { blocks });
            }
            None => {}
        }
    }

    /// The material of block `(x, y, z)`.
    pub fn block(&self, x: i32, y: i32, z: i32) -> Material {
        self.section(SectionPos::of_block(x, y, z))
            .map_or(Material::Air, |section| section.get(x, y, z))
    }

    pub fn section(&self, pos: SectionPos) -> Option<&Section> {
        self.sections.get(&pos)
    }

    /// Forgets every section, as when the player changes dimension.
    pub fn clear(&mut self) {
        self.sections.clear();
    }

    /// How many sections hold anything but air.
    pub fn sections(&self) -> usize {
        self.sections.len()
    }
}

/// FxHash's multiply-rotate over section coordinates: the world is looked up
/// once per ray step, where SipHash would cost more than the step.
#[derive(Default)]
struct PosHasher(u64);

impl Hasher for PosHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.add(u64::from(byte));
        }
    }

    fn write_i32(&mut self, i: i32) {
        self.add(u64::from(i as u32));
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

impl PosHasher {
    fn add(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_land_in_the_right_section_and_place() {
        let mut world = World::new();
        world.set_block(-1, 64, 17, Material::Rock);
        assert_eq!(world.block(-1, 64, 17), Material::Rock);
        assert_eq!(world.block(0, 64, 17), Material::Air);
        assert_eq!(world.block(-1, 65, 17), Material::Air);
        assert!(world.section(SectionPos::new(-1, 4, 1)).is_some());
        assert_eq!(world.sections(), 1);
    }

    #[test]
    fn sections_of_air_are_not_kept() {
        let mut world = World::new();
        let mut blocks = [0u8; SECTION_BLOCKS];
        blocks[index(3, 4, 5)] = Material::Wood.id();
        world.set_section(SectionPos::new(0, 0, 0), &blocks);
        assert_eq!(world.block(3, 4, 5), Material::Wood);

        world.set_block(3, 4, 5, Material::Air);
        assert_eq!(world.sections(), 0);
        world.set_section(SectionPos::new(0, 0, 0), &[0; SECTION_BLOCKS]);
        assert_eq!(world.sections(), 0);
    }

    #[test]
    fn the_section_order_is_minecrafts() {
        // y, then z, then x.
        assert_eq!(index(1, 0, 0), 1);
        assert_eq!(index(0, 0, 1), 16);
        assert_eq!(index(0, 1, 0), 256);
        assert_eq!(index(15, 15, 15), SECTION_BLOCKS - 1);
        assert_eq!(index(-1, -1, -1), SECTION_BLOCKS - 1);
    }
}
