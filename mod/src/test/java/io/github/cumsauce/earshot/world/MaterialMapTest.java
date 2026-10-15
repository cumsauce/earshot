package io.github.cumsauce.earshot.world;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.material.Fluids;
import org.junit.jupiter.api.Test;

/**
 * The packed native byte: the material id in the low nibble, how much of the block is there in the high one (0 =
 * full; otherwise sixteenths, so a slab carries 8).
 */
class MaterialMapTest {
	private static byte fill(BlockState state) {
		return (byte) (MaterialMap.pack(state) >> 4);
	}

	@Test
	void fullCubesPackFull() {
		for (BlockState state : new BlockState[] {Blocks.STONE.defaultBlockState(),
				Blocks.OAK_PLANKS.defaultBlockState(), Blocks.OAK_LEAVES.defaultBlockState(),
				Blocks.GLOWSTONE.defaultBlockState()}) {
			assertEquals(0, fill(state), state + " should be full");
		}
	}

	@Test
	void aSlabIsHalfAndStairsAreThreeQuarters() {
		assertEquals(8, fill(Blocks.OAK_SLAB.defaultBlockState()), "slab");
		assertTrue(fill(Blocks.OAK_STAIRS.defaultBlockState()) > 8, "stairs more than a slab");
		assertTrue(fill(Blocks.OAK_STAIRS.defaultBlockState()) < 15, "stairs less than full");
	}

	@Test
	void thinBlocksAreNeverLessThanAQuarter() {
		// A lone cobblestone wall post is a quarter full or more.
		assertTrue(fill(Blocks.COBBLESTONE_WALL.defaultBlockState()) >= 4, "wall post");
	}

	@Test
	void thinnessLiesForThingsThatFillTheirOpening() {
		// A closed door is thin, but it fills its doorway.
		assertEquals(0, fill(Blocks.OAK_DOOR.defaultBlockState()), "closed door");
		assertEquals(0, fill(Blocks.OAK_TRAPDOOR.defaultBlockState()), "closed trapdoor");
		// An open one is air either way.
		byte open = MaterialMap.pack(Blocks.OAK_DOOR.defaultBlockState()
				.setValue(BlockStateProperties.OPEN, true));
		assertEquals(0, open, "open door");
		// And a glass pane fills its window.
		assertEquals(0, fill(Blocks.GLASS_PANE.defaultBlockState()), "pane");
	}

	@Test
	void thinSoftLayersAreRugs() {
		byte carpet = MaterialMap.pack(Blocks.WHITE_CARPET.defaultBlockState());
		assertEquals(AcousticMaterial.RUG.id(), carpet & 0x0F, "carpet");
		assertTrue((carpet >> 4) > 0 && (carpet >> 4) < 15, "thin, but there: " + carpet);
		byte layer = MaterialMap.pack(Blocks.SNOW.defaultBlockState());
		assertEquals(AcousticMaterial.RUG.id(), layer & 0x0F, "snow layer");
		assertEquals(AcousticMaterial.SNOW.id(),
				MaterialMap.pack(Blocks.SNOW_BLOCK.defaultBlockState()) & 0x0F, "full snow");
	}

	@Test
	void waterPacksFullAndAirPacksZero() {
		byte water = MaterialMap.pack(Blocks.WATER.defaultBlockState());
		assertEquals(AcousticMaterial.WATER.id(), water & 0x0F, "water material");
		assertEquals(0, fill(Blocks.WATER.defaultBlockState()), "water fills its block");
		assertEquals(0, MaterialMap.pack(Blocks.AIR.defaultBlockState()), "air");
		assertEquals(0, MaterialMap.pack(Blocks.OAK_FENCE.defaultBlockState()), "fence");
	}
}
