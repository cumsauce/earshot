package io.github.cumsauce.earshot.world;

import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.natives.EarshotException;
import io.github.cumsauce.earshot.natives.NativeBridge;
import java.lang.foreign.MemorySegment;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashSet;
import java.util.Iterator;
import java.util.List;
import java.util.Set;
import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientChunkEvents;
import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientTickEvents;
import net.fabricmc.fabric.api.event.lifecycle.v1.CommonLifecycleEvents;
import net.minecraft.client.Minecraft;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import org.jspecify.annotations.Nullable;

/**
 * Keeps the native world a copy of the blocks around the player, as materials: each loaded chunk within
 * {@value #RADIUS} chunks is copied section by section, nearest first and within a time budget per tick, and every
 * block change after that is passed on as it happens. Runs on the client thread.
 */
public final class WorldMirror {
	/** How far from the player chunks are mirrored, in chunks: farther than any sound path. */
	public static final int RADIUS = 8;
	/** Mirrored chunks this much farther away are dropped again. */
	private static final int EVICT_MARGIN = 2;
	/** Copying time per tick, so loading a world never hitches. */
	private static final long BUDGET_NANOS = 2_000_000;
	private static final int EVICT_EVERY_TICKS = 20;

	private final NativeBridge bridge;
	private final MemorySegment engine;
	private final MaterialMap materials = new MaterialMap();
	/** Loaded chunks not copied yet, by {@link ChunkPos#pack()}. */
	private final Set<Long> pending = new HashSet<>();
	/** Chunks copied, by {@link ChunkPos#pack()}. */
	private final Set<Long> mirrored = new HashSet<>();
	private final byte[] blocks = new byte[NativeBridge.SECTION_BLOCKS];
	private @Nullable ClientLevel level;
	private int ticksUntilEvict;
	private boolean failed;

	public WorldMirror(NativeBridge bridge, MemorySegment engine) {
		this.bridge = bridge;
		this.engine = engine;
	}

	public void register() {
		ClientChunkEvents.CHUNK_LOAD.register(this::chunkLoaded);
		ClientChunkEvents.CHUNK_UNLOAD.register(this::chunkUnloaded);
		ClientTickEvents.END_CLIENT_TICK.register(this::tick);
		CommonLifecycleEvents.TAGS_LOADED.register((registries, client) -> {
			if (client) tagsLoaded();
		});
	}

	/** Materials depend on tags, which come with a world's data: decide them again and copy every chunk again. */
	private void tagsLoaded() {
		materials.clear();
		recopy();
	}

	/** Uses {@code overrides} from now on, copying every chunk again with them. */
	public void setOverrides(MaterialOverrides overrides) {
		materials.setOverrides(overrides);
		recopy();
	}

	private void recopy() {
		pending.addAll(mirrored);
		mirrored.clear();
	}

	public MaterialMap materials() {
		return materials;
	}

	/** Chunks waiting to be copied. */
	public int pending() {
		return pending.size();
	}

	/** Chunks copied. */
	public int mirrored() {
		return mirrored.size();
	}

	/** Whether the block at {@code pos} has been copied, so the native world knows it. */
	public boolean isMirrored(BlockPos pos) {
		return mirrored.contains(ChunkPos.pack(pos));
	}

	/** A block changed in {@code level}: called for every change, from {@code ClientLevel.sendBlockUpdated}. */
	public void blockChanged(ClientLevel level, BlockPos pos, BlockState state) {
		if (level != this.level || !mirrored.contains(ChunkPos.pack(pos))) return;
		call(() -> bridge.setBlock(engine, pos.getX(), pos.getY(), pos.getZ(), materials.packed(state)));
	}

	private void chunkLoaded(ClientLevel level, LevelChunk chunk) {
		switchTo(level);
		pending.add(chunk.getPos().pack());
	}

	private void chunkUnloaded(ClientLevel level, LevelChunk chunk) {
		if (level != this.level) return;
		long key = chunk.getPos().pack();
		pending.remove(key);
		if (mirrored.remove(key)) forget(level, chunk.getPos());
	}

	private void tick(Minecraft minecraft) {
		ClientLevel level = minecraft.level;
		if (level == null || minecraft.player == null) {
			switchTo(null);
			return;
		}
		switchTo(level);
		ChunkPos center = minecraft.player.chunkPosition();
		if (--ticksUntilEvict <= 0) {
			ticksUntilEvict = EVICT_EVERY_TICKS;
			evictFarFrom(level, center);
		}
		copyNearest(level, center);
	}

	/** Copies pending chunks within {@link #RADIUS} of {@code center}, nearest first, until the budget runs out. */
	private void copyNearest(ClientLevel level, ChunkPos center) {
		List<Long> near = new ArrayList<>();
		for (long key : pending) {
			if (distance(ChunkPos.unpack(key), center) <= RADIUS) near.add(key);
		}
		near.sort(Comparator.comparingInt(key -> distance(ChunkPos.unpack(key), center)));
		long deadline = System.nanoTime() + BUDGET_NANOS;
		for (long key : near) {
			if (System.nanoTime() > deadline) break;
			ChunkPos pos = ChunkPos.unpack(key);
			pending.remove(key);
			LevelChunk chunk = level.getChunkSource().getChunk(pos.x(), pos.z(), ChunkStatus.FULL, false);
			if (chunk == null) continue;
			if (call(() -> copy(level, chunk))) mirrored.add(key);
		}
	}

	/** Drops mirrored chunks the player has moved away from; they are copied again if the player comes back. */
	private void evictFarFrom(ClientLevel level, ChunkPos center) {
		for (Iterator<Long> keys = mirrored.iterator(); keys.hasNext();) {
			long key = keys.next();
			ChunkPos pos = ChunkPos.unpack(key);
			if (distance(pos, center) > RADIUS + EVICT_MARGIN) {
				keys.remove();
				pending.add(key);
				forget(level, pos);
			}
		}
	}

	private void copy(ClientLevel level, LevelChunk chunk) {
		ChunkPos pos = chunk.getPos();
		LevelChunkSection[] sections = chunk.getSections();
		for (int i = 0; i < sections.length; i++) {
			int sectionY = level.getSectionYFromSectionIndex(i);
			LevelChunkSection section = sections[i];
			if (section.hasOnlyAir()) {
				bridge.removeSection(engine, pos.x(), sectionY, pos.z());
				continue;
			}
			BlockState last = null;
			byte lastId = 0;
			for (int y = 0; y < 16; y++) {
				for (int z = 0; z < 16; z++) {
					for (int x = 0; x < 16; x++) {
						BlockState state = section.getBlockState(x, y, z);
						// Runs of one state are common: skip the lookup for them.
						if (state != last) {
							last = state;
							lastId = materials.packed(state);
						}
						blocks[(y << 8) | (z << 4) | x] = lastId;
					}
				}
			}
			bridge.setSection(engine, pos.x(), sectionY, pos.z(), blocks);
		}
	}

	private void forget(ClientLevel level, ChunkPos pos) {
		call(() -> {
			for (int sectionY = level.getMinSectionY(); sectionY <= level.getMaxSectionY(); sectionY++) {
				bridge.removeSection(engine, pos.x(), sectionY, pos.z());
			}
		});
	}

	/** Starts over when the player changes dimension or leaves. */
	private void switchTo(@Nullable ClientLevel level) {
		if (level == this.level) return;
		this.level = level;
		pending.clear();
		mirrored.clear();
		// Starting over gives a fresh chance: what failed in the old world says nothing about the new one.
		failed = false;
		call(() -> bridge.clearWorld(engine));
	}

	/** Runs a native call; a failure is logged once and turns the mirror off rather than the game. */
	private boolean call(Runnable call) {
		if (failed) return false;
		try {
			call.run();
			return true;
		} catch (EarshotException e) {
			failed = true;
			EarshotClient.LOGGER.error("World mirror stopped: a native call failed", e);
			return false;
		}
	}

	private static int distance(ChunkPos a, ChunkPos b) {
		return Math.max(Math.abs(a.x() - b.x()), Math.abs(a.z() - b.z()));
	}
}
