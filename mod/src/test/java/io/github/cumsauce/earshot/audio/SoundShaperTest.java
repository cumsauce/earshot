package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
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

	/** A shaping pass that dies part way must stop shaping cleanly, not repeat half-applied every tick. */
	@Test
	void a_failed_update_stops_shaping() {
		boolean[] failed = {false};
		int[] cleanedUp = {0};
		// What update() passes as onFail: set failed (tick() checks it first and stops queueing passes) and
		// release the filters and sends the half-pass left attached.
		Runnable onFail = () -> {
			failed[0] = true;
			cleanedUp[0]++;
		};

		// A pass that throws — requireRoom's IllegalStateException, say. The throw must not escape: the sound
		// thread's executor would only log "Error executing task" and come back for the same broken pass next
		// tick, with half the sources still filtered and sent.
		boolean survived = SoundShaper.shapeSafely(() -> {
			throw new IllegalStateException("room has 0 doubles");
		}, onFail);
		assertFalse(survived, "a thrown pass must report failure");
		assertTrue(failed[0], "onFail must mark the shaper failed so tick() stops queueing passes");
		assertEquals(1, cleanedUp[0], "onFail must release what the half-pass left attached");

		// A pass that does not throw runs to the end and leaves onFail alone.
		boolean[] ran = {false};
		assertTrue(SoundShaper.shapeSafely(() -> ran[0] = true, onFail));
		assertTrue(ran[0], "a healthy pass must run");
		assertEquals(1, cleanedUp[0], "a healthy pass must not run onFail");
	}
}
