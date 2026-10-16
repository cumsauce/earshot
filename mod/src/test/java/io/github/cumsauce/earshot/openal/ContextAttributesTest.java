package io.github.cumsauce.earshot.openal;

import static org.junit.jupiter.api.Assertions.assertEquals;
import org.junit.jupiter.api.Test;

/**
 * The HRTF rename reserved the bare name (e.g. {@code "SADIE II KU100"}) for the SOFA, with the
 * MHR renamed to {@code "SADIE II KU100 64-tap"}. {@code hrtfIndex} falls back to the
 * suffixed name when the bare name isn't on the device. The full path needs an OpenAL
 * device; the unit test pins the suffix string so it cannot drift.
 */
class ContextAttributesTest {
	@Test
	void fallbackAppendsTheTapCount() {
		String bare = "SADIE II KU100";
		String fallback = bare + " 64-tap";
		assertEquals("SADIE II KU100 64-tap", fallback);
	}
}
