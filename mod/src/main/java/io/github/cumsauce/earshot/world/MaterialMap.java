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
import net.minecraft.world.level.block.SnowLayerBlock;
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
 * state id. Tags come with a world's data, so {@link #clear} starts over whenever they reload. Materials set by hand
 * ({@link MaterialOverrides}) win over the guesses, except that what lets sound through (air, open doors, fences,
 * small shapes) stays open unless an override says so.
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
	private MaterialOverrides overrides = MaterialOverrides.NONE;

	/** Forgets every decision, for when tags reload. */
	public void clear() {
		byState = new byte[0];
	}

	/** Uses {@code overrides} from now on, and forgets every decision. */
	public void setOverrides(MaterialOverrides overrides) {
		this.overrides = overrides;
		clear();
	}

	public MaterialOverrides overrides() {
		return overrides;
	}

	/** The material of {@code state}. */
	public AcousticMaterial material(BlockState state) {
		return AcousticMaterial.values()[packed(state) & 0x0F];
	}

	/**
	 * The native byte for {@code state}: the material id in the low nibble, how much of the block is there in the
	 * high one (0 = full; otherwise sixteenths, so a slab carries 8).
	 */
	public byte packed(BlockState state) {
		int stateId = Block.getId(state);
		if (stateId < 0) return pack(state, overrides);
		if (stateId >= byState.length) {
			int size = Math.max(stateId + 1, Block.BLOCK_STATE_REGISTRY.size());
			int old = byState.length;
			byState = Arrays.copyOf(byState, size);
			Arrays.fill(byState, old, size, UNKNOWN);
		}
		byte id = byState[stateId];
		if (id == UNKNOWN) {
			id = pack(state, overrides);
			byState[stateId] = id;
		}
		return id;
	}

	/** Decides a state's material, with no overrides. */
	static AcousticMaterial classify(BlockState state) {
		return classify(state, MaterialOverrides.NONE);
	}

	/** Decides a state's material. */
	static AcousticMaterial classify(BlockState state, MaterialOverrides overrides) {
		if (state.isAir()) return AcousticMaterial.AIR;
		AcousticMaterial set = overrides.find(state);
		if (set == AcousticMaterial.AIR) return set;
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
		// A thin soft layer doesn't stop sound, but it soaks the room's highs up.
		if (state.is(BlockTags.WOOL_CARPETS) || state.getSoundType() == SoundType.MOSS_CARPET
				|| block instanceof SnowLayerBlock) {
			return AcousticMaterial.RUG;
		}
		// A closed door is thin, but it fills its doorway.
		boolean panel = block instanceof DoorBlock || block instanceof TrapDoorBlock || block instanceof FenceGateBlock;
		if (!panel && volume(shape) < SOLID_VOLUME) return AcousticMaterial.AIR;
		if (set != null) return set;

		if (block instanceof LeavesBlock || state.is(BlockTags.LEAVES)) return AcousticMaterial.LEAVES;
		if (block instanceof IceBlock || state.is(BlockTags.ICE)) return AcousticMaterial.ICE;
		if (state.is(BlockTags.BEDS) || state.is(BlockTags.WOOL)) return AcousticMaterial.CLOTH;
		if (state.is(BlockTags.DIRT)) return AcousticMaterial.SOIL;
		if (state.is(BlockTags.SAND)) return AcousticMaterial.SAND;
		if (state.is(BlockTags.SNOW)) return AcousticMaterial.SNOW;
		return BY_SOUND.getOrDefault(state.getSoundType(), AcousticMaterial.ROCK);
	}

	/** Decides a state's native byte, with no overrides. */
	static byte pack(BlockState state) {
		return pack(state, MaterialOverrides.NONE);
	}

	/** Decides a state's native byte: the material's id, with the fill fraction beside it. */
	static byte pack(BlockState state, MaterialOverrides overrides) {
		AcousticMaterial material = classify(state, overrides);
		if (material == AcousticMaterial.AIR) return 0;
		return (byte) (fillCode(state, material) << 4 | material.id());
	}

	/**
	 * How much of the block is there, as a nibble code: 0 for full, otherwise sixteenths, never under a quarter. The
	 * mass law scales a wall's transmission loss with its thickness, so a half slab passes most of what a full block
	 * stops. Things whose thinness is a lie — a closed door fills its doorway, a glass pane fills its window — and
	 * fluids stay full: the water's surface toll is charged per crossing, not per block.
	 */
	private static byte fillCode(BlockState state, AcousticMaterial material) {
		if (material == AcousticMaterial.WATER || material == AcousticMaterial.LAVA) return 0;
		Block block = state.getBlock();
		if (block instanceof DoorBlock || block instanceof TrapDoorBlock || block instanceof FenceGateBlock
				|| block instanceof IronBarsBlock) {
			return 0;
		}
		VoxelShape shape = state.getShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO);
		double v = volume(shape);
		if (v >= 0.97) return 0;
		return (byte) Math.clamp(Math.round(v * 16), 4, 15);
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
