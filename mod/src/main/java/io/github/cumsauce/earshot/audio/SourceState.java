package io.github.cumsauce.earshot.audio;

import net.minecraft.world.phys.Vec3;
import org.jspecify.annotations.Nullable;

/**
 * One source's shaping, smoothed from tick to tick so nothing clicks: the gains per band, and how far from where the
 * game puts it the sound seems to come from (an offset, so a moving source isn't dragged behind).
 */
public final class SourceState {
	/** The share of the way to a new value each tick moves: about 100 ms to settle at 20 ticks a second. */
	private static final float SMOOTHING = 0.5f;
	/** Faster than this, in blocks a second, a source jumped (teleported, or was moved by the game) rather than moved. */
	static final double MAX_SPEED = 80;
	/**
	 * How far from the origin a coordinate may be for the native engine, as its {@code Vec3::is_usable}'s bound
	 * (inclusive). A point past it fails the native call and stops every source's shaping, so it is not heard.
	 */
	static final double MAX_COORDINATE = 1e9;

	/**
	 * Whether the native engine can use this point at all, as its {@code Vec3::is_usable} does: finite, with no
	 * coordinate past {@value #MAX_COORDINATE} in magnitude (the bound inclusive). A sound placed at a point it
	 * cannot use fails the native call and silences every source's shaping, so such points are not heard.
	 */
	static boolean usable(@Nullable Vec3 position) {
		return position != null && usable(position.x) && usable(position.y) && usable(position.z);
	}

	private static boolean usable(double coord) {
		return Double.isFinite(coord) && Math.abs(coord) <= MAX_COORDINATE;
	}

	/** The OpenAL filter for the direct path, 0 until made. */
	int filter;
	/** The OpenAL filter for the send to the room's reverb, 0 until made. */
	int sendFilter;
	/** The OpenAL filter for the send to another space's reverb, 0 until made, and whether that send is set. */
	int spaceFilter;
	boolean spaceSent;
	/** Whether the source sends to an early-reflections slot of its own. */
	boolean earlySent;
	/** Whether the source has Earshot's realistic distance falloff rather than the game's linear one. */
	boolean realistic;
	private boolean started;
	private final float[] gains = {1, 1, 1};
	private Vec3 offset = Vec3.ZERO;
	private final Motion motion = new Motion();

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

	/** How fast the source moves, in blocks a second, from where it was at the last update. */
	Vec3 velocity(Vec3 position, long nanos) {
		return motion.update(position, nanos);
	}

	/** A velocity estimated from successive positions, smoothed, and zeroed after a jump. */
	static final class Motion {
		private @Nullable Vec3 last;
		private long lastNanos;
		private Vec3 velocity = Vec3.ZERO;

		Vec3 update(Vec3 position, long nanos) {
			double seconds = (nanos - lastNanos) / 1e9;
			if (last != null && seconds > 0.005) {
				Vec3 moved = position.subtract(last).scale(1 / seconds);
				velocity = moved.length() > MAX_SPEED ? Vec3.ZERO : velocity.lerp(moved, SMOOTHING);
			}
			if (last == null || seconds > 0.005) {
				last = position;
				lastNanos = nanos;
			}
			return velocity;
		}
	}

	/** Where the sound seems to come from, for a sound the game puts at {@code position}. */
	Vec3 heardFrom(Vec3 position) {
		return position.add(offset);
	}
}
