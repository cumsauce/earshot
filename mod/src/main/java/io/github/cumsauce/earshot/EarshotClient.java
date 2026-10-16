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

	/** Whether Earshot is shaping sounds now: not Off, and not bypassed. */
	public static boolean shaping() {
		return !bypassed && config.active() && shaper != null;
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

	/**
	 * How far room rays reach: twice as far under a dimension's ceiling, whose caverns are far bigger, and half as far
	 * again on Ultra, whose budget absorbs the cost and whose players build halls over a hundred blocks across.
	 */
	private static double roomReach() {
		var level = net.minecraft.client.Minecraft.getInstance().level;
		boolean ceiling = level != null && level.dimensionType().hasCeiling();
		double base = config.preset().resolve() == EarshotConfig.Preset.ULTRA ? 96 : 64;
		return ceiling ? base * 2 : base;
	}

	/** Under a ceiling or not, as {@link #roomReach} last saw. */
	private static boolean ceiling;

	/** Takes up the room reach of a dimension just entered. Called on the client thread each tick. */
	private static void followDimension() {
		var level = net.minecraft.client.Minecraft.getInstance().level;
		boolean now = level != null && level.dimensionType().hasCeiling();
		if (now == ceiling) return;
		ceiling = now;
		use(bypassed ? config.withPreset(EarshotConfig.Preset.OFF) : config);
	}

	/** Hands {@code settings} to the engine and the shaper. */
	private static void use(EarshotConfig settings) {
		NativeBridge bridge = EarshotClient.bridge;
		MemorySegment engine = EarshotClient.engine;
		SoundShaper shaper = EarshotClient.shaper;
		if (bridge == null || engine == null || shaper == null) return;
		bridge.setQuality(engine, settings.quality(roomReach()));
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
		io.github.cumsauce.earshot.openal.HrtfFiles.install();
		SoundOptionsButton.register();
		BypassKey.register();
		net.fabricmc.fabric.api.client.rendering.v1.hud.HudElementRegistry.addLast(
				net.minecraft.resources.Identifier.fromNamespaceAndPath("earshot", "debug"),
				new io.github.cumsauce.earshot.ui.DebugOverlay());
		NativeBridge bridge = null;
		MemorySegment engine = null;
		SoundShaper shaper = null;
		try {
			bridge = new NativeBridge(NativeLoader.load(configDir()));
			nativeAbi = bridge.abiVersion();
			if (nativeAbi != NativeBridge.ABI_VERSION) {
				LOGGER.error("Earshot is off: native ABI version {} does not match expected {}", nativeAbi,
						NativeBridge.ABI_VERSION);
				return;
			}
			engine = bridge.create();
			LOGGER.info("Started the native engine (ABI version {})", nativeAbi);
			WorldMirror mirror = new WorldMirror(bridge, engine);
			mirror.materials().setOverrides(MaterialOverrides.load(materialsFile()));
			mirror.register();
			shaper = new SoundShaper(bridge, engine);
			EarshotCommands.register(bridge, engine, mirror, shaper);
			bridge.setQuality(engine, config.quality(64));
			shaper.configure(config);
			shaper.voice().configure(config.voice(), config.ownVoice());
			EarshotClient.bridge = bridge;
			EarshotClient.engine = engine;
			EarshotClient.mirror = mirror;
			EarshotClient.shaper = shaper;
		} catch (NativeLoader.UnsupportedPlatformException e) {
			LOGGER.error("Earshot is off: this platform has no native engine", e);
			stop(bridge, engine, shaper);
			return;
		} catch (Exception | LinkageError e) {
			LOGGER.error("Earshot is off: could not load the native engine", e);
			stop(bridge, engine, shaper);
			return;
		}
		net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientTickEvents.END_CLIENT_TICK
				.register(minecraft -> followDimension());
		net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientLifecycleEvents.CLIENT_STOPPING
				.register(minecraft -> stop());
		LOGGER.info("Preset {} ({})", config.preset().key(), config.preset().resolve().key());
	}

	/** Frees what the engine started with, as {@link #stop(NativeBridge, MemorySegment, SoundShaper)}. */
	private static void stop() {
		stop(bridge, engine, shaper);
	}

	/**
	 * Forgets and frees the engine: the shaper lets go of its sources and voices, then the engine itself goes, which
	 * must be its last call. Safe to run over whatever of the three got built.
	 */
	private static void stop(@Nullable NativeBridge bridge, @Nullable MemorySegment engine,
			@Nullable SoundShaper shaper) {
		EarshotClient.bridge = null;
		EarshotClient.engine = null;
		EarshotClient.mirror = null;
		EarshotClient.shaper = null;
		if (shaper != null) {
			try {
				shaper.contextClosing();
			} catch (Exception | LinkageError e) {
				LOGGER.error("Earshot: could not close the sound context", e);
			}
		}
		if (bridge != null && engine != null) {
			try {
				bridge.destroy(engine);
			} catch (Exception | LinkageError e) {
				LOGGER.error("Earshot: could not free the native engine", e);
			}
		}
	}
}
