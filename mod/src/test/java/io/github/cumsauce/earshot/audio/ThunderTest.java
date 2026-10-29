package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class ThunderTest {
	@Test
	void thunderComesAboutThreeSecondsAKilometreAfterTheFlash() {
		assertEquals(58, Thunder.delayTicks(1000));
		assertEquals(0, Thunder.delayTicks(5), "a bolt next door is heard at once");
		// Ten kilometres: cold air carries it slower, so it is heard later than in a desert.
		int cold = Thunder.delayTicks(10_000, Air.speedOfSound(-30));
		int hot = Thunder.delayTicks(10_000, Air.speedOfSound(50));
		assertEquals(639, cold);
		assertEquals(553, hot);
	}
}
