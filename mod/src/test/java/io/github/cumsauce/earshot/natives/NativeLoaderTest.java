package io.github.cumsauce.earshot.natives;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class NativeLoaderTest {
	@Test
	void libraryNamesMatchWhatXtaskStages() {
		assertEquals("libearshot_ffi.so", NativeLoader.libraryFileName("linux"));
		assertEquals("libearshot_ffi.dylib", NativeLoader.libraryFileName("macos"));
		assertEquals("earshot_ffi.dll", NativeLoader.libraryFileName("windows"));
	}
}
