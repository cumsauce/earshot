package io.github.cumsauce.earshot;

import io.github.cumsauce.earshot.command.EarshotCommands;
import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.natives.NativeLoader;
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

	/** {@code config/earshot/}: settings and extracted natives. */
	public static Path configDir() {
		return FabricLoader.getInstance().getConfigDir().resolve("earshot");
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

	@Override
	public void onInitializeClient() {
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
		mirror.register();
		EarshotCommands.register(bridge, engine, mirror);
		EarshotClient.bridge = bridge;
		EarshotClient.engine = engine;
		EarshotClient.mirror = mirror;
	}
}
