package io.github.cumsauce.earshot.audio;

import net.minecraft.world.phys.Vec3;

/**
 * One source's shaping, smoothed from tick to tick so nothing clicks: the gains per band, and how far from where the
 * game puts it the sound seems to come from (an offset, so a moving source isn't dragged behind).
 */
public final class SourceState {
	/** The share of the way to a new value each tick moves: about 100 ms to settle at 20 ticks a second. */
	private static final float SMOOTHING = 0.5f;

	/** The OpenAL filter for the direct path, 0 until made. */
	int filter;
	/** The OpenAL filter for the send to the room's reverb, 0 until made. */
	int sendFilter;
	private boolean started;
	private final float[] gains = {1, 1, 1};
	private Vec3 offset = Vec3.ZERO;

	/** Moves towards {@code target} gains and the heard-from point {@code heardFrom} of a sound at {@code position}. */
	void approach(float[] target, Vec3 heardFrom, Vec3 position) {
		Vec3 targetOffset = heardFrom.subtract(position);
		if (!started) {
			started = true;
			System.arraycopy(target, 0, gains, 0, gains.length);
			offset = targetOffset;
			return;
		}
		for (int band = 0; band < gains.length; band++) {
			gains[band] += (target[band] - gains[band]) * SMOOTHING;
		}
		offset = offset.lerp(targetOffset, SMOOTHING);
	}

	boolean started() {
		return started;
	}

	/** The smoothed gains, low to high. */
	float[] gains() {
		return gains.clone();
	}

	/** Where the sound seems to come from, for a sound the game puts at {@code position}. */
	Vec3 heardFrom(Vec3 position) {
		return position.add(offset);
	}
}
