package io.github.cumsauce.earshot.openal;

import java.nio.IntBuffer;
import org.lwjgl.openal.ALC11;
import org.lwjgl.openal.EXTEfx;
import org.lwjgl.system.MemoryStack;

/**
 * The game's context attributes plus what Earshot needs:
 * <ul>
 * <li>{@value #SENDS} auxiliary sends per source: the game asks for none, which leaves OpenAL Soft's default of 2, and
 * reverb needs 4 (two listener slots to crossfade, two for the spaces sources are in; docs/PLAN.md);
 * <li>{@value #VOICE_SOURCES} sources for Yak VC's voices beyond what the game takes. The game sizes its source pools
 * from the context's mono sources and takes all of them up to {@value #GAME_SOURCES} (255 for sounds, 8 for streams),
 * so asking for that many more leaves those free.
 * </ul>
 */
public final class ContextAttributes {
	public static final int SENDS = 4;
	/** The most sources the game ever takes. */
	public static final int GAME_SOURCES = 263;
	public static final int VOICE_SOURCES = 32;

	private ContextAttributes() {}

	/** A copy of {@code attributes} (key, value pairs ending in 0) with Earshot's set, on {@code stack}. */
	public static IntBuffer withEarshotAttributes(MemoryStack stack, IntBuffer attributes) {
		IntBuffer out = stack.mallocInt(attributes.remaining() + 4);
		for (int i = attributes.position(); i + 1 < attributes.limit() && attributes.get(i) != 0; i += 2) {
			int key = attributes.get(i);
			if (key == EXTEfx.ALC_MAX_AUXILIARY_SENDS || key == ALC11.ALC_MONO_SOURCES) continue;
			out.put(key).put(attributes.get(i + 1));
		}
		return out.put(EXTEfx.ALC_MAX_AUXILIARY_SENDS).put(SENDS)
				.put(ALC11.ALC_MONO_SOURCES).put(GAME_SOURCES + VOICE_SOURCES)
				.put(0).flip();
	}
}
