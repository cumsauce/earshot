package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertNotSame;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.Test;

/**
 * The loader executor's lifetime: a normal release keeps it, only a context close swaps it. The OpenAL side of
 * release() needs a live context, so these cover the Java-side split rather than the AL deletions.
 */
class ConvolutionReverbTest {
	private static ConvolutionReverb reverb() {
		return new ConvolutionReverb((madeFrom, seed) -> new float[0], (madeFrom, now) -> true);
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
}
