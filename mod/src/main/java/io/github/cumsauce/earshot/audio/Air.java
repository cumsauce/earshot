package io.github.cumsauce.earshot.audio;

/**
 * How much air takes from the highs over distance, from its temperature and humidity: ISO 9613-1's atmospheric
 * absorption, at sea-level pressure. Dry air soaks up treble faster than humid air, so a desert dulls far sounds more
 * than a jungle or a rainy day does.
 */
final class Air {
	/** OpenAL's air absorption factor 1 takes this many dB a metre from the highs, at its 5 kHz reference. */
	private static final double OPENAL_DB_PER_METRE = 0.05;
	private static final double HF_REFERENCE_HZ = 5000;

	private Air() {}

	/** ISO 9613-1 absorption at {@code hz}, in dB a metre, in air at {@code celsius} and {@code humidity} percent. */
	static double absorptionDbPerMetre(double hz, double celsius, double humidity) {
		double t = celsius + 273.15;
		double tr = t / 293.15;
		double saturation = Math.pow(10, -6.8346 * Math.pow(273.16 / t, 1.261) + 4.6151);
		double h = humidity * saturation;
		double frO = 24 + 4.04e4 * h * (0.02 + h) / (0.391 + h);
		double frN = Math.pow(tr, -0.5) * (9 + 280 * h * Math.exp(-4.170 * (Math.pow(tr, -1.0 / 3) - 1)));
		double f2 = hz * hz;
		return 8.686 * f2 * (1.84e-11 * Math.sqrt(tr) + Math.pow(tr, -2.5)
				* (0.01275 * Math.exp(-2239.1 / t) / (frO + f2 / frO) + 0.1068 * Math.exp(-3352.0 / t) / (frN + f2 / frN)));
	}

	/**
	 * Speed of sound in air at {@code celsius}, in blocks (metres) a second: the textbook {@code 331.3 + 0.606·T},
	 * clamped as the native engine's {@code speed_of_sound} clamps it, so the delays here and the echoes there run
	 * on the same air.
	 */
	static double speedOfSound(double celsius) {
		return Math.clamp(331.3 + 0.606 * Math.clamp(celsius, -60, 60), 300, 370);
	}

	/** OpenAL's air absorption factor for air at {@code celsius} and {@code humidity} percent. */
	static float factor(double celsius, double humidity) {
		return (float) Math.clamp(absorptionDbPerMetre(HF_REFERENCE_HZ, celsius, humidity) / OPENAL_DB_PER_METRE, 0, 10);
	}

	/**
	 * The air in a biome: about 20 °C per unit of the game's temperature (plains 16 °C, desert 40 °C, snowy plains
	 * below freezing). Humidity: near saturation in rain; dry (15 %) where it never rains (desert, savanna, badlands,
	 * the Nether); humid (80 %) where it is hot and rains (jungle); else 55 %.
	 */
	static float factorForBiome(float temperature, boolean rains, boolean raining) {
		double celsius = Math.clamp(20.0 * temperature, -30, 50);
		return factor(celsius, humidityFor(celsius, rains, raining));
	}

	/** The humidity the absorption uses at {@code celsius}: near saturation in rain, and the rest as above. */
	static double humidityFor(double celsius, boolean rains, boolean raining) {
		return raining ? 95 : !rains ? 15 : celsius >= 19 ? 80 : 55;
	}
}
