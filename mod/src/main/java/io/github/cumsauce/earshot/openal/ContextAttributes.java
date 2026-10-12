package io.github.cumsauce.earshot.openal;

import java.nio.IntBuffer;
import org.lwjgl.openal.EXTEfx;
import org.lwjgl.system.MemoryStack;

/**
 * The game's context attributes plus {@value #SENDS} auxiliary sends per source: the game asks for none, which leaves
 * OpenAL Soft's default of 2, and reverb needs 4 (two listener slots to crossfade, two for the spaces sources are in;
 * docs/PLAN.md).
 */
public final class ContextAttributes {
	public static final int SENDS = 4;

	private ContextAttributes() {}

	/** A copy of {@code attributes} (key, value pairs ending in 0) with the auxiliary sends set, on {@code stack}. */
	public static IntBuffer withAuxiliarySends(MemoryStack stack, IntBuffer attributes) {
		IntBuffer out = stack.mallocInt(attributes.remaining() + 2);
		for (int i = attributes.position(); i + 1 < attributes.limit() && attributes.get(i) != 0; i += 2) {
			if (attributes.get(i) == EXTEfx.ALC_MAX_AUXILIARY_SENDS) continue;
			out.put(attributes.get(i)).put(attributes.get(i + 1));
		}
		return out.put(EXTEfx.ALC_MAX_AUXILIARY_SENDS).put(SENDS).put(0).flip();
	}
}
