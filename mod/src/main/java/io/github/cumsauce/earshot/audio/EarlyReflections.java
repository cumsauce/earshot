package io.github.cumsauce.earshot.audio;

import io.github.cumsauce.earshot.natives.NativeBridge;
import java.lang.foreign.MemorySegment;
import net.minecraft.world.phys.Vec3;
import org.lwjgl.openal.EXTEfx;

/**
 * Early reflections per source (M6, Ultra): the loudest few sounds each get the first echoes off the walls around
 * them, timed and panned from where they bounce. The listener's reverb has early reflections too, but as if every
 * sound were beside the listener; a sound across a corridor reflects off its own walls.
 *
 * <p>Each chosen sound gets an EAX slot of its own that plays only reflections (no late reverb), set from the
 * native first-order image sources, and sends to it through the last aux send. Runs on the sound thread, inside
 * {@link SoundShaper}'s update.
 */
final class EarlyReflections {
	/** How many sounds get their own early reflections. */
	static final int SOURCES = 4;
	/** The aux send for it: after the listener's two and the other spaces' one. */
	static final int SEND = 3;

	private final NativeBridge bridge;
	private final MemorySegment engine;
	private final int[] effects = new int[SOURCES];
	private final int[] slots = new int[SOURCES];

	EarlyReflections(NativeBridge bridge, MemorySegment engine) {
		this.bridge = bridge;
		this.engine = engine;
	}

	/** The game closed its context, and every slot and effect with it. */
	void contextClosing() {
		java.util.Arrays.fill(effects, 0);
		java.util.Arrays.fill(slots, 0);
	}

	/**
	 * Measures the early reflections of up to {@value #SOURCES} sounds at {@code positions} and sets a slot for each.
	 * Returns each one's slot, or 0 where it has no reflections.
	 *
	 * @param right the listener's right, {@code up} up and {@code forward} forward, in world axes
	 */
	int[] update(double[] listener, Vec3[] positions, Vec3 right, Vec3 up, Vec3 forward) {
		int count = Math.min(positions.length, SOURCES);
		int[] assigned = new int[count];
		if (count == 0) return assigned;
		double[] sources = new double[3 * count];
		for (int i = 0; i < count; i++) {
			sources[3 * i] = positions[i].x;
			sources[3 * i + 1] = positions[i].y;
			sources[3 * i + 2] = positions[i].z;
		}
		double[] early = new double[NativeBridge.EARLY_DOUBLES * count];
		bridge.early(engine, listener, sources, count, early);
		for (int i = 0; i < count; i++) {
			int at = NativeBridge.EARLY_DOUBLES * i;
			if (early[at + 5] == 0) continue;
			if (slots[i] == 0) make(i);
			Vec3 from = new Vec3(early[at + 2], early[at + 3], early[at + 4]);
			// EAX pans in its own left-handed frame: x right, y up, z forward.
			float[] pan = {(float) from.dot(right), (float) from.dot(up), (float) from.dot(forward)};
			int effect = effects[i];
			EXTEfx.alEffectf(effect, EXTEfx.AL_EAXREVERB_REFLECTIONS_GAIN,
					(float) Math.clamp(Math.sqrt(early[at]), 0, 1));
			EXTEfx.alEffectf(effect, EXTEfx.AL_EAXREVERB_REFLECTIONS_DELAY,
					(float) Math.clamp(early[at + 1], 0, EXTEfx.AL_EAXREVERB_MAX_REFLECTIONS_DELAY));
			EXTEfx.alEffectfv(effect, EXTEfx.AL_EAXREVERB_REFLECTIONS_PAN, pan);
			EXTEfx.alAuxiliaryEffectSloti(slots[i], EXTEfx.AL_EFFECTSLOT_EFFECT, effect);
			assigned[i] = slots[i];
		}
		return assigned;
	}

	/** Lets every slot and effect go, once no source sends to them (at the end of an update). */
	void free() {
		for (int i = 0; i < SOURCES; i++) {
			if (slots[i] == 0) continue;
			EXTEfx.alAuxiliaryEffectSloti(slots[i], EXTEfx.AL_EFFECTSLOT_EFFECT, EXTEfx.AL_EFFECT_NULL);
			EXTEfx.alDeleteAuxiliaryEffectSlots(slots[i]);
			EXTEfx.alDeleteEffects(effects[i]);
			slots[i] = 0;
			effects[i] = 0;
		}
	}

	/** An EAX reverb with reflections only, no late reverb. Never louder than the sound itself. */
	private void make(int i) {
		int effect = EXTEfx.alGenEffects();
		EXTEfx.alEffecti(effect, EXTEfx.AL_EFFECT_TYPE, EXTEfx.AL_EFFECT_EAXREVERB);
		// Diffuse: a real wall's echo is smeared a little, and discrete taps would comb.
		EXTEfx.alEffectf(effect, EXTEfx.AL_EAXREVERB_DIFFUSION, 1f);
		EXTEfx.alEffectf(effect, EXTEfx.AL_EAXREVERB_GAIN, 1f);
		EXTEfx.alEffectf(effect, EXTEfx.AL_EAXREVERB_DECAY_TIME, EXTEfx.AL_EAXREVERB_MIN_DECAY_TIME);
		EXTEfx.alEffectf(effect, EXTEfx.AL_EAXREVERB_LATE_REVERB_GAIN, 0f);
		EXTEfx.alEffectf(effect, EXTEfx.AL_EAXREVERB_ECHO_DEPTH, 0f);
		effects[i] = effect;
		slots[i] = EXTEfx.alGenAuxiliaryEffectSlots();
	}
}
