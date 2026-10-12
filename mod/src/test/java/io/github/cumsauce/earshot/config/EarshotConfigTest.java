package io.github.cumsauce.earshot.config;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;

import com.google.gson.JsonParser;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class EarshotConfigTest {
	@Test
	void missingAndBadValuesFallBackToTheDefaults() {
		EarshotConfig config = EarshotConfig.fromJson(JsonParser.parseString(
				"{\"preset\": \"ULTRA\", \"doppler\": false, \"reverb\": \"loud\", \"occlusion\": \"yes\"}")
				.getAsJsonObject());
		assertEquals(EarshotConfig.Preset.ULTRA, config.preset());
		assertFalse(config.doppler());
		assertEquals(EarshotConfig.DEFAULT.reverb(), config.reverb());
		assertEquals(EarshotConfig.DEFAULT.occlusion(), config.occlusion());
		assertEquals(EarshotConfig.DEFAULT.hrtf(), config.hrtf());
		assertFalse(config.ownVoice(), "own voice is off unless asked for");
	}

	@Test
	void savedSettingsLoadBackTheSame(@TempDir Path dir) throws Exception {
		Path file = dir.resolve("earshot").resolve("earshot.json");
		assertEquals(EarshotConfig.DEFAULT, EarshotConfig.load(file), "no file: the defaults");
		EarshotConfig config = new EarshotConfig(EarshotConfig.Preset.LOW, false, true, EarshotConfig.Reverb.EAX, false,
				true, false, false, "Built-In HRTF", true);
		config.save(file);
		assertEquals(config, EarshotConfig.load(file));
		Files.writeString(file, "not json {");
		assertEquals(EarshotConfig.DEFAULT, EarshotConfig.load(file), "unreadable: the defaults");
	}

	@Test
	void togglesTurnTheirPartOfTheTraceOff() {
		EarshotConfig high = new EarshotConfig(EarshotConfig.Preset.HIGH, true, true, EarshotConfig.Reverb.AUTO, true,
				true, true, true, "", false);
		assertArrayEquals(new double[] {4096, 16, 0.1, 9, 32, 0.3}, high.quality());
		EarshotConfig noPaths = new EarshotConfig(EarshotConfig.Preset.HIGH, true, false, EarshotConfig.Reverb.OFF,
				true, true, true, true, "", false);
		assertArrayEquals(new double[] {0, 16, 0.1, 9, 0, 0.3}, noPaths.quality());
		EarshotConfig off = new EarshotConfig(EarshotConfig.Preset.OFF, true, true, EarshotConfig.Reverb.AUTO, true,
				true, true, true, "", false);
		assertFalse(off.active());
		assertEquals(0, off.quality()[0]);
		assertEquals(0, off.quality()[3]);
	}
}
