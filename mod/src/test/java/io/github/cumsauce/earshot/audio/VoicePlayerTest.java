package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

/**
 * The distance {@link VoicePlayer} fades a voice out at, read without an OpenAL context: OpenAL would reject a NaN
 * max distance with AL_INVALID_VALUE into the context's shared error state, where a later bracketed caller could
 * blame its own window for it.
 */
class VoicePlayerTest {
	/** Yak VC's range can be NaN, infinite or non-positive: none of those may reach OpenAL as a max distance. */
	@Test
	void aVoiceWithANanRangeKeepsTheDefaultDistance() {
		assertEquals(16, VoicePlayer.maxDistance(Float.NaN));
		assertEquals(16, VoicePlayer.maxDistance(Float.POSITIVE_INFINITY));
		assertEquals(16, VoicePlayer.maxDistance(Float.NEGATIVE_INFINITY));
		assertEquals(16, VoicePlayer.maxDistance(0));
		assertEquals(16, VoicePlayer.maxDistance(-1));
	}

	/** A usable range passes through, with full volume distance plus one as the floor, as before. */
	@Test
	void aUsableRangeStillFadesFromTheFloor() {
		assertEquals(5, VoicePlayer.maxDistance(3));
		assertEquals(8, VoicePlayer.maxDistance(8));
		assertEquals(32, VoicePlayer.maxDistance(32));
	}
}
