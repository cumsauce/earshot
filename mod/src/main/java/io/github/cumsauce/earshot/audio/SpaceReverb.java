package io.github.cumsauce.earshot.audio;

import io.github.cumsauce.earshot.natives.NativeBridge;
import java.lang.foreign.MemorySegment;
import java.util.ArrayList;
import java.util.List;
import net.minecraft.world.phys.Vec3;
import org.jspecify.annotations.Nullable;
import org.lwjgl.openal.EXTEfx;

/**
 * Reverb for sounds in a different space from the listener (M5): a cave heard from outside rings with the cave's
 * reverb, coming out of the opening, rather than with the open air around the listener. A sound counts as elsewhere
 * when it comes round a corner or through a wall. Up to {@value #SPACES} spaces at once, each an EAX reverb measured
 * from where its sounds are and panned towards where they are heard from. Sounds send to it through the send after the
 * listener's two.
 *
 * <p>Everything here runs on the sound thread, inside {@link SoundShaper}'s update.
 */
final class SpaceReverb {
	static final int SPACES = 2;
	/** The aux send sources use for it; 0 and 1 are the listener's reverb. */
	static final int SEND = 2;
	/** Sounds within this many blocks of a space's centre share its reverb. */
	private static final double SAME_SPACE = 8;
	/** A space is measured again this often while it has sounds, and let go once it has had none this long. */
	private static final long REMEASURE_NANOS = 1_000_000_000L;
	private static final long IDLE_NANOS = 3_000_000_000L;
	/** The trace for a space: about a millisecond. */
	private static final int RAYS = 512;
	private static final int BOUNCES = 8;
	/** How far round a sound's way must go, beyond the straight line, for it to be elsewhere. */
	static final double ROUND = 2;
	/** Or how little of the mid band gets straight through. */
	static final float WALLED = 0.3f;
	/** How strongly the reverb comes from its sounds' direction, 0 (all round) to 1. */
	private static final float PAN = 0.7f;

	private final NativeBridge bridge;
	private final MemorySegment engine;
	private final @Nullable Space[] spaces = new Space[SPACES];
	private long seed;
	/**
	 * Measures spaces off the sound thread, which also feeds the game's streams: a trace takes about a millisecond.
	 * Results are applied on the sound thread at the end of the next update.
	 */
	private final java.util.concurrent.ExecutorService measurer = java.util.concurrent.Executors
			.newSingleThreadExecutor(task -> {
				Thread thread = new Thread(task, "earshot-space");
				thread.setDaemon(true);
				return thread;
			});

	/** One space and its reverb. */
	private static final class Space {
		final int effect;
		final int slot;
		Vec3 centre;
		long measuredAt;
		long usedAt;
		final double[] room = new double[NativeBridge.ROOM_DOUBLES];
		/** A measurement made off the sound thread, not yet applied; and whether one is under way. */
		volatile double @Nullable [] measured;
		volatile boolean measuring;
		/** Whether its reverb has been set from a measurement yet: until then its slot is silent. */
		boolean ready;
		/** The sums of where this update's sounds are and where they are heard from, and how many there are. */
		Vec3 positions = Vec3.ZERO;
		Vec3 heardFrom = Vec3.ZERO;
		int sounds;

		Space(int effect, int slot, Vec3 centre) {
			this.effect = effect;
			this.slot = slot;
			this.centre = centre;
		}
	}

	SpaceReverb(NativeBridge bridge, MemorySegment engine) {
		this.bridge = bridge;
		this.engine = engine;
	}

	/** The rooms measured for the spaces in use, as {@link NativeBridge#room}'s doubles. For tests and debugging. */
	List<double[]> rooms() {
		List<double[]> rooms = new ArrayList<>();
		for (Space space : spaces) {
			if (space != null && space.ready) rooms.add(space.room.clone());
		}
		return rooms;
	}

	/** The game closed its context, and every slot and effect with it. */
	void contextClosing() {
		java.util.Arrays.fill(spaces, null);
	}

	/** Starts an update: no sounds in any space yet. */
	void begin() {
		for (Space space : spaces) {
			if (space == null) continue;
			space.positions = Vec3.ZERO;
			space.heardFrom = Vec3.ZERO;
			space.sounds = 0;
		}
	}

	/**
	 * The slot for a sound at {@code position}, heard from {@code heardFrom}, in a space away from the listener: the
	 * space it shares with other sounds near it, or a new one if one is free. 0 if none is.
	 */
	int slotFor(Vec3 position, Vec3 heardFrom, long now) {
		Space best = null;
		for (Space space : spaces) {
			if (space != null && space.centre.distanceTo(position) < SAME_SPACE
					&& (best == null || space.centre.distanceTo(position) < best.centre.distanceTo(position))) {
				best = space;
			}
		}
		if (best == null) best = claim(position, now);
		if (best == null) return 0;
		best.usedAt = now;
		best.positions = best.positions.add(position);
		best.heardFrom = best.heardFrom.add(heardFrom);
		best.sounds++;
		return best.slot;
	}

	/**
	 * Ends an update, after every sound's sends are set: lets go of spaces without sounds for a while (or of all, when
	 * {@code off}), measures the rest again when due, and points each reverb towards where its sounds are heard from.
	 *
	 * @param right the listener's right, {@code up} up and {@code forward} forward, in world axes
	 */
	void end(long now, boolean off, Vec3 listener, Vec3 right, Vec3 up, Vec3 forward) {
		for (int i = 0; i < SPACES; i++) {
			Space space = spaces[i];
			if (space == null) continue;
			if (off || (space.sounds == 0 && now - space.usedAt > IDLE_NANOS)) {
				free(space);
				spaces[i] = null;
				continue;
			}
			apply(space);
			if (space.sounds == 0) continue;
			if (now - space.measuredAt > REMEASURE_NANOS) {
				space.centre = space.positions.scale(1.0 / space.sounds);
				measure(space, now);
			}
			Vec3 toward = space.heardFrom.scale(1.0 / space.sounds).subtract(listener).normalize();
			// EAX reverb pans in its own left-handed frame: x right, y up, z forward.
			float[] pan = {(float) (PAN * toward.dot(right)), (float) (PAN * toward.dot(up)),
					(float) (PAN * toward.dot(forward))};
			EXTEfx.alEffectfv(space.effect, EXTEfx.AL_EAXREVERB_REFLECTIONS_PAN, pan);
			EXTEfx.alEffectfv(space.effect, EXTEfx.AL_EAXREVERB_LATE_REVERB_PAN, pan);
			EXTEfx.alAuxiliaryEffectSloti(space.slot, EXTEfx.AL_EFFECTSLOT_EFFECT, space.effect);
		}
	}

	private @Nullable Space claim(Vec3 position, long now) {
		for (int i = 0; i < SPACES; i++) {
			if (spaces[i] != null) continue;
			int effect = EXTEfx.alGenEffects();
			EXTEfx.alEffecti(effect, EXTEfx.AL_EFFECT_TYPE, EXTEfx.AL_EFFECT_EAXREVERB);
			int slot = EXTEfx.alGenAuxiliaryEffectSlots();
			// Silent until its room is measured, rather than a generic reverb for a moment.
			EXTEfx.alAuxiliaryEffectSlotf(slot, EXTEfx.AL_EFFECTSLOT_GAIN, 0f);
			Space space = new Space(effect, slot, position);
			measure(space, now);
			spaces[i] = space;
			return space;
		}
		return null;
	}

	/** Starts measuring {@code space} again, off the sound thread, unless it's being measured already. */
	private void measure(Space space, long now) {
		space.measuredAt = now;
		if (space.measuring) return;
		space.measuring = true;
		double[] at = {space.centre.x, space.centre.y, space.centre.z};
		long forSeed = ++seed;
		measurer.execute(() -> {
			try {
				double[] room = new double[NativeBridge.ROOM_DOUBLES];
				bridge.roomAt(engine, at, RAYS, BOUNCES, forSeed, room);
				space.measured = room;
			} catch (RuntimeException e) {
				io.github.cumsauce.earshot.EarshotClient.LOGGER.warn("Measuring a space failed", e);
			} finally {
				space.measuring = false;
			}
		});
	}

	/** Sets a space's reverb from its latest measurement, if a new one has come in. On the sound thread. */
	private static void apply(Space space) {
		double[] room = space.measured;
		if (room == null) return;
		space.measured = null;
		System.arraycopy(room, 0, space.room, 0, room.length);
		for (int i = 0; i < SoundShaper.EAX_PARAMETERS.length; i++) {
			EXTEfx.alEffectf(space.effect, SoundShaper.EAX_PARAMETERS[i], (float) room[NativeBridge.EAX_FIRST + i]);
		}
		EXTEfx.alAuxiliaryEffectSloti(space.slot, EXTEfx.AL_EFFECTSLOT_EFFECT, space.effect);
		if (!space.ready) {
			EXTEfx.alAuxiliaryEffectSlotf(space.slot, EXTEfx.AL_EFFECTSLOT_GAIN, 1f);
			space.ready = true;
		}
	}

	private static void free(Space space) {
		EXTEfx.alAuxiliaryEffectSloti(space.slot, EXTEfx.AL_EFFECTSLOT_EFFECT, EXTEfx.AL_EFFECT_NULL);
		EXTEfx.alDeleteAuxiliaryEffectSlots(space.slot);
		EXTEfx.alDeleteEffects(space.effect);
	}
}
