package io.github.cumsauce.earshot.openal;

import io.github.cumsauce.earshot.EarshotClient;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;

/**
 * The HRTF data sets Earshot ships (from the SADIE II database, Apache 2.0; see {@code earshot/hrtf/NOTICE.txt}),
 * unpacked where the game's OpenAL Soft finds them: the top of the game's working directory, which it searches before
 * anything else and names each set after its file. It only looks when it opens a device, so this runs before the
 * sound engine starts, and only writes a file that is missing or different.
 */
public final class HrtfFiles {
	/** The data sets, as the HRTF setting lists them. */
	public static final List<String> NAMES = List.of("SADIE II KU100", "SADIE II KEMAR");

	private HrtfFiles() {}

	public static void install() {
		Path dir = Path.of("").toAbsolutePath();
		for (String name : NAMES) {
			String resource = "/earshot/hrtf/" + name + ".mhr";
			try (InputStream in = HrtfFiles.class.getResourceAsStream(resource)) {
				if (in == null) {
					EarshotClient.LOGGER.warn("The jar has no {}", resource);
					continue;
				}
				byte[] data = in.readAllBytes();
				Path file = dir.resolve(name + ".mhr");
				if (Files.isRegularFile(file) && Arrays.equals(Files.readAllBytes(file), data)) continue;
				Files.write(file, data);
				EarshotClient.LOGGER.info("Unpacked the {} HRTF to {}", name, file);
			} catch (IOException e) {
				EarshotClient.LOGGER.warn("Couldn't unpack the {} HRTF into {}", name, dir, e);
			}
		}
	}
}
