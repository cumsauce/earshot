package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class ThunderTest {
	@Test
	void thunderComesAboutThreeSecondsAKilometreAfterTheFlash() {
		assertEquals(58, Thunder.delayTicks(1000));
		assertEquals(0, Thunder.delayTicks(5), "a bolt next door is heard at once");
	}
}
