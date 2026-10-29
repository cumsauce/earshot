package io.github.cumsauce.earshot.openal;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import org.junit.jupiter.api.Test;

/**
 * The HRTF rename reserved the bare name (e.g. {@code "SADIE II KU100"}) for the SOFA, with the
 * MHR renamed to {@code "SADIE II KU100 64-tap"}. {@code hrtfIndex} falls back to
 * {@link HrtfFiles#MHR_SUFFIX} appended to the bare name when the bare name isn't on the device.
 * That lookup needs a live OpenAL device, so the unit test pins what it can: the suffix constant
 * the fallback appends, and the bundled MHRs actually being named with it.
 */
class ContextAttributesTest {
	@Test
	void theFallbackSuffixIsPinned() {
		// If this drifts, the fallback no longer matches the suffixed names OpenAL lists the
		// bundled MHRs under (or the file names HrtfFiles.install unpacks them as).
		assertEquals(" 64-tap", HrtfFiles.MHR_SUFFIX);
	}

	@Test
	void theShippedMhrsAreNamedWithTheSuffix() {
		for (String name : HrtfFiles.NAMES) {
			assertNotNull(HrtfFiles.class.getResourceAsStream("/earshot/hrtf/" + name + HrtfFiles.MHR_SUFFIX + ".mhr"),
					name + HrtfFiles.MHR_SUFFIX + ".mhr isn't bundled");
		}
	}
}
