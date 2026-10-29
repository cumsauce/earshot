package io.github.cumsauce.earshot.openal;

import io.github.cumsauce.earshot.EarshotClient;
import java.io.IOException;
import java.io.InputStream;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse.BodyHandlers;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Arrays;
import java.util.HexFormat;
import java.util.List;
import java.util.Map;
import java.util.function.BooleanSupplier;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;

/**
 * The HRTF data sets Earshot ships (from the SADIE II database, Apache 2.0; see {@code earshot/hrtf/NOTICE.txt}),
 * unpacked where the game's OpenAL Soft finds them: the top of the game's working directory, which it searches before
 * anything else and names each set after its file. It only looks when it opens a device, so this runs before the
 * sound engine starts, and only writes a file that is missing or different.
 *
 * <p>The 64-tap minimum-phase MHRs are bundled and extracted on every startup. The 256-tap SOFAs (the source data
 * OpenAL Soft's {@code makemhr} produced the MHRs from) are downloaded on first use, via
 * {@link #ensure(String, ProgressListener, BooleanSupplier)}, and verified by SHA-256.
 */
public final class HrtfFiles {
	/** The data sets, as the HRTF setting lists them. */
	public static final List<String> NAMES = List.of("SADIE II KU100", "SADIE II KEMAR");

	/**
	 * The suffix on the bundled 64-tap MHRs' file names, and on the names OpenAL lists them under. The bare names in
	 * {@link #NAMES} are reserved for the 256-tap SOFAs fetched on first use; without the suffix the MHR would
	 * shadow the SOFA in OpenAL Soft's search path.
	 */
	public static final String MHR_SUFFIX = " 64-tap";

	/**
	 * Where each SOFA comes from: the zip's URL, the exact member to take from it (the zip holds one SOFA per
	 * sample rate, named after its format, not after the set's display name), and the SHA-256 fingerprint — the
	 * in-jar / on-disk fingerprint of that member's contents.
	 */
	public record SofaSource(String url, String entry, String sha256) {}

	/**
	 * The two SADIE II 48 kHz 24-bit 256-tap SOFAs Earshot can fetch on first use, with their source URLs, the
	 * zip member each lives in, and SHA-256 fingerprints (Apache 2.0, from Zenodo record 12092466).
	 */
	public static final Map<String, SofaSource> SOFAS = Map.of(
			"SADIE II KU100", new SofaSource(
					"https://zenodo.org/records/12092466/files/D1_HRIR_SOFA.zip",
					"D1_HRIR_SOFA/D1_48K_24bit_256tap_FIR_SOFA.sofa",
					"9af7cb19531e52fb7ae8ec92621e6ab62b1d5fe584b3742be36699a0ddb0ccd4"),
			"SADIE II KEMAR", new SofaSource(
					"https://zenodo.org/records/12092466/files/D2_HRIR_SOFA.zip",
					"D2_HRIR_SOFA/D2_48K_24bit_256tap_FIR_SOFA.sofa",
					"bb4f3d4126e686f438c4dcd8c11541e62393c0d90db1da6f17d1dd13d01c8ef7"));

	/** Receives progress updates: {@code bytesRead} so far, {@code totalBytesOrZero} if known. */
	public interface ProgressListener {
		void update(long bytesRead, long totalBytesOrZero);
		default void done() {}
		default void failed() {}
	}

	private HrtfFiles() {}

	public static void install() {
		Path dir = Path.of("").toAbsolutePath();
		for (String name : NAMES) {
			String resource = "/earshot/hrtf/" + name + MHR_SUFFIX + ".mhr";
			try (InputStream in = HrtfFiles.class.getResourceAsStream(resource)) {
				if (in == null) {
					EarshotClient.LOGGER.warn("The jar has no {}", resource);
					continue;
				}
				byte[] data = in.readAllBytes();
				Path file = dir.resolve(name + MHR_SUFFIX + ".mhr");
				if (Files.isRegularFile(file) && Arrays.equals(Files.readAllBytes(file), data)) continue;
				Files.write(file, data);
				EarshotClient.LOGGER.info("Unpacked the {} HRTF (64-tap MHR) to {}", name, file);
			} catch (IOException e) {
				EarshotClient.LOGGER.warn("Couldn't unpack the {} HRTF into {}", name, dir, e);
			}
		}
	}

	/** Convenience: ensures the SOFA is at the working directory's expected path. */
	public static void ensure(String name, ProgressListener progress) throws IOException {
		ensure(Path.of("").toAbsolutePath(), name, progress, () -> false);
	}

	/** Package-private overload for tests: ensures the SOFA at the given path with no cancellation. */
	static void ensure(Path target, String name, ProgressListener progress) throws IOException {
		ensure(target, name, progress, () -> false);
	}

	/**
	 * Ensures the SOFA for {@code name} is on disk at {@code target} with a matching SHA-256. If the file is
	 * already on disk and verifies, returns immediately. Otherwise streams the Zenodo zip, extracts the
	 * 48 kHz 24-bit 256-tap SOFA member, verifies SHA-256, and writes to {@code target}. On any failure (network,
	 * missing member, mismatched SHA, cancelled), the partial file is deleted and {@link IOException} is thrown.
	 *
	 * @param target the path to write the SOFA to (used by tests; production passes the CWD)
	 * @param name the SOFA's display name (a key in {@link #SOFAS})
	 * @param progress called with bytes read so far
	 * @param cancelled checked periodically; returning {@code true} aborts with an {@code IOException}
	 */
	public static void ensure(Path target, String name, ProgressListener progress, BooleanSupplier cancelled)
			throws IOException {
		// Every failure, the early ones included, passes the one catch below: failed() once, whichever way.
		Path partial = partialOf(target);
		try {
			SofaSource source = SOFAS.get(name);
			if (source == null) throw new IOException("Unknown SOFA: " + name);
			if (cancelled.getAsBoolean()) throw new IOException("download cancelled");
			if (verified(target, name)) return; // already there and good
			Files.deleteIfExists(target);
			// Streamed into a sibling and moved into place only once it verifies: a download cut short (the
			// game closed, a crash) never leaves a file at target that looks like the SOFA.
			streamExtract(source, partial, progress, cancelled);
			if (cancelled.getAsBoolean()) throw new IOException("download cancelled");
			if (!sha256Of(partial).equalsIgnoreCase(source.sha256())) {
				throw new IOException("SOFA SHA-256 mismatch: " + name);
			}
			Files.move(partial, target, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
			progress.done();
		} catch (IOException | RuntimeException e) {
			try {
				Files.deleteIfExists(partial);
			} catch (IOException cleanup) {
				e.addSuppressed(cleanup);
			}
			progress.failed(); // once per failed download, whichever way it failed
			throw e;
		}
	}

	/** Whether the SOFA for {@code name} is at {@code target} and matches its SHA-256. */
	public static boolean verified(Path target, String name) {
		SofaSource source = SOFAS.get(name);
		if (source == null || !Files.isRegularFile(target)) return false;
		try {
			return sha256Of(target).equalsIgnoreCase(source.sha256());
		} catch (IOException e) {
			return false;
		}
	}

	/**
	 * Where a download of {@code target} streams to before it verifies. Unique per download, so a cancelled
	 * download still unwinding cannot truncate or delete the file of one started after it.
	 */
	private static Path partialOf(Path target) {
		return target.resolveSibling(target.getFileName() + "." + Long.toHexString(System.nanoTime()) + ".part");
	}

	private static void streamExtract(SofaSource source, Path target,
			ProgressListener progress, BooleanSupplier cancelled) throws IOException {
		HttpClient client = HttpClient.newBuilder().followRedirects(HttpClient.Redirect.NORMAL).build();
		HttpRequest request = HttpRequest.newBuilder(URI.create(source.url()))
				.header("User-Agent", "Earshot/SOFA-fetch").GET().build();
		try {
			streamExtractInner(client, request, source, target, progress, cancelled);
		} catch (InterruptedException e) {
			Thread.currentThread().interrupt();
			throw new IOException("interrupted", e);
		}
	}

	private static void streamExtractInner(HttpClient client, HttpRequest request, SofaSource source, Path target,
			ProgressListener progress, BooleanSupplier cancelled) throws IOException, InterruptedException {
		try (var in = client.send(request, BodyHandlers.ofInputStream()).body();
				var zipIn = new ZipInputStream(in)) {
			ZipEntry entry;
			long read = 0;
			byte[] buf = new byte[64 * 1024];
			while ((entry = zipIn.getNextEntry()) != null) {
				// Skipping an entry reads through it (several megabytes each), so a cancel is heeded here too.
				if (cancelled.getAsBoolean()) return;
				// The zip holds one SOFA per sample rate; only the pinned member matches.
				if (!entry.getName().equals(source.entry())) continue;
				// Opened only now, so a download with no matching member leaves no empty file behind.
				try (var out = Files.newOutputStream(target)) {
					int n;
					while ((n = zipIn.read(buf)) > 0) {
						if (cancelled.getAsBoolean()) {
							zipIn.closeEntry();
							return;
						}
						out.write(buf, 0, n);
						read += n;
						progress.update(read, 0);
					}
				}
				zipIn.closeEntry();
				return;
			}
			throw new IOException("no " + source.entry() + " in " + source.url());
		}
	}

	private static String sha256Of(Path file) throws IOException {
		try (InputStream in = Files.newInputStream(file)) {
			MessageDigest md = MessageDigest.getInstance("SHA-256");
			byte[] buf = new byte[64 * 1024];
			int n;
			while ((n = in.read(buf)) > 0) md.update(buf, 0, n);
			return HexFormat.of().formatHex(md.digest());
		} catch (NoSuchAlgorithmException e) {
			throw new IOException(e);
		}
	}
}
