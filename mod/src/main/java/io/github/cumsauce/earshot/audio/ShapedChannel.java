package io.github.cumsauce.earshot.audio;

import net.minecraft.world.phys.Vec3;
import org.jspecify.annotations.Nullable;

/** What Earshot keeps on each of the game's OpenAL channels; implemented by {@code ChannelMixin}. */
public interface ShapedChannel {
	/** The OpenAL source name. */
	int earshot$source();

	/** Where the game puts the sound, or null before it says. */
	@Nullable Vec3 earshot$position();

	/** A sound out in the world that fades with distance; music and menu sounds aren't. */
	boolean earshot$positional();

	/** How far the game lets the sound carry, in blocks: where its linear falloff reaches silence. */
	float earshot$range();

	/** Earshot's own state for the source. */
	SourceState earshot$state();
}
