package io.github.cumsauce.earshot.natives;

import static java.lang.foreign.ValueLayout.JAVA_INT;

import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.invoke.MethodHandle;

/**
 * Downcall handles for the C ABI in {@code crates/earshot-ffi/include/earshot.h}. For now (M0) only the ABI version,
 * which proves the library ships in the jar and loads.
 */
public final class NativeBridge {
	/** Must equal {@code EARSHOT_ABI_VERSION} in {@code earshot.h}. */
	public static final int ABI_VERSION = 1;

	private final MethodHandle abiVersion;

	public NativeBridge(SymbolLookup library) {
		Linker linker = Linker.nativeLinker();
		abiVersion = linker.downcallHandle(find(library, "earshot_abi_version"), FunctionDescriptor.of(JAVA_INT));
	}

	/** Returns the library's {@code EARSHOT_ABI_VERSION}. */
	public int abiVersion() {
		try {
			return (int) abiVersion.invokeExact();
		} catch (Throwable t) {
			throw new IllegalStateException("bad downcall " + abiVersion.type(), t);
		}
	}

	private static MemorySegment find(SymbolLookup library, String name) {
		return library.find(name).orElseThrow(() -> new UnsatisfiedLinkError("missing native symbol " + name));
	}
}
