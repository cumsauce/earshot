package io.github.cumsauce.earshot.natives;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.Test;

/**
 * The length guard behind every native count this mod trusts: a value outside {@code 0…bound} fails fast naming
 * itself and the bound, rather than being truncated into a wrong-sized array or index.
 */
class NativeLengthGuardTest {
	/** Inside the bound, both ends included: the count is left alone. */
	@Test
	void aCountWithinTheGuardIsLeftAlone() {
		assertDoesNotThrow(() -> NativeBridge.checkNativeLength(0, 10, "test count"));
		assertDoesNotThrow(() -> NativeBridge.checkNativeLength(5, 10, "test count"));
		assertDoesNotThrow(() -> NativeBridge.checkNativeLength(10, 10, "test count"));
		// The widest bound the mod ever passes stays exact at its top.
		assertDoesNotThrow(
				() -> NativeBridge.checkNativeLength(Integer.MAX_VALUE, Integer.MAX_VALUE, "test count"));
	}

	/** A negative count means the native side reported nonsense; it is rejected, naming both ends. */
	@Test
	void aNegativeCountIsRejected() {
		IllegalStateException e = assertThrows(IllegalStateException.class,
				() -> NativeBridge.checkNativeLength(-1, 10, "test count"));
		assertTrue(e.getMessage().contains("-1"), e.getMessage());
		assertTrue(e.getMessage().contains("10"), e.getMessage());
	}

	/** One past the bound is as outside as a million past: both are rejected. */
	@Test
	void aCountPastTheGuardIsRejected() {
		IllegalStateException e = assertThrows(IllegalStateException.class,
				() -> NativeBridge.checkNativeLength(11, 10, "test count"));
		assertTrue(e.getMessage().contains("11"), e.getMessage());
		assertTrue(e.getMessage().contains("10"), e.getMessage());
	}

	/** A count too big for an int fails rather than truncating through a narrowing cast. */
	@Test
	void aCountPastWhatAnIntCanHoldIsRejected() {
		IllegalStateException e = assertThrows(IllegalStateException.class, () -> NativeBridge
				.checkNativeLength(Integer.MAX_VALUE + 1L, Integer.MAX_VALUE, "test count"));
		assertTrue(e.getMessage().contains(Long.toString(Integer.MAX_VALUE + 1L)), e.getMessage());
		assertTrue(e.getMessage().contains(Long.toString(Integer.MAX_VALUE)), e.getMessage());
	}
}
