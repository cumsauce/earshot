package io.github.cumsauce.earshot.audio;

import java.util.Collections;
import java.util.Set;
import java.util.WeakHashMap;
import net.minecraft.client.resources.sounds.SoundInstance;
import net.minecraft.sounds.SoundEvents;
import net.minecraft.world.phys.Vec3;

/**
 * Thunder arrives as late as sound takes to come from the bolt: about three seconds a kilometre after the flash. The
 * game plays it at once; Earshot holds it back by distance over the speed of sound. Air absorption already takes the
 * highs off far thunder.
 */
public final class Thunder {
	/**
	 * The air's speed of sound, blocks (metres) a second: the listener's climate as {@link SoundShaper} last read
	 * it, 20 °C until then. Read on the client thread, written on the sound thread.
	 */
	private static volatile double speedOfSound = Air.speedOfSound(20);
	/** The sounds already held back, which play as they come back. */
	private static final Set<SoundInstance> HELD = Collections.synchronizedSet(Collections.newSetFromMap(new WeakHashMap<>()));

	private Thunder() {}

	/** Whether {@code sound} is a thunderclap. */
	public static boolean isThunder(SoundInstance sound) {
		return sound.getIdentifier().equals(SoundEvents.LIGHTNING_BOLT_THUNDER.location());
	}

	/**
	 * How many ticks to hold {@code sound} back for a listener at {@code ear}, or 0 to play it now: when it isn't
	 * thunder, is near, or has been held back already.
	 */
	public static int holdTicks(SoundInstance sound, Vec3 ear) {
		if (!isThunder(sound) || sound.isRelative() || HELD.remove(sound)) return 0;
		int ticks = delayTicks(ear.distanceTo(new Vec3(sound.getX(), sound.getY(), sound.getZ())));
		if (ticks > 0) HELD.add(sound);
		return ticks;
	}

	/** The air the listener is in has turned {@code celsius}: sound now covers distance at its speed. */
	static void air(double celsius) {
		speedOfSound = Air.speedOfSound(celsius);
	}

	/** Game ticks sound takes to cover {@code distance} blocks in the air as it is now. */
	static int delayTicks(double distance) {
		return delayTicks(distance, speedOfSound);
	}

	/** Game ticks sound takes to cover {@code distance} blocks at {@code speed} blocks a second. */
	static int delayTicks(double distance, double speed) {
		return (int) Math.round(distance / speed * 20);
	}
}
