package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotSame;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.List;
import java.util.concurrent.AbstractExecutorService;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.Test;

/**
 * The loader executor's lifetime: a normal release keeps it, only a context close swaps it. The OpenAL side of
 * release() needs a live context, so these cover the Java-side split rather than the AL deletions.
 */
class ConvolutionReverbTest {
	private static ConvolutionReverb reverb() {
		return new ConvolutionReverb((madeFrom, seed) -> new float[0], (madeFrom, now) -> true);
	}

	/**
	 * Policy constants, asserted so a change has to face the measurement behind them
	 * (docs/investigations/2026-10-18-reverb-levels.md). Honest note: these assert constants, not behaviour —
	 * the evidence is the before/after probe numbers in that doc; there is no OpenAL seam to test the slot
	 * through.
	 */
	@Test
	void the_slot_is_not_held_below_the_rooms_own_reverb() {
		assertTrue(ConvolutionReverb.LEVEL >= 1.0f,
				"the response already carries the room's late energy (ir.rs normalises to late_energy) and the EAX "
						+ "slot runs at 1: LEVEL " + ConvolutionReverb.LEVEL + " attenuates the wet again");
	}

	@Test
	void a_response_may_ring_as_long_as_the_room() {
		assertTrue(ConvolutionReverb.MAX_SECONDS >= 3.0f,
				"a 5 s cave decay steepened 40 dB by " + ConvolutionReverb.MAX_SECONDS
						+ " s is the flat tail the report is about");
	}

	@Test
	void releasing_a_reverb_keeps_its_loader() {
		ConvolutionReverb reverb = reverb();
		var loader = reverb.loader();
		reverb.release();
		assertSame(loader, reverb.loader());
	}

	@Test
	void closing_the_context_swaps_the_loader() {
		ConvolutionReverb reverb = reverb();
		var loader = reverb.loader();
		reverb.contextClosing();
		assertNotSame(loader, reverb.loader(), "the next context needs a fresh executor");
		assertTrue(loader.isShutdown(), "a load queued against the closed context must not run");
	}

	/**
	 * A load tick() submitted can lose its race with the context closing: shutdownNow() drops the task, so load()
	 * never runs and never clears the flag. Only the close itself can, or the next context never loads again.
	 */
	@Test
	void context_closing_clears_a_load_that_never_ran() {
		ConvolutionReverb reverb = reverb();
		// A loader that takes its task and never runs it, the way a close's shutdownNow() leaves a queued
		// load dropped: nothing runs, so nothing clears the flag the load was submitted under.
		reverb.loader(discardingLoader());
		reverb.loading(true);
		reverb.contextClosing();
		assertFalse(reverb.loading(), "a load that never ran must not latch the reverb off");
	}

	/** An executor that keeps whatever is submitted to it and never runs any of it. */
	private static ExecutorService discardingLoader() {
		return new AbstractExecutorService() {
			@Override
			public void shutdown() {}

			@Override
			public List<Runnable> shutdownNow() {
				return List.of();
			}

			@Override
			public boolean isShutdown() {
				return false;
			}

			@Override
			public boolean isTerminated() {
				return false;
			}

			@Override
			public boolean awaitTermination(long timeout, TimeUnit unit) {
				return true;
			}

			@Override
			public void execute(Runnable command) {}
		};
	}
}
