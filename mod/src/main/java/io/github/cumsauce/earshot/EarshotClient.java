package io.github.cumsauce.earshot;

import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.natives.NativeLoader;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import java.nio.file.Path;
import net.fabricmc.api.ClientModInitializer;
import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientLifecycleEvents;
import net.fabricmc.loader.api.FabricLoader;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

public final class EarshotClient implements ClientModInitializer {
	public static final Logger LOGGER = LoggerFactory.getLogger("earshot");

	private static int nativeAbi = -1;

	/** {@code config/earshot/}: settings and extracted natives. */
	public static Path configDir() {
		return FabricLoader.getInstance().getConfigDir().resolve("earshot");
	}

	/** The loaded native library's ABI version, or -1 if it didn't load. */
	public static int nativeAbi() {
		return nativeAbi;
	}

	@Override
	public void onInitializeClient() {
		try {
			nativeAbi = new NativeBridge(NativeLoader.load(configDir())).abiVersion();
			if (nativeAbi == NativeBridge.ABI_VERSION) {
				LOGGER.info("Loaded the native engine (ABI version {})", nativeAbi);
			} else {
				LOGGER.error("Native ABI version {} does not match expected {}", nativeAbi, NativeBridge.ABI_VERSION);
			}
		} catch (NativeLoader.UnsupportedPlatformException e) {
			LOGGER.error("Earshot is off: this platform has no native engine", e);
		} catch (Exception | LinkageError e) {
			LOGGER.error("Earshot is off: could not load the native engine", e);
		}
		// The Library mixin probes OpenAL as soon as the sound engine opens its device. If it didn't (no device, or
		// the mixin missed), try once more when the client is up, so the log always says why.
		ClientLifecycleEvents.CLIENT_STARTED.register(minecraft -> {
			if (OpenAlProbe.last() == null) OpenAlProbe.run("client started");
		});
	}
}
