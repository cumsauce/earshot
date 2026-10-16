package io.github.cumsauce.earshot.openal;

import java.nio.IntBuffer;
import org.lwjgl.openal.ALC10;
import org.lwjgl.openal.ALC11;
import org.lwjgl.openal.EXTEfx;
import org.lwjgl.openal.SOFTHRTF;
import org.lwjgl.system.MemoryStack;

/**
 * The game's context attributes plus what Earshot needs:
 * <ul>
 * <li>{@value #SENDS} auxiliary sends per source: the game asks for none, which leaves OpenAL Soft's default of 2, and
 * reverb needs 4 (two listener slots to crossfade, two for the spaces sources are in; docs/PLAN.md);
 * <li>{@value #VOICE_SOURCES} sources for Yak VC's voices beyond what the game takes. The game sizes its source pools
 * from the context's mono sources and takes all of them up to {@value #GAME_SOURCES} (255 for sounds, 8 for streams),
 * so asking for that many more leaves those free;
 * <li>the HRTF dataset picked in the settings, when the game asks for HRTF (Directional Audio) and OpenAL lists it;
 * with none picked, OpenAL Soft's built-in one, which it would otherwise give up for the first data set it finds (the
 * ones Earshot unpacks, {@link HrtfFiles}).
 * </ul>
 */
public final class ContextAttributes {
	public static final int SENDS = 4;
	/** The most sources the game ever takes. */
	public static final int GAME_SOURCES = 263;
	public static final int VOICE_SOURCES = 32;

	private ContextAttributes() {}

	/**
	 * A copy of {@code attributes} (key, value pairs ending in 0) with Earshot's set, on {@code stack}.
	 *
	 * @param device the device the context is for
	 * @param hrtf   whether the game asked for HRTF
	 * @param hrtfName the HRTF dataset to use, or empty for OpenAL's default
	 */
	public static IntBuffer withEarshotAttributes(MemoryStack stack, IntBuffer attributes, long device, boolean hrtf,
			String hrtfName) {
		int hrtfId = hrtf ? hrtfIndex(device, hrtfName) : -1;
		IntBuffer out = stack.mallocInt(attributes.remaining() + 8);
		for (int i = attributes.position(); i + 1 < attributes.limit() && attributes.get(i) != 0; i += 2) {
			int key = attributes.get(i);
			if (key == EXTEfx.ALC_MAX_AUXILIARY_SENDS || key == ALC11.ALC_MONO_SOURCES
					|| key == org.lwjgl.openal.SOFTOutputLimiter.ALC_OUTPUT_LIMITER_SOFT) {
				continue;
			}
			if (key == SOFTHRTF.ALC_HRTF_ID_SOFT && hrtfId >= 0) continue;
			out.put(key).put(attributes.get(i + 1));
		}
		// OpenAL Soft's output limiter: many sounds and their reverbs together never clip the speakers.
		out.put(org.lwjgl.openal.SOFTOutputLimiter.ALC_OUTPUT_LIMITER_SOFT).put(ALC10.ALC_TRUE);
		out.put(EXTEfx.ALC_MAX_AUXILIARY_SENDS).put(SENDS).put(ALC11.ALC_MONO_SOURCES).put(GAME_SOURCES + VOICE_SOURCES);
		if (hrtfId >= 0) out.put(SOFTHRTF.ALC_HRTF_ID_SOFT).put(hrtfId);
		return out.put(0).flip();
	}

	/** OpenAL Soft's own HRTF, as it lists it. */
	public static final String BUILT_IN_HRTF = "Built-In HRTF";

	/**
	 * The index of the HRTF dataset {@code name} on {@code device} ({@value #BUILT_IN_HRTF} for an empty name), or -1
	 * if it isn't listed.
	 */
	private static int hrtfIndex(long device, String name) {
		if (device == 0 || !ALC10.alcIsExtensionPresent(device, "ALC_SOFT_HRTF")) return -1;
		if (name.isEmpty()) name = BUILT_IN_HRTF;
		int count = ALC10.alcGetInteger(device, SOFTHRTF.ALC_NUM_HRTF_SPECIFIERS_SOFT);
		for (int i = 0; i < count; i++) {
			if (name.equals(SOFTHRTF.alcGetStringiSOFT(device, SOFTHRTF.ALC_HRTF_SPECIFIER_SOFT, i))) return i;
		}
		// Fall back to the MHR (named "SADIE II KU100 64-tap" since the MHRs are renamed so
		// the bare name is reserved for the SOFA). The SOFA, if picked, is downloaded on
		// first use by HrtfFiles.ensure() before the device is opened.
		String fallback = name + " 64-tap";
		for (int i = 0; i < count; i++) {
			if (fallback.equals(SOFTHRTF.alcGetStringiSOFT(device, SOFTHRTF.ALC_HRTF_SPECIFIER_SOFT, i))) return i;
		}
		return -1;
	}
}
