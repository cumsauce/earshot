package io.github.cumsauce.earshot.audio;

import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.natives.NativeBridge;
import java.lang.foreign.MemorySegment;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import org.lwjgl.openal.AL10;
import org.lwjgl.openal.EXTEfx;

/**
 * Convolution reverb (M3, the High tier): impulse responses synthesized from the room measured around the listener,
 * played through OpenAL Soft's in-progress convolution effect.
 *
 * <p>There are two slots, the one playing and one a new response loads into. Sources send to both, and when a new
 * response is ready the slot gains crossfade over about a quarter of a second, so an update never clicks; the slot
 * faded out then lets its effect go, so only one convolution runs at a time (about 10 % of a core on an M3, docs/PLAN.md).
 * OpenAL Soft takes 10–20 ms to prepare a response, so loading runs on Earshot's own thread, never the game's.
 *
 * <p>The responses are in world axes; the effect's orientation turns them as the listener turns, so reflections stay
 * where the walls are.
 */
public final class ConvolutionReverb {
	/** The response's level: convolution sums every sound's send, so it is held at half. */
	private static final float LEVEL = 0.5f;

	/** {@code AL_SOFT_convolution_effect}, in-progress in OpenAL Soft and so not in LWJGL. */
	private static final int AL_EFFECT_CONVOLUTION_SOFT = 0xA000;
	private static final int AL_CONVOLUTION_ORIENTATION_SOFT = 0x100F;
	private static final int AL_FORMAT_BFORMAT3D_FLOAT32 = 0x20032;
	private static final int SAMPLE_RATE = 48_000;
	/** The longest response: longer ones double the cost (docs/PLAN.md, M0). */
	private static final float MAX_SECONDS = 1.5f;
	/** At most this often a new response is considered. */
	private static final long CHECK_EVERY_NANOS = 500_000_000L;
	/** The crossfade moves this much a tick: 5 ticks. */
	private static final float FADE_STEP = 0.2f;
	/** The orientation is set again once the listener turns this far (cosine of about 3°). */
	private static final float TURNED = 0.9986f;

	/** Makes a response from what it depends on (a room, a source's place) and a seed; empty when it can't yet. */
	@FunctionalInterface
	public interface Maker {
		float[] make(double[] madeFrom, long seed);
	}

	/** Whether a response made from {@code madeFrom} is out of date for {@code now}. */
	@FunctionalInterface
	public interface Changed {
		boolean changed(double[] madeFrom, double[] now);
	}

	private final Maker maker;
	private final Changed changed;
	private final ExecutorService loader = Executors.newSingleThreadExecutor(task -> {
		Thread thread = new Thread(task, "earshot-ir");
		thread.setDaemon(true);
		return thread;
	});

	// All of the below is guarded by `this`.
	private final int[] slots = new int[2];
	private final int[] effects = new int[2];
	private final int[] buffers = new int[2];
	/** The slot playing, or -1. */
	private int active = -1;
	/** The slot a new response is fading in on, or -1. */
	private int incoming = -1;
	private float fade;
	private boolean loading;
	private long checkedAt;
	/** The room the response playing (or loading) was made from. */
	private double[] madeFrom = new double[0];
	/** Bumped when the game closes its context, so a load in flight for the old one is dropped. */
	private int generation;
	private final float[] orientation = {0, 0, -1, 0, 1, 0};
	private boolean failed;
	private long seed;
	private int loads;

	/** The listener's reverb: responses for the room around the listener, made again as it changes. */
	public ConvolutionReverb(NativeBridge bridge, MemorySegment engine) {
		this((room, seed) -> bridge.impulseResponse(engine, SAMPLE_RATE, MAX_SECONDS, seed),
				ConvolutionReverb::roomChanged);
	}

	public ConvolutionReverb(Maker maker, Changed changed) {
		this.maker = maker;
		this.changed = changed;
	}

	/** The two slots to send to, or zeros before they exist. On the sound thread. */
	public synchronized int[] slots() {
		return slots.clone();
	}

	/** How many responses have been loaded, for tests. */
	public synchronized int loads() {
		return loads;
	}

	/** Whether a response is playing. */
	public synchronized boolean playing() {
		return active >= 0;
	}

	/**
	 * Called every tick on the sound thread: makes the slots if this context has none, moves any crossfade on,
	 * turns the responses with the listener, and starts loading a new response when the room has changed.
	 *
	 * @param room        what the response depends on: for the listener's, the room ({@link NativeBridge#room}'s
	 *                    doubles)
	 * @param orientation the world's -z and +y axes in the listener's frame: at x, y, z, then up x, y, z
	 */
	public synchronized void tick(double[] room, float[] orientation) {
		if (failed) return;
		if (slots[0] == 0 && !make()) return;
		if (incoming >= 0) crossfade();
		if (turned(orientation)) {
			System.arraycopy(orientation, 0, this.orientation, 0, 6);
			for (int i : new int[] {active, incoming}) {
				if (i < 0) continue;
				EXTEfx.alEffectfv(effects[i], AL_CONVOLUTION_ORIENTATION_SOFT, this.orientation);
				EXTEfx.alAuxiliaryEffectSloti(slots[i], EXTEfx.AL_EFFECTSLOT_EFFECT, effects[i]);
			}
		}
		long now = System.nanoTime();
		if (!loading && incoming < 0 && now - checkedAt >= CHECK_EVERY_NANOS
				&& (madeFrom.length == 0 ? room.length > 0 && maker != null : changed.changed(madeFrom, room))) {
			checkedAt = now;
			loading = true;
			madeFrom = room.clone();
			double[] from = madeFrom;
			int into = active == 0 ? 1 : 0;
			int forGeneration = generation;
			long forSeed = ++seed;
			loader.execute(() -> load(into, forGeneration, forSeed, from));
		}
	}

	/**
	 * Lets go of the slots, effects and responses (on the sound thread), for when nothing will send to them: an idle
	 * convolution still costs its share of the mixer. The next tick makes them again.
	 */
	public synchronized void release() {
		if (slots[0] == 0) return;
		for (int i = 0; i < 2; i++) {
			EXTEfx.alAuxiliaryEffectSloti(slots[i], EXTEfx.AL_EFFECTSLOT_EFFECT, EXTEfx.AL_EFFECT_NULL);
			EXTEfx.alAuxiliaryEffectSloti(slots[i], AL10.AL_BUFFER, 0);
			EXTEfx.alDeleteAuxiliaryEffectSlots(slots[i]);
			EXTEfx.alDeleteEffects(effects[i]);
			if (buffers[i] != 0) AL10.alDeleteBuffers(buffers[i]);
		}
		int error = AL10.alGetError();
		if (error != AL10.AL_NO_ERROR) {
			// A source still sending to a slot keeps it alive: it leaks until the context goes.
			EarshotClient.LOGGER.warn("Releasing a convolution reverb failed (OpenAL error 0x{})",
					Integer.toHexString(error));
		}
		contextClosing();
	}

	/** The game is closing its context, which frees every object in it: forget them all. */
	public synchronized void contextClosing() {
		generation++;
		java.util.Arrays.fill(slots, 0);
		java.util.Arrays.fill(effects, 0);
		java.util.Arrays.fill(buffers, 0);
		active = -1;
		incoming = -1;
		madeFrom = new double[0];
	}

	private boolean make() {
		AL10.alGetError();
		for (int i = 0; i < 2; i++) {
			effects[i] = EXTEfx.alGenEffects();
			EXTEfx.alEffecti(effects[i], EXTEfx.AL_EFFECT_TYPE, AL_EFFECT_CONVOLUTION_SOFT);
			slots[i] = EXTEfx.alGenAuxiliaryEffectSlots();
			EXTEfx.alAuxiliaryEffectSlotf(slots[i], EXTEfx.AL_EFFECTSLOT_GAIN, 0f);
		}
		int error = AL10.alGetError();
		if (error != AL10.AL_NO_ERROR) {
			failed = true;
			EarshotClient.LOGGER.warn("Convolution reverb is off: making its slots failed (OpenAL error 0x{})",
					Integer.toHexString(error));
			return false;
		}
		return true;
	}

	/**
	 * On the loader thread: makes a response and loads it into slot {@code into}, then hands it to the sound thread
	 * to fade in. The OpenAL calls run outside the lock: the sound thread doesn't touch this slot until it is handed
	 * over.
	 */
	private void load(int into, int forGeneration, long forSeed, double[] from) {
		try {
			float[] ir = maker.make(from, forSeed);
			int slot;
			int effect;
			synchronized (this) {
				if (forGeneration != generation || ir.length == 0) {
					loading = false;
					// Nothing to make it from yet: try again next time.
					if (ir.length == 0) madeFrom = new double[0];
					return;
				}
				slot = slots[into];
				effect = effects[into];
			}
			int buffer = AL10.alGenBuffers();
			AL10.alBufferData(buffer, AL_FORMAT_BFORMAT3D_FLOAT32, ir, SAMPLE_RATE);
			// Buffer first, then the effect, as OpenAL Soft's own example does.
			EXTEfx.alAuxiliaryEffectSloti(slot, AL10.AL_BUFFER, buffer);
			EXTEfx.alEffectfv(effect, AL_CONVOLUTION_ORIENTATION_SOFT, orientation());
			EXTEfx.alAuxiliaryEffectSloti(slot, EXTEfx.AL_EFFECTSLOT_EFFECT, effect);
			int error = AL10.alGetError();
			synchronized (this) {
				loading = false;
				if (forGeneration != generation) return;
				if (error != AL10.AL_NO_ERROR) {
					failed = true;
					EarshotClient.LOGGER.warn("Convolution reverb is off: loading a response failed (OpenAL error 0x{})",
							Integer.toHexString(error));
					return;
				}
				if (buffers[into] != 0) AL10.alDeleteBuffers(buffers[into]);
				buffers[into] = buffer;
				incoming = into;
				fade = 0;
				loads++;
			}
		} catch (RuntimeException e) {
			synchronized (this) {
				loading = false;
			}
			EarshotClient.LOGGER.warn("Making an impulse response failed", e);
		}
	}

	private synchronized float[] orientation() {
		return orientation.clone();
	}

	/** One step of the crossfade to the incoming slot; at the end the old slot lets its effect and response go. */
	private void crossfade() {
		fade = Math.min(1f, fade + FADE_STEP);
		// Every sound sends into the same response, so their sum is held well under full scale.
		EXTEfx.alAuxiliaryEffectSlotf(slots[incoming], EXTEfx.AL_EFFECTSLOT_GAIN, LEVEL * fade);
		if (active >= 0) EXTEfx.alAuxiliaryEffectSlotf(slots[active], EXTEfx.AL_EFFECTSLOT_GAIN, LEVEL * (1f - fade));
		if (fade < 1f) return;
		if (active >= 0) {
			EXTEfx.alAuxiliaryEffectSloti(slots[active], EXTEfx.AL_EFFECTSLOT_EFFECT, EXTEfx.AL_EFFECT_NULL);
			EXTEfx.alAuxiliaryEffectSloti(slots[active], AL10.AL_BUFFER, 0);
			AL10.alDeleteBuffers(buffers[active]);
			buffers[active] = 0;
		}
		active = incoming;
		incoming = -1;
	}

	private boolean turned(float[] next) {
		float at = next[0] * orientation[0] + next[1] * orientation[1] + next[2] * orientation[2];
		float up = next[3] * orientation[3] + next[4] * orientation[4] + next[5] * orientation[5];
		return at < TURNED || up < TURNED;
	}

	/**
	 * Whether {@code room} differs enough from the one the playing response was made from to make another: the mid
	 * decay by 10 %, the enclosure by 0.1, or the mean free path by 15 %.
	 */
	static boolean roomChanged(double[] madeFrom, double[] room) {
		return Math.abs(Math.log(room[1] / madeFrom[1])) > Math.log(1.1)
				|| Math.abs(room[4] - madeFrom[4]) > 0.1
				|| Math.abs(Math.log((room[3] + 1) / (madeFrom[3] + 1))) > Math.log(1.15);
	}
}
