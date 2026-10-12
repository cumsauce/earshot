package io.github.cumsauce.earshot.natives;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.lang.foreign.Arena;
import java.lang.foreign.SymbolLookup;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HashMap;
import java.util.HexFormat;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import org.tukaani.xz.XZInputStream;

/**
 * Extracts the bundled earshot-ffi library for this platform and opens it. Adapted from yakvc's loader.
 *
 * <p>Libraries live in the jar xz-compressed, as {@code natives/<os>-<arch>/<library>.xz} (or
 * {@code natives/<os>-universal/}), which makes the jar about 30 % smaller than its own compression would. Next to them
 * is a {@code natives.sha256} manifest of the uncompressed libraries. The library is unpacked to
 * {@code <configDir>/natives/<sha256>/}, checked against the manifest and opened with the global arena. It is never
 * unloaded, because it owns live threads.
 */
public final class NativeLoader {
	private static final String RESOURCE_ROOT = "/earshot/natives/";

	private NativeLoader() {}

	public static SymbolLookup load(Path configDir) throws IOException {
		Map<String, String> manifest = readManifest();
		String os = os();
		String file = libraryFileName(os);
		for (String platform : List.of(os + "-" + arch(), os + "-universal")) {
			String entry = platform + "/" + file;
			String hash = manifest.get(entry);
			if (hash != null) {
				Path path = extract(entry, hash, configDir.resolve("natives").resolve(hash).resolve(file));
				return SymbolLookup.libraryLookup(path, Arena.global());
			}
		}
		throw new UnsupportedPlatformException(os + "-" + arch());
	}

	static String os() {
		String name = System.getProperty("os.name").toLowerCase(Locale.ROOT);
		if (name.contains("linux")) return "linux";
		if (name.contains("win")) return "windows";
		if (name.contains("mac")) return "macos";
		throw new UnsupportedPlatformException(name);
	}

	static String arch() {
		String arch = System.getProperty("os.arch").toLowerCase(Locale.ROOT);
		return switch (arch) {
			case "amd64", "x86_64" -> "x86_64";
			case "aarch64", "arm64" -> "aarch64";
			default -> throw new UnsupportedPlatformException(arch);
		};
	}

	static String libraryFileName(String os) {
		return switch (os) {
			case "windows" -> "earshot_ffi.dll";
			case "macos" -> "libearshot_ffi.dylib";
			default -> "libearshot_ffi.so";
		};
	}

	private static Map<String, String> readManifest() throws IOException {
		Map<String, String> manifest = new HashMap<>();
		try (InputStream in = resource("natives.sha256");
				BufferedReader reader = new BufferedReader(new InputStreamReader(in, StandardCharsets.UTF_8))) {
			String line;
			while ((line = reader.readLine()) != null) {
				int sep = line.indexOf("  ");
				if (sep > 0) manifest.put(line.substring(sep + 2), line.substring(0, sep));
			}
		}
		return manifest;
	}

	private static Path extract(String entry, String expectedHash, Path target) throws IOException {
		if (Files.isRegularFile(target) && sha256(target).equals(expectedHash)) {
			return target;
		}
		Files.createDirectories(target.getParent());
		Path tmp = Files.createTempFile(target.getParent(), target.getFileName().toString(), ".tmp");
		try {
			try (InputStream in = new XZInputStream(resource(entry + ".xz"))) {
				Files.copy(in, tmp, StandardCopyOption.REPLACE_EXISTING);
			}
			String actual = sha256(tmp);
			if (!actual.equals(expectedHash)) {
				throw new IOException("native library " + entry + " has hash " + actual + ", expected " + expectedHash);
			}
			Files.move(tmp, target, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
		} finally {
			Files.deleteIfExists(tmp);
		}
		return target;
	}

	private static InputStream resource(String name) throws IOException {
		InputStream in = NativeLoader.class.getResourceAsStream(RESOURCE_ROOT + name);
		if (in == null) throw new IOException("missing resource " + RESOURCE_ROOT + name);
		return in;
	}

	private static String sha256(Path path) throws IOException {
		try {
			MessageDigest digest = MessageDigest.getInstance("SHA-256");
			return HexFormat.of().formatHex(digest.digest(Files.readAllBytes(path)));
		} catch (NoSuchAlgorithmException e) {
			throw new AssertionError(e);
		}
	}

	public static final class UnsupportedPlatformException extends RuntimeException {
		UnsupportedPlatformException(String platform) {
			super("no Earshot native library for " + platform);
		}
	}
}
