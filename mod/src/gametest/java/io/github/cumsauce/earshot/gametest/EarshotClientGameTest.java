package io.github.cumsauce.earshot.gametest;

import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import net.fabricmc.fabric.api.client.gametest.v1.FabricClientGameTest;
import net.fabricmc.fabric.api.client.gametest.v1.context.ClientGameTestContext;

/**
 * Starts the real client and checks M0: the native library loaded, and the Library mixin probed the game's own
 * OpenAL. The report goes to {@code openal-probe.txt} in the run directory, which CI keeps.
 *
 * <p>CI has no sound card, so it runs this with {@code ALSOFT_DRIVERS=null}: OpenAL Soft's null backend still opens
 * a device and context with all of its features.
 */
@SuppressWarnings("UnstableApiUsage")
public class EarshotClientGameTest implements FabricClientGameTest {
	@Override
	public void runTest(ClientGameTestContext context) {
		if (EarshotClient.nativeAbi() != NativeBridge.ABI_VERSION) {
			throw new AssertionError("native library ABI " + EarshotClient.nativeAbi() + ", expected "
					+ NativeBridge.ABI_VERSION);
		}
		OpenAlProbe.Report report = OpenAlProbe.last();
		if (report == null) {
			throw new AssertionError("the OpenAL probe never ran: no sound device opened, or the Library mixin missed");
		}
		try {
			Files.writeString(Path.of("openal-probe.txt"), report.describe() + "\n");
		} catch (IOException e) {
			throw new UncheckedIOException(e);
		}
		if (!report.efx()) throw new AssertionError("the game's OpenAL has no EFX");
	}
}
