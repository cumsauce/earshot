package io.github.cumsauce.earshot.audio;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Set;
import java.util.WeakHashMap;
import net.minecraft.client.resources.sounds.SoundInstance;
import net.minecraft.client.resources.sounds.TickableSoundInstance;
import net.minecraft.world.phys.Vec3;

/**
 * Sound takes time to arrive: about 3 ms a block, so a block broken 60 blocks away is heard a sixth of a second after
 * it is seen to break, as in life. The game plays every sound the moment it happens; with realistic distance, Earshot
 * holds a one-shot back by its distance over the speed of sound, to the nearest tick. Loops and sounds that follow
 * something (a minecart, a bee) start at once: the game checks those are still playing, and a held one isn't yet.
 * Thunder has its own, longer hold ({@link Thunder}).
 *
 * <p>Held sounds wait in Earshot's own queue, not the game's: the game starts its queued sounds while walking that
 * queue, and a sound starting can start another (Foley voicing a mob's call), which would be held in turn, into the
 * queue being walked, and crash the game. Earshot's queue is let out at the start of each sound tick, from a copy.
 */
public final class Propagation {
	/** The sounds already held back, which play as they come back. */
	private static final Set<SoundInstance> HELD = Collections.synchronizedSet(Collections.newSetFromMap(new WeakHashMap<>()));

	/** A held sound and the ticks it still waits. */
	private static final class Waiting {
		final SoundInstance sound;
		int ticks;

		Waiting(SoundInstance sound, int ticks) {
			this.sound = sound;
			this.ticks = ticks;
		}
	}

	private static final List<Waiting> QUEUE = new ArrayList<>();

	/**
	 * Whether held sounds are being let out now (on the client thread). A sound started meanwhile was started by one
	 * of them (Foley voicing it, or layering a strike under it): it has already waited, and isn't held again.
	 */
	private static boolean releasing;

	private Propagation() {}

	/** Holds {@code sound} back {@code ticks} ticks. */
	public static synchronized void hold(SoundInstance sound, int ticks) {
		QUEUE.add(new Waiting(sound, ticks));
	}

	/** One tick on: the held sounds now due, taken out of the queue. */
	public static synchronized List<SoundInstance> due() {
		// Almost every tick holds nothing, and the queue is empty almost all the time: nothing to count down, and no
		// list worth making. The game calls this at the head of every sound tick.
		if (QUEUE.isEmpty()) return List.of();
		List<SoundInstance> due = new ArrayList<>();
		QUEUE.removeIf(waiting -> {
			if (--waiting.ticks > 0) return false;
			due.add(waiting.sound);
			return true;
		});
		return due;
	}

	/** Plays {@code play}'s sounds as let out: anything they start isn't held again. Client thread. */
	public static void release(Runnable play) {
		releasing = true;
		try {
			play.run();
		} finally {
			releasing = false;
		}
	}

	/** Whether held sounds are being let out right now. Client thread. */
	public static boolean releasing() {
		return releasing;
	}

	/** Forgets {@code sound} if it is held: it was stopped before it started. */
	public static synchronized void cancel(SoundInstance sound) {
		QUEUE.removeIf(waiting -> waiting.sound == sound);
	}

	/** Forgets every held sound, as when the game stops them all. */
	public static synchronized void clear() {
		QUEUE.clear();
		HELD.clear();
	}

	/** How many ticks to hold {@code sound} back for a listener at {@code ear}, or 0 to play it now. */
	public static int holdTicks(SoundInstance sound, Vec3 ear) {
		if (sound.isRelative() || sound.isLooping() || sound instanceof TickableSoundInstance
				|| sound.getAttenuation() == SoundInstance.Attenuation.NONE || HELD.remove(sound)) {
			return 0;
		}
		int ticks = Thunder.delayTicks(ear.distanceTo(new Vec3(sound.getX(), sound.getY(), sound.getZ())));
		if (ticks > 0) HELD.add(sound);
		return ticks;
	}
}
