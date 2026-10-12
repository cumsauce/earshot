package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.Test;

class AirTest {
	@Test
	void matchesIso9613Tables() {
		// ISO 9613-1 at 20 °C and 50 % humidity: about 4.7 dB/km at 1 kHz and 28 dB/km at 4 kHz.
		assertEquals(0.0047, Air.absorptionDbPerMetre(1000, 20, 50), 0.0007);
		assertEquals(0.028, Air.absorptionDbPerMetre(4000, 20, 50), 0.003);
	}

	@Test
	void dryAirDullsMoreThanHumidAndRainLeastOfAll() {
		float desert = Air.factorForBiome(2.0f, false, false);
		float plains = Air.factorForBiome(0.8f, true, false);
		float rain = Air.factorForBiome(0.8f, true, true);
		assertTrue(desert > plains && plains > rain, desert + " " + plains + " " + rain);
		assertTrue(plains > 0.3f && plains < 2.5f, "near OpenAL's own 1 for mild air: " + plains);
	}
}
