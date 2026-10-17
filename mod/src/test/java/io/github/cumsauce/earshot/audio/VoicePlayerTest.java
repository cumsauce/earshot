package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;

import net.minecraft.world.phys.Vec3;
import org.junit.jupiter.api.Test;

/**
 * The distances {@link VoicePlayer} fades a voice out at and the directions it points, read without an OpenAL
 * context: OpenAL would reject a NaN max distance or direction with AL_INVALID_VALUE into the context's shared
 * error state, where a later bracketed caller could blame its own window for it.
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

	/** A bad rotation can give a facing of NaN or infinity: none of those may reach OpenAL as a direction. */
	@Test
	void aVoiceWithAnUnusableFacingStaysOmnidirectional() {
		assertNull(VoicePlayer.direction(new Vec3(Double.NaN, 0, 0)));
		assertNull(VoicePlayer.direction(new Vec3(0, Double.NaN, 0)));
		assertNull(VoicePlayer.direction(new Vec3(0, 0, Double.NaN)));
		assertNull(VoicePlayer.direction(new Vec3(Double.POSITIVE_INFINITY, 0, 0)));
		assertNull(VoicePlayer.direction(new Vec3(0, Double.POSITIVE_INFINITY, 0)));
		assertNull(VoicePlayer.direction(new Vec3(0, 0, Double.POSITIVE_INFINITY)));
		assertNull(VoicePlayer.direction(new Vec3(Double.NEGATIVE_INFINITY, 0, 0)));
		assertNull(VoicePlayer.direction(new Vec3(0, Double.NEGATIVE_INFINITY, 0)));
		assertNull(VoicePlayer.direction(new Vec3(0, 0, Double.NEGATIVE_INFINITY)));
		assertNull(VoicePlayer.direction(null));
	}

	/** A normalised valid facing passes through unchanged, so the voice still points the way its speaker faces. */
	@Test
	void aUsableFacingStillPointsTheVoice() {
		Vec3 facing = new Vec3(0, 0, -1);
		assertSame(facing, VoicePlayer.direction(facing));
		Vec3 other = new Vec3(1, 0, 0);
		assertSame(other, VoicePlayer.direction(other));
	}
}
