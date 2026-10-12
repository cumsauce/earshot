package io.github.cumsauce.earshot.world;

import java.util.Arrays;
import java.util.IdentityHashMap;
import java.util.Map;
import net.minecraft.core.BlockPos;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.DoorBlock;
import net.minecraft.world.level.block.FenceBlock;
import net.minecraft.world.level.block.FenceGateBlock;
import net.minecraft.world.level.block.IceBlock;
import net.minecraft.world.level.block.IronBarsBlock;
import net.minecraft.world.level.block.LeavesBlock;
import net.minecraft.world.level.block.SoundType;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.TrapDoorBlock;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.VoxelShape;

/**
 * Which {@link AcousticMaterial} each block state is. Decided once per state, from the block's class, tags, the sound
 * type and the collision shape, so modded blocks get a sensible material without a list, then kept in an array by
 * state id. Tags come with a world's data, so {@link #clear} starts over whenever they reload.
 */
public final class MaterialMap {
	/** Smaller shapes (buttons, rails, carpets, torches' bases, fence posts) let sound by. */
	private static final double SOLID_VOLUME = 0.2;
	private static final byte UNKNOWN = -1;

	private static final Map<SoundType, AcousticMaterial> BY_SOUND = new IdentityHashMap<>();

	static {
		for (SoundType sound : new SoundType[] {SoundType.WOOL, SoundType.SLIME_BLOCK, SoundType.HONEY_BLOCK,
				SoundType.SPONGE, SoundType.WET_SPONGE, SoundType.WART_BLOCK, SoundType.SHROOMLIGHT, SoundType.MOSS,
				SoundType.MOSS_CARPET}) {
			BY_SOUND.put(sound, AcousticMaterial.CLOTH);
		}
		for (SoundType sound : new SoundType[] {SoundType.GLASS, SoundType.AMETHYST, SoundType.AMETHYST_CLUSTER}) {
			BY_SOUND.put(sound, AcousticMaterial.GLASS);
		}
		for (SoundType sound : new SoundType[] {SoundType.WOOD, SoundType.BAMBOO_WOOD, SoundType.NETHER_WOOD,
				SoundType.CHERRY_WOOD, SoundType.LADDER, SoundType.SCAFFOLDING, SoundType.CHISELED_BOOKSHELF,
				SoundType.SHELF, SoundType.BAMBOO, SoundType.HANGING_SIGN, SoundType.NETHER_WOOD_HANGING_SIGN,
				SoundType.BAMBOO_WOOD_HANGING_SIGN, SoundType.CHERRY_WOOD_HANGING_SIGN, SoundType.STEM}) {
			BY_SOUND.put(sound, AcousticMaterial.WOOD);
		}
		for (SoundType sound : new SoundType[] {SoundType.METAL, SoundType.ANVIL, SoundType.LANTERN, SoundType.CHAIN,
				SoundType.NETHERITE_BLOCK, SoundType.COPPER, SoundType.COPPER_BULB, SoundType.COPPER_GRATE,
				SoundType.IRON, SoundType.HEAVY_CORE, SoundType.VAULT, SoundType.TRIAL_SPAWNER, SoundType.SPAWNER,
				SoundType.LODESTONE}) {
			BY_SOUND.put(sound, AcousticMaterial.METAL);
		}
		for (SoundType sound : new SoundType[] {SoundType.SAND, SoundType.GRAVEL, SoundType.SUSPICIOUS_SAND,
				SoundType.SUSPICIOUS_GRAVEL, SoundType.SOUL_SAND}) {
			BY_SOUND.put(sound, AcousticMaterial.SAND);
		}
		for (SoundType sound : new SoundType[] {SoundType.GRASS, SoundType.WET_GRASS, SoundType.ROOTED_DIRT,
				SoundType.MUD, SoundType.NYLIUM, SoundType.SOUL_SOIL, SoundType.MANGROVE_ROOTS,
				SoundType.MUDDY_MANGROVE_ROOTS, SoundType.SCULK}) {
			BY_SOUND.put(sound, AcousticMaterial.SOIL);
		}
		for (SoundType sound : new SoundType[] {SoundType.SNOW, SoundType.POWDER_SNOW}) {
			BY_SOUND.put(sound, AcousticMaterial.SNOW);
		}
	}

	private byte[] byState = new byte[0];

	/** Forgets every decision, for when tags reload. */
	public void clear() {
		byState = new byte[0];
	}

	/** The material of {@code state}. */
	public AcousticMaterial material(BlockState state) {
		return AcousticMaterial.values()[id(state)];
	}

	/** The native id of {@code state}'s material. */
	public byte id(BlockState state) {
		int stateId = Block.getId(state);
		if (stateId < 0) return classify(state).id();
		if (stateId >= byState.length) {
			int size = Math.max(stateId + 1, Block.BLOCK_STATE_REGISTRY.size());
			int old = byState.length;
			byState = Arrays.copyOf(byState, size);
			Arrays.fill(byState, old, size, UNKNOWN);
		}
		byte id = byState[stateId];
		if (id == UNKNOWN) {
			id = classify(state).id();
			byState[stateId] = id;
		}
		return id;
	}

	/** Decides a state's material. */
	static AcousticMaterial classify(BlockState state) {
		if (state.isAir()) return AcousticMaterial.AIR;
		Block block = state.getBlock();
		FluidState fluid = state.getFluidState();
		VoxelShape shape = state.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO);
		if (shape.isEmpty()) {
			if (fluid.getType().isSame(Fluids.WATER)) return AcousticMaterial.WATER;
			if (fluid.getType().isSame(Fluids.LAVA)) return AcousticMaterial.LAVA;
			return AcousticMaterial.AIR;
		}
		// An open door, trapdoor or gate leaves its opening clear.
		if (state.hasProperty(BlockStateProperties.OPEN) && state.getValue(BlockStateProperties.OPEN)) {
			return AcousticMaterial.AIR;
		}
		// Fences are mostly gaps; panes and bars are thin, but a glass pane fills its window.
		if (block instanceof FenceBlock || state.is(BlockTags.FENCES)) return AcousticMaterial.AIR;
		if (block instanceof IronBarsBlock) {
			return state.getSoundType() == SoundType.GLASS ? AcousticMaterial.GLASS : AcousticMaterial.AIR;
		}
		// A closed door is thin, but it fills its doorway.
		boolean panel = block instanceof DoorBlock || block instanceof TrapDoorBlock || block instanceof FenceGateBlock;
		if (!panel && volume(shape) < SOLID_VOLUME) return AcousticMaterial.AIR;

		if (block instanceof LeavesBlock || state.is(BlockTags.LEAVES)) return AcousticMaterial.LEAVES;
		if (block instanceof IceBlock || state.is(BlockTags.ICE)) return AcousticMaterial.ICE;
		if (state.is(BlockTags.BEDS) || state.is(BlockTags.WOOL)) return AcousticMaterial.CLOTH;
		if (state.is(BlockTags.DIRT)) return AcousticMaterial.SOIL;
		if (state.is(BlockTags.SAND)) return AcousticMaterial.SAND;
		if (state.is(BlockTags.SNOW)) return AcousticMaterial.SNOW;
		return BY_SOUND.getOrDefault(state.getSoundType(), AcousticMaterial.ROCK);
	}

	/** The volume of a collision shape, in blocks. */
	private static double volume(VoxelShape shape) {
		double volume = 0;
		for (AABB box : shape.toAabbs()) {
			volume += box.getXsize() * box.getYsize() * box.getZsize();
		}
		return volume;
	}
}
