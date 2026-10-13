use std::sync::LazyLock;

/// Frequency bands acoustics is computed in: low (up to about 500 Hz), mid,
/// and high (from about 2 kHz).
pub const BANDS: usize = 3;

/// One value per band, low to high.
pub type Bands = [f32; BANDS];

/// What a block is made of, acoustically. Java maps every block state to one
/// (by sound type, tags and shape), and the mirror stores its id, one byte
/// per block. The ids are the order here; Java's `AcousticMaterial` must list
/// the same names in the same order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Material {
    /// Also anything that doesn't stop sound: plants, open doors, unknown
    /// blocks.
    #[default]
    Air = 0,
    /// Stone, deepslate, bricks, concrete, terracotta.
    Rock,
    Wood,
    Glass,
    /// Wool, carpet, beds, hay: soft and absorbent.
    Cloth,
    Leaves,
    /// Dirt, grass, mud, clay, moss.
    Soil,
    /// Sand and gravel.
    Sand,
    Metal,
    Water,
    Lava,
    Ice,
    Snow,
}

/// How a material treats sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Acoustics {
    /// Share of the energy a reflection off it loses, per band.
    pub absorption: Bands,
    /// Share of the reflected energy scattered in every direction rather
    /// than mirrored.
    pub scattering: f32,
    /// Share of the energy that gets through one whole block, per band.
    pub transmission: Bands,
}

/// Absorption, scattering and transmission loss (dB per block) for each
/// material, in [`Material::ALL`] order. Absorption follows published
/// coefficients for similar surfaces (rock and concrete, wood panelling,
/// glass, heavy fabric, soil, snow) at about 250 Hz, 1 kHz and 4 kHz.
/// Transmission loss is a real wall's of that material (field values: a
/// masonry wall some 45 dB at 1 kHz, a timber one under 30, glazing under 30,
/// rising with frequency as the mass law has it), rather than a solid metre's,
/// which would stop nearly everything: a sound behind a wall is faint and dull,
/// and mostly comes through the doorway, which pathing finds.
const TABLE: [(&str, Bands, f32, Bands); Material::ALL.len()] = [
    ("air", [0.0, 0.0, 0.0], 0.0, [0.0, 0.0, 0.0]),
    // Scattering is at block scale: worked and natural stone (cobble, cliffs, stepped terrain) is rough and
    // breaks a reflection up, so its echoes come back diffuse rather than mirror-clean.
    // Natural and worked stone is rough and porous, not sealed concrete: about rough rock and unglazed brick's
    // coefficients, so a bare stone room rings for a few seconds, as caves do, not ten.
    ("rock", [0.05, 0.07, 0.10], 0.6, [35.0, 45.0, 55.0]),
    ("wood", [0.15, 0.10, 0.08], 0.35, [20.0, 28.0, 35.0]),
    ("glass", [0.18, 0.06, 0.04], 0.05, [22.0, 28.0, 30.0]),
    ("cloth", [0.15, 0.55, 0.70], 0.5, [4.0, 8.0, 15.0]),
    ("leaves", [0.10, 0.30, 0.50], 0.8, [1.0, 2.0, 4.0]),
    ("soil", [0.15, 0.40, 0.55], 0.65, [30.0, 40.0, 50.0]),
    ("sand", [0.20, 0.45, 0.60], 0.6, [25.0, 35.0, 45.0]),
    ("metal", [0.08, 0.05, 0.04], 0.1, [30.0, 40.0, 50.0]),
    ("water", [0.01, 0.01, 0.02], 0.05, [3.0, 5.0, 9.0]),
    ("lava", [0.05, 0.05, 0.05], 0.1, [30.0, 40.0, 50.0]),
    ("ice", [0.02, 0.02, 0.03], 0.05, [25.0, 35.0, 45.0]),
    ("snow", [0.45, 0.70, 0.85], 0.6, [6.0, 12.0, 20.0]),
];

static ACOUSTICS: LazyLock<[Acoustics; Material::ALL.len()]> = LazyLock::new(|| {
    TABLE.map(|(_, absorption, scattering, loss_db)| Acoustics {
        absorption,
        scattering,
        transmission: loss_db.map(|db| 10f32.powf(-db / 10.0)),
    })
});

impl Material {
    /// Every material, in id order.
    pub const ALL: [Material; 13] = [
        Material::Air,
        Material::Rock,
        Material::Wood,
        Material::Glass,
        Material::Cloth,
        Material::Leaves,
        Material::Soil,
        Material::Sand,
        Material::Metal,
        Material::Water,
        Material::Lava,
        Material::Ice,
        Material::Snow,
    ];

    /// The material with this id, or air for an unknown one.
    pub fn from_id(id: u8) -> Material {
        Material::ALL
            .get(usize::from(id))
            .copied()
            .unwrap_or_default()
    }

    pub fn id(self) -> u8 {
        self as u8
    }

    /// Its lowercase name, as Java's `AcousticMaterial` spells it.
    pub fn name(self) -> &'static str {
        TABLE[self as usize].0
    }

    pub fn acoustics(self) -> &'static Acoustics {
        &ACOUSTICS[self as usize]
    }

    /// Whether sound reflects off it. Everything but air does; even water
    /// reflects at its surface.
    pub fn is_solid(self) -> bool {
        self != Material::Air
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_unknown_is_air() {
        for material in Material::ALL {
            assert_eq!(Material::from_id(material.id()), material);
        }
        assert_eq!(Material::from_id(200), Material::Air);
    }

    #[test]
    fn transmission_falls_with_frequency_and_air_passes_everything() {
        assert_eq!(Material::Air.acoustics().transmission, [1.0; BANDS]);
        for material in &Material::ALL[1..] {
            let [low, mid, high] = material.acoustics().transmission;
            assert!(low >= mid && mid >= high && high > 0.0, "{material:?}");
        }
        // 18 dB through a block of rock at low frequencies.
        let rock = Material::Rock.acoustics().transmission[0];
        assert!((rock - 0.000_316).abs() < 1e-5, "{rock}");
    }

    #[test]
    fn soft_materials_absorb_more_than_hard_ones() {
        let mid = |m: Material| m.acoustics().absorption[1];
        assert!(mid(Material::Cloth) > mid(Material::Wood));
        assert!(mid(Material::Wood) > mid(Material::Rock));
    }
}
