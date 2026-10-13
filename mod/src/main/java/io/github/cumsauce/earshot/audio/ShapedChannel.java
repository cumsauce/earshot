package io.github.cumsauce.earshot.audio;

import net.minecraft.world.phys.Vec3;
import org.jspecify.annotations.Nullable;

/** What Earshot keeps on each of the game's OpenAL channels; implemented by {@code ChannelMixin}. */
public interface ShapedChannel {
	/** The OpenAL source name. */
	int earshot$source();

	/** Where the game puts the sound, or null before it says. */
	@Nullable Vec3 earshot$position();

	/**
	 * A sound out in the world, not at the listener's head; music and menu sounds aren't. Walls and rooms shape it,
	 * whether or not it fades with distance (a placed bed, such as Foley's water, sets its own level).
	 */
	boolean earshot$positional();

	/**
	 * How far the game lets the sound carry, in blocks: where its linear falloff reaches silence; 0 for a sound that
	 * doesn't fade with distance.
	 */
	float earshot$range();

	/** Earshot's own state for the source. */
	SourceState earshot$state();
}
