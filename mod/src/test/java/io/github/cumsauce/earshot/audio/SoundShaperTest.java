package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.cumsauce.earshot.natives.NativeBridge;
import java.util.ArrayList;
import java.util.List;
import net.minecraft.world.phys.Vec3;
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

	/** A sound at a point the native engine cannot use is not heard — one bad mod must not silence the rest. */
	@Test
	void an_unusable_position_is_not_heard() {
		List<SoundShaper.Target> targets = List.of(
				new SoundShaper.Target(1, new SourceState(), new Vec3(4, 64, -8), null, 16),
				new SoundShaper.Target(2, new SourceState(), new Vec3(Double.NaN, 1, 2), null, 16),
				new SoundShaper.Target(3, new SourceState(), new Vec3(0, -30, 9), null, 16));
		List<SoundShaper.Target> dropped = new ArrayList<>();

		List<SoundShaper.Target> heard = SoundShaper.heardTargets(targets, dropped);

		assertEquals(2, heard.size(), "the two at points the native engine can use are still heard");
		assertEquals(1, heard.get(0).source());
		assertEquals(3, heard.get(1).source());
		assertEquals(1, dropped.size(), "the NaN-positioned one is dropped, to have its shaping released");
		assertEquals(2, dropped.get(0).source());
	}

	/** The predicate mirrors the native Vec3::is_usable: finite coordinates, each within ±1e9 inclusive. */
	@Test
	void the_usable_predicate_matches_the_native_one() {
		assertTrue(SourceState.usable(new Vec3(0, 0, 0)));
		assertTrue(SourceState.usable(new Vec3(1e9, -1e9, 1e9)));
		assertFalse(SourceState.usable(new Vec3(Double.NaN, 0, 0)));
		assertFalse(SourceState.usable(new Vec3(0, Double.POSITIVE_INFINITY, 0)));
		assertFalse(SourceState.usable(new Vec3(0, 0, Double.NEGATIVE_INFINITY)));
		assertFalse(SourceState.usable(new Vec3(1e300, 0, 0)), "past the native bound, as 1e9 is not");
		assertFalse(SourceState.usable(null));
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

	/** Leaving convolution lets its slots go: off that mode they would keep convolving with nothing sending. */
	@Test
	void leaving_convolution_frees_its_slots() {
		assertFalse(SoundShaper.freesConvolution(SoundShaper.ReverbMode.CONVOLUTION));
		assertTrue(SoundShaper.freesConvolution(SoundShaper.ReverbMode.EAX));
		assertTrue(SoundShaper.freesConvolution(SoundShaper.ReverbMode.OFF));
	}

	/**
	 * A failed pass must take sends 0 and 1 off every target that holds them, not only own-reverb owners: the
	 * listener's reverb slots sit on an ordinary target's sends too, and OpenAL will not delete a slot a source
	 * still sends to.
	 */
	@Test
	void every_target_with_a_send_filter_has_its_sends_taken_off() {
		SourceState ordinary = new SourceState();
		ordinary.sendFilter = 4;
		SourceState owner = new SourceState();
		owner.sendFilter = 5;
		SourceState neverSent = new SourceState();
		List<SoundShaper.Target> targets = List.of(
				new SoundShaper.Target(11, ordinary, new Vec3(1, 2, 3), null, 16),
				new SoundShaper.Target(22, owner, new Vec3(4, 5, 6), null, 16),
				new SoundShaper.Target(33, neverSent, new Vec3(7, 8, 9), null, 16));

		List<SoundShaper.Target> holders = SoundShaper.sendsToDetach(targets);

		assertEquals(List.of(11, 22), holders.stream().map(SoundShaper.Target::source).toList(),
				"the ordinary target's sends hold the listener's reverb as much as the own-reverb owner's do");
		assertTrue(SoundShaper.sendsToDetach(List.of()).isEmpty(), "with no targets there is nothing to detach");
	}

	/** The mixer's filter cannot pass one, so a gain above one must ride the source's own gain instead. */
	@Test
	void a_gain_above_one_rides_the_source_gain() {
		// An ordinary gain takes no lift: the filter carries it all, as before.
		assertEquals(1f, SoundShaper.lift(new float[] {0.5f, 0.3f, 0.2f}), 1e-6f);
		assertEquals(1f, SoundShaper.lift(new float[] {1f, 1f, 1f}), 1e-6f);
		// The soil ground's mid-band crest over open ground: the lift is the peak band.
		assertEquals(1.44f, SoundShaper.lift(new float[] {0.7f, 1.44f, 0.5f}), 1e-6f);
		// Nothing lifts past the hard ground's double.
		assertEquals(2f, SoundShaper.lift(new float[] {2.5f, 0.1f, 0.1f}), 1e-6f);
	}
}
