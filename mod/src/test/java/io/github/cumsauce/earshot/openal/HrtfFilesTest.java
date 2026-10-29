package io.github.cumsauce.earshot.openal;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class HrtfFilesTest {
	/**
	 * The on-disk SOFA, when its SHA-256 matches the catalog, must be left alone (no
	 * download, no overwrite, no progress updates).
	 */
	@Test
	void ensureSkipsAlreadyPresentSofa(@TempDir Path dir) throws Exception {
		Path sofa = dir.resolve("SADIE II KU100.sofa");
		try (InputStream in = HrtfFiles.class.getResourceAsStream(
				"/earshot/hrtf/test-fixtures/SADIE II KU100.sofa")) {
			if (in == null) {
				fail("no /earshot/hrtf/test-fixtures/SADIE II KU100.sofa on the test classpath — "
						+ "run `git lfs pull` (the SOFAs are tracked via Git LFS)");
			}
			Files.copy(in, sofa, java.nio.file.StandardCopyOption.REPLACE_EXISTING);
		}
		long beforeSize = Files.size(sofa);
		long beforeModified = Files.getLastModifiedTime(sofa).toMillis();
		Thread.sleep(20);
		int[] updateCount = {0};
		HrtfFiles.ensure(sofa, "SADIE II KU100", (r, t) -> updateCount[0]++);
		assertEquals(beforeSize, Files.size(sofa), "SOFA file size unchanged");
		assertEquals(beforeModified, Files.getLastModifiedTime(sofa).toMillis(),
				"SOFA file mtime unchanged");
		assertEquals(0, updateCount[0], "no progress updates when SHA matches");
	}

	/** A name not in {@link HrtfFiles#SOFAS} throws {@link IOException} immediately. */
	@Test
	void ensureThrowsOnUnknownName(@TempDir Path dir) {
		Path target = dir.resolve("nonexistent.sofa");
		IOException thrown = assertThrows(IOException.class,
				() -> HrtfFiles.ensure(target, "Not a real HRTF", (r, t) -> {}));
		assertTrue(thrown.getMessage().contains("Unknown SOFA"));
	}

	/** A cancelled download throws and leaves no partial file. */
	@Test
	void ensureWithCancellationThrows(@TempDir Path dir) throws Exception {
		Path sofa = dir.resolve("SADIE II KU100.sofa");
		// No pre-existing file: cancellation happens before any download starts, so the
		// file must not appear on disk.
		IOException thrown = assertThrows(IOException.class, () -> HrtfFiles.ensure(sofa, "SADIE II KU100",
				(r, t) -> {}, () -> true));
		assertTrue(thrown.getMessage().contains("cancelled"),
				"cancellation should throw IOException: " + thrown.getMessage());
		assertFalse(Files.exists(sofa), "cancelled-before-download leaves no partial file");
	}
}
