package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.cumsauce.earshot.natives.NativeBridge;
import org.junit.jupiter.api.Test;

/**
 * The native-boundary checks {@link SoundShaper} makes before it indexes the arrays the native calls fill: they
 * need no OpenAL context and no live engine, so they are read here.
 */
class SoundShaperTest {
	/** Every source's block is heard[at] … heard[at + 7], so the array must hold all of the last one. */
	@Test
	void heardIsOnlyShapedWhenItHoldsEverySource() {
		assertTrue(SoundShaper.heardHolds(new double[3 * NativeBridge.HEARD_DOUBLES], 3));
		assertTrue(SoundShaper.heardHolds(new double[0], 0));
		assertFalse(SoundShaper.heardHolds(new double[3 * NativeBridge.HEARD_DOUBLES - 1], 3),
				"one short of heard[at + 7] in the last block would throw mid-update");
		assertFalse(SoundShaper.heardHolds(new double[NativeBridge.HEARD_DOUBLES], 2));
		assertFalse(SoundShaper.heardHolds(new double[8], -1), "a negative count is not a count");
	}

	/** The EAX parameters are read straight out of a whole room: room[EAX_FIRST] … up to the last of them. */
	@Test
	void theEaxParametersAreOnlyReadFromAWholeRoom() {
		SoundShaper.requireRoom(new double[NativeBridge.ROOM_DOUBLES]);

		IllegalStateException shortRoom = assertThrows(IllegalStateException.class,
				() -> SoundShaper.requireRoom(new double[NativeBridge.ROOM_DOUBLES - 1]));
		assertTrue(shortRoom.getMessage().contains(Integer.toString(NativeBridge.ROOM_DOUBLES - 1)),
				shortRoom.getMessage());

		IllegalStateException longRoom = assertThrows(IllegalStateException.class,
				() -> SoundShaper.requireRoom(new double[NativeBridge.ROOM_DOUBLES + 1]));
		assertTrue(longRoom.getMessage().contains(Integer.toString(NativeBridge.ROOM_DOUBLES + 1)),
				longRoom.getMessage());
	}

	/** A context's close frees its slots, so the source ids recorded on them must not survive into the next. */
	@Test
	void theContextCloseForgetsTheSourceOwners() {
		int[] ownOwners = {11, 0};
		int[] earlyOwners = new int[EarlyReflections.SOURCES];
		earlyOwners[0] = 7;
		earlyOwners[1] = 3;
		boolean[] unusedOwn = {true, false};

		SoundShaper.forgetOwners(ownOwners, earlyOwners, unusedOwn);

		assertArrayEquals(new int[2], ownOwners);
		assertArrayEquals(new int[EarlyReflections.SOURCES], earlyOwners);
		assertArrayEquals(new boolean[2], unusedOwn);
	}
}
