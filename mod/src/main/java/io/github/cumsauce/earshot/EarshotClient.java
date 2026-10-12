package io.github.cumsauce.earshot;

import io.github.cumsauce.earshot.audio.SoundShaper;
import io.github.cumsauce.earshot.command.EarshotCommands;
import io.github.cumsauce.earshot.config.EarshotConfig;
import io.github.cumsauce.earshot.ui.BypassKey;
import io.github.cumsauce.earshot.ui.SoundOptionsButton;
import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.natives.NativeLoader;
import io.github.cumsauce.earshot.world.MaterialOverrides;
import io.github.cumsauce.earshot.world.WorldMirror;
import java.lang.foreign.MemorySegment;
import java.nio.file.Path;
import net.fabricmc.api.ClientModInitializer;
import net.fabricmc.loader.api.FabricLoader;
import org.jspecify.annotations.Nullable;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

public final class EarshotClient implements ClientModInitializer {
	public static final Logger LOGGER = LoggerFactory.getLogger("earshot");

	private static int nativeAbi = -1;
	private static @Nullable NativeBridge bridge;
	private static @Nullable MemorySegment engine;
	private static @Nullable WorldMirror mirror;
	private static @Nullable SoundShaper shaper;
	private static volatile EarshotConfig config = EarshotConfig.DEFAULT;
	private static volatile boolean bypassed;

	/** {@code config/earshot/}: settings and extracted natives. */
	public static Path configDir() {
		return FabricLoader.getInstance().getConfigDir().resolve("earshot");
	}

	/** {@code config/earshot/materials.json}: materials set by hand. */
	public static Path materialsFile() {
		return configDir().resolve("materials.json");
	}

	/** The settings in use. */
	public static EarshotConfig config() {
		return config;
	}

	/**
	 * Uses {@code config} from now on, and saves it. A different HRTF dataset takes a new OpenAL context, so the game's
	 * sound engine reloads for it.
	 */
	public static void apply(EarshotConfig next) {
		EarshotConfig previous = config;
		config = next;
		bypassed = false;
		next.save(configDir().resolve("earshot.json"));
		use(next);
		if (!next.hrtf().equals(previous.hrtf())) {
			net.minecraft.client.Minecraft.getInstance().getSoundManager().reload();
		}
	}

	/** Whether the bypass key has Earshot leaving sounds alone for now. */
	public static boolean bypassed() {
		return bypassed;
	}

	/**
	 * Bypasses Earshot for A/B listening, as the Off preset would, or ends the bypass. Not saved: the settings stay
	 * as they are, and changing them ends the bypass.
	 */
	public static void setBypassed(boolean bypass) {
		bypassed = bypass;
		use(bypass ? config.withPreset(EarshotConfig.Preset.OFF) : config);
	}

	/** Hands {@code settings} to the engine and the shaper. */
	private static void use(EarshotConfig settings) {
		NativeBridge bridge = EarshotClient.bridge;
		MemorySegment engine = EarshotClient.engine;
		SoundShaper shaper = EarshotClient.shaper;
		if (bridge == null || engine == null || shaper == null) return;
		bridge.setQuality(engine, settings.quality());
		shaper.configure(settings);
		shaper.voice().configure(settings.voice(), settings.ownVoice());
	}

	/** The loaded native library's ABI version, or -1 if it didn't load. */
	public static int nativeAbi() {
		return nativeAbi;
	}

	/** The native bridge, or null if the engine didn't start. */
	public static @Nullable NativeBridge bridge() {
		return bridge;
	}

	/** The native engine handle, or null if it didn't start. It lives as long as the game. */
	public static @Nullable MemorySegment engine() {
		return engine;
	}

	/** The world mirror, or null if the engine didn't start. */
	public static @Nullable WorldMirror mirror() {
		return mirror;
	}

	/** What shapes the playing sounds, or null if the engine didn't start. */
	public static @Nullable SoundShaper shaper() {
		return shaper;
	}

	@Override
	public void onInitializeClient() {
		config = EarshotConfig.load(configDir().resolve("earshot.json"));
		SoundOptionsButton.register();
		BypassKey.register();
		NativeBridge bridge;
		try {
			bridge = new NativeBridge(NativeLoader.load(configDir()));
			nativeAbi = bridge.abiVersion();
		} catch (NativeLoader.UnsupportedPlatformException e) {
			LOGGER.error("Earshot is off: this platform has no native engine", e);
			return;
		} catch (Exception | LinkageError e) {
			LOGGER.error("Earshot is off: could not load the native engine", e);
			return;
		}
		if (nativeAbi != NativeBridge.ABI_VERSION) {
			LOGGER.error("Earshot is off: native ABI version {} does not match expected {}", nativeAbi,
					NativeBridge.ABI_VERSION);
			return;
		}
		MemorySegment engine = bridge.create();
		LOGGER.info("Started the native engine (ABI version {})", nativeAbi);
		WorldMirror mirror = new WorldMirror(bridge, engine);
		mirror.materials().setOverrides(MaterialOverrides.load(materialsFile()));
		mirror.register();
		SoundShaper shaper = new SoundShaper(bridge, engine);
		EarshotCommands.register(bridge, engine, mirror, shaper);
		EarshotClient.bridge = bridge;
		EarshotClient.engine = engine;
		EarshotClient.mirror = mirror;
		EarshotClient.shaper = shaper;
		bridge.setQuality(engine, config.quality());
		shaper.configure(config);
		shaper.voice().configure(config.voice(), config.ownVoice());
		LOGGER.info("Preset {} ({})", config.preset().key(), config.preset().resolve().key());
	}
}
