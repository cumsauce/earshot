package io.github.cumsauce.earshot.audio;

import java.util.Collections;
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
 */
public final class Propagation {
	/** The sounds already held back, which play as they come back. */
	private static final Set<SoundInstance> HELD = Collections.synchronizedSet(Collections.newSetFromMap(new WeakHashMap<>()));

	private Propagation() {}

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
