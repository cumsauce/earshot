package io.github.cumsauce.earshot.config;

import com.google.gson.GsonBuilder;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParseException;
import com.google.gson.JsonParser;
import com.google.gson.JsonPrimitive;
import io.github.cumsauce.earshot.EarshotClient;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.NoSuchFileException;
import java.nio.file.Path;
import java.util.Locale;

/**
 * Earshot's settings, kept in {@code config/earshot/earshot.json}. A missing or unreadable value falls back to its
 * default, so an old or hand-edited file still loads.
 *
 * @param preset        how much to trace (docs/PLAN.md, "Performance budget")
 * @param occlusion     whether what is in the way muffles sounds
 * @param pathing       whether sounds come round corners
 * @param reverb        which reverb, or {@link Reverb#AUTO} for the best the game's OpenAL has
 * @param airAbsorption whether far sounds lose their highs
 * @param underwater    whether everything is muffled underwater
 * @param doppler       whether moving sounds shift in pitch
 * @param voice         whether Yak VC's voice plays through Earshot
 * @param hrtf          the HRTF dataset for Directional Audio, or empty for OpenAL's default
 * @param ownVoice      whether the player's own voice in Yak VC echoes off the room (headphones only)
 * @param realisticFalloff whether the game's sounds fall off as 1/distance rather than linearly
 */
public record EarshotConfig(Preset preset, boolean occlusion, boolean pathing, Reverb reverb, boolean airAbsorption,
		boolean underwater, boolean doppler, boolean voice, String hrtf, boolean ownVoice, boolean realisticFalloff) {
	public static final EarshotConfig DEFAULT = new EarshotConfig(Preset.AUTO, true, true, Reverb.AUTO, true, true, true,
			true, "", false, true);

	/** How much the engine traces. */
	public enum Preset {
		/** Picked from the machine's cores; the budget does the rest. */
		AUTO(null),
		/** Earshot leaves the game's sounds alone. */
		OFF(new double[] {0, 0, 0.25, 0, 0, 0.1}),
		LOW(new double[] {256, 4, 0.25, 1, 16, 0.1}),
		MEDIUM(new double[] {1024, 8, 0.125, 5, 24, 0.2}),
		HIGH(new double[] {4096, 16, 0.1, 9, 32, 0.3}),
		/** Its room trace may take 1.2 cores, spread over up to 4 threads. */
		ULTRA(new double[] {16384, 32, 0.1, 9, 48, 1.2});

		/** {@code earshot_set_quality}'s doubles: room rays, bounces, interval, occlusion rays, field reach, budget. */
		private final double[] quality;

		Preset(double[] quality) {
			this.quality = quality;
		}

		/** What this preset is on this machine: {@link #AUTO} picks by core count. */
		public Preset resolve() {
			if (this != AUTO) return this;
			int cores = Runtime.getRuntime().availableProcessors();
			return cores >= 8 ? HIGH : cores >= 4 ? MEDIUM : LOW;
		}

		public String key() {
			return name().toLowerCase(Locale.ROOT);
		}
	}

	/** Which reverb. */
	public enum Reverb {
		AUTO,
		CONVOLUTION,
		EAX,
		OFF;

		public String key() {
			return name().toLowerCase(Locale.ROOT);
		}
	}

	/** Whether Earshot shapes the game's sounds at all. */
	public boolean active() {
		return preset.resolve() != Preset.OFF;
	}

	/**
	 * {@code earshot_set_quality}'s doubles for these settings, with room rays reaching {@code reach} blocks.
	 */
	public double[] quality(double reach) {
		double[] quality = java.util.Arrays.copyOf(preset.resolve().quality, 7);
		quality[6] = reach;
		if (!occlusion) {
			quality[3] = 0;
			quality[4] = 0;
		}
		if (!pathing) quality[4] = 0;
		if (reverb == Reverb.OFF) quality[0] = 0;
		return quality;
	}

	public EarshotConfig withPreset(Preset preset) {
		return new EarshotConfig(preset, occlusion, pathing, reverb, airAbsorption, underwater, doppler, voice, hrtf,
				ownVoice, realisticFalloff);
	}

	public EarshotConfig withReverb(Reverb reverb) {
		return new EarshotConfig(preset, occlusion, pathing, reverb, airAbsorption, underwater, doppler, voice, hrtf,
				ownVoice, realisticFalloff);
	}

	public EarshotConfig withVoice(boolean voice) {
		return new EarshotConfig(preset, occlusion, pathing, reverb, airAbsorption, underwater, doppler, voice, hrtf,
				ownVoice, realisticFalloff);
	}

	public EarshotConfig withRealisticFalloff(boolean realisticFalloff) {
		return new EarshotConfig(preset, occlusion, pathing, reverb, airAbsorption, underwater, doppler, voice, hrtf,
				ownVoice, realisticFalloff);
	}

	public EarshotConfig withHrtf(String hrtf) {
		return new EarshotConfig(preset, occlusion, pathing, reverb, airAbsorption, underwater, doppler, voice, hrtf,
				ownVoice, realisticFalloff);
	}

	public EarshotConfig withOwnVoice(boolean ownVoice) {
		return new EarshotConfig(preset, occlusion, pathing, reverb, airAbsorption, underwater, doppler, voice, hrtf,
				ownVoice, realisticFalloff);
	}

	/** The settings in {@code file}, or the defaults if there is none or it can't be read. */
	public static EarshotConfig load(Path file) {
		try {
			JsonElement json = JsonParser.parseString(Files.readString(file));
			return json.isJsonObject() ? fromJson(json.getAsJsonObject()) : DEFAULT;
		} catch (NoSuchFileException e) {
			return DEFAULT;
		} catch (IOException | JsonParseException e) {
			EarshotClient.LOGGER.warn("Couldn't read {}, so Earshot uses its default settings", file, e);
			return DEFAULT;
		}
	}

	public void save(Path file) {
		try {
			Files.createDirectories(file.getParent());
			Files.writeString(file, new GsonBuilder().setPrettyPrinting().create().toJson(toJson()) + "\n");
		} catch (IOException e) {
			EarshotClient.LOGGER.warn("Couldn't save Earshot's settings to {}", file, e);
		}
	}

	static EarshotConfig fromJson(JsonObject json) {
		EarshotConfig d = DEFAULT;
		return new EarshotConfig(
				named(json, "preset", Preset.class, d.preset),
				flag(json, "occlusion", d.occlusion),
				flag(json, "pathing", d.pathing),
				named(json, "reverb", Reverb.class, d.reverb),
				flag(json, "air_absorption", d.airAbsorption),
				flag(json, "underwater", d.underwater),
				flag(json, "doppler", d.doppler),
				flag(json, "voice", d.voice),
				text(json, "hrtf", d.hrtf),
				flag(json, "own_voice", d.ownVoice),
				// A new key: settings saved before falloff carried far get it on.
				flag(json, "realistic_distance", d.realisticFalloff));
	}

	JsonObject toJson() {
		JsonObject json = new JsonObject();
		json.addProperty("preset", preset.key());
		json.addProperty("occlusion", occlusion);
		json.addProperty("pathing", pathing);
		json.addProperty("reverb", reverb.key());
		json.addProperty("air_absorption", airAbsorption);
		json.addProperty("underwater", underwater);
		json.addProperty("doppler", doppler);
		json.addProperty("voice", voice);
		json.addProperty("hrtf", hrtf);
		json.addProperty("own_voice", ownVoice);
		json.addProperty("realistic_distance", realisticFalloff);
		return json;
	}

	private static boolean flag(JsonObject json, String key, boolean fallback) {
		return json.get(key) instanceof JsonPrimitive p && p.isBoolean() ? p.getAsBoolean() : fallback;
	}

	private static String text(JsonObject json, String key, String fallback) {
		return json.get(key) instanceof JsonPrimitive p && p.isString() ? p.getAsString() : fallback;
	}

	private static <E extends Enum<E>> E named(JsonObject json, String key, Class<E> type, E fallback) {
		String name = text(json, key, "");
		for (E value : type.getEnumConstants()) {
			if (value.name().equalsIgnoreCase(name)) return value;
		}
		return fallback;
	}
}
