package io.github.cumsauce.earshot.world;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * The packed native byte: the material id in the low nibble, how full the block is in the high one (0 = full,
 * otherwise sixteenths, so a slab carries 8). No blocks are needed — this reads the packing itself, as the
 * native world does when it takes a byte back apart. Loading {@link MaterialMap} pulls in {@code SoundType},
 * whose constants live in a registry, so the registries are bootstrapped once here.
 */
class MaterialMapTest {
	/** Full (0), a quarter, a half, three quarters and the most the nibble can hold. */
	private static final int[] FILLS = {0, 4, 8, 12, 15};

	@BeforeAll
	static void bootstrapRegistries() {
		net.minecraft.SharedConstants.tryDetectVersion();
		net.minecraft.server.Bootstrap.bootStrap();
	}

	@Test
	void everyMaterialAndFillSurvivesAPackAndUnpack() {
		for (AcousticMaterial material : AcousticMaterial.values()) {
			for (int fill : FILLS) {
				byte packed = MaterialMap.packNibble(fill, material);
				String at = material + " packed at fill " + fill;
				assertEquals(material, MaterialMap.materialOf(packed), at);
				assertEquals(material.id() & 0x0F, packed & 0x0F, at);
				// The fill half, unpacked as the native world reads it.
				assertEquals(fill, packed >>> 4 & 0x0F, at);
			}
		}
	}

	/** A fill outside the 4 bits it is given would roll into the id, so it is refused loudly instead. */
	@Test
	void aFillPastTheFourBitsItHasIsRefused() {
		IllegalStateException sixteen = assertThrows(IllegalStateException.class,
				() -> MaterialMap.packNibble(16, AcousticMaterial.ROCK));
		assertTrue(sixteen.getMessage().contains("16"), sixteen.getMessage());
		assertThrows(IllegalStateException.class, () -> MaterialMap.packNibble(-1, AcousticMaterial.ROCK));
		// A full block is 16 sixteenths, but 0 is what fits in the nibble and what the natives read as full.
		assertEquals((byte) (AcousticMaterial.ROCK.id() & 0x0F), MaterialMap.packNibble(0, AcousticMaterial.ROCK));
	}
}
