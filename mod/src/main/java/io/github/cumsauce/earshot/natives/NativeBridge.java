package io.github.cumsauce.earshot.natives;

import static java.lang.foreign.ValueLayout.ADDRESS;
import static java.lang.foreign.ValueLayout.JAVA_BYTE;
import static java.lang.foreign.ValueLayout.JAVA_DOUBLE;
import static java.lang.foreign.ValueLayout.JAVA_FLOAT;
import static java.lang.foreign.ValueLayout.JAVA_INT;
import static java.lang.foreign.ValueLayout.JAVA_LONG;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.nio.charset.StandardCharsets;

/**
 * Downcall handles for the C ABI in {@code crates/earshot-ffi/include/earshot.h}, plus thin typed wrappers that turn
 * error codes into {@link EarshotException}. Modelled on yakvc's bridge.
 *
 * <p>{@code engine} arguments are the handle from {@link #create}. The per-section and per-block calls are linked as
 * critical so they take Java arrays without copying. A failed call's message is kept per thread, so calls come from
 * platform threads only.
 */
public final class NativeBridge {
	/** Must equal {@code EARSHOT_ABI_VERSION} in {@code earshot.h}. */
	public static final int ABI_VERSION = 2;
	/** Blocks in a section, in Minecraft's order. */
	public static final int SECTION_BLOCKS = 4096;
	/** Doubles per traced leg: from x, y, z, to x, y, z, energy, escaped (1 or 0). */
	public static final int SEGMENT_DOUBLES = 8;

	// size_t is 64 bits on every platform Minecraft runs on.
	private static final ValueLayout.OfLong SIZE_T = JAVA_LONG;

	private final MethodHandle abiVersion;
	private final MethodHandle create;
	private final MethodHandle destroy;
	private final MethodHandle materialNames;
	private final MethodHandle setSection;
	private final MethodHandle removeSection;
	private final MethodHandle setBlock;
	private final MethodHandle clearWorld;
	private final MethodHandle sectionCount;
	private final MethodHandle transmission;
	private final MethodHandle tracePaths;
	private final MethodHandle lastError;

	public NativeBridge(SymbolLookup library) {
		Linker linker = Linker.nativeLinker();
		Linker.Option critical = Linker.Option.critical(true);
		abiVersion = linker.downcallHandle(find(library, "earshot_abi_version"), FunctionDescriptor.of(JAVA_INT));
		create = linker.downcallHandle(find(library, "earshot_create"), FunctionDescriptor.of(JAVA_INT, JAVA_INT, ADDRESS));
		destroy = linker.downcallHandle(find(library, "earshot_destroy"), FunctionDescriptor.ofVoid(ADDRESS));
		materialNames = linker.downcallHandle(find(library, "earshot_material_names"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, SIZE_T, ADDRESS));
		setSection = linker.downcallHandle(find(library, "earshot_set_section"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, JAVA_INT, JAVA_INT, JAVA_INT, ADDRESS), critical);
		removeSection = linker.downcallHandle(find(library, "earshot_remove_section"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, JAVA_INT, JAVA_INT, JAVA_INT));
		setBlock = linker.downcallHandle(find(library, "earshot_set_block"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, JAVA_INT, JAVA_INT, JAVA_INT, JAVA_BYTE));
		clearWorld = linker.downcallHandle(find(library, "earshot_clear_world"), FunctionDescriptor.of(JAVA_INT, ADDRESS));
		sectionCount = linker.downcallHandle(find(library, "earshot_section_count"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS), critical);
		transmission = linker.downcallHandle(find(library, "earshot_transmission"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, ADDRESS, ADDRESS), critical);
		tracePaths = linker.downcallHandle(find(library, "earshot_trace_paths"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, JAVA_INT, JAVA_INT, JAVA_DOUBLE, JAVA_LONG, ADDRESS,
						SIZE_T, ADDRESS, ADDRESS),
				critical);
		lastError = linker.downcallHandle(find(library, "earshot_last_error"),
				FunctionDescriptor.of(SIZE_T, ADDRESS, SIZE_T));
	}

	/** Returns the library's {@code EARSHOT_ABI_VERSION}. */
	public int abiVersion() {
		return (int) invoke(abiVersion);
	}

	/** Starts an engine with an empty world. */
	public MemorySegment create() {
		try (Arena arena = Arena.ofConfined()) {
			MemorySegment out = arena.allocate(ADDRESS);
			check((int) invoke(create, ABI_VERSION, out));
			return out.get(ADDRESS, 0);
		}
	}

	/** Frees the engine. Must be the last call on {@code engine}. */
	public void destroy(MemorySegment engine) {
		invoke(destroy, engine);
	}

	/** The native material names in id order, as JSON. */
	public String materialNames() {
		try (Arena arena = Arena.ofConfined()) {
			MemorySegment needed = arena.allocate(SIZE_T);
			long cap = 1024;
			while (true) {
				MemorySegment buf = arena.allocate(cap);
				check((int) invoke(materialNames, buf, cap, needed));
				long length = needed.get(SIZE_T, 0);
				if (length <= cap) {
					return new String(buf.asSlice(0, length).toArray(JAVA_BYTE), StandardCharsets.UTF_8);
				}
				cap = length;
			}
		}
	}

	/**
	 * Replaces the section at section coordinates {@code x, y, z}.
	 *
	 * @param blocks {@value #SECTION_BLOCKS} material ids, index {@code (y << 8) | (z << 4) | x}
	 */
	public void setSection(MemorySegment engine, int x, int y, int z, byte[] blocks) {
		if (blocks.length != SECTION_BLOCKS) {
			throw new IllegalArgumentException("a section has " + SECTION_BLOCKS + " blocks, not " + blocks.length);
		}
		check((int) invoke(setSection, engine, x, y, z, MemorySegment.ofArray(blocks)));
	}

	public void removeSection(MemorySegment engine, int x, int y, int z) {
		check((int) invoke(removeSection, engine, x, y, z));
	}

	public void setBlock(MemorySegment engine, int x, int y, int z, byte material) {
		check((int) invoke(setBlock, engine, x, y, z, material));
	}

	public void clearWorld(MemorySegment engine) {
		check((int) invoke(clearWorld, engine));
	}

	/** How many sections hold anything but air. */
	public long sectionCount(MemorySegment engine) {
		long[] out = new long[1];
		check((int) invoke(sectionCount, engine, MemorySegment.ofArray(out)));
		return out[0];
	}

	/** The share of sound energy, low, mid and high, that gets straight through the blocks from {@code from} to {@code to}. */
	public float[] transmission(MemorySegment engine, double[] from, double[] to) {
		requireLength(from, 3, "from");
		requireLength(to, 3, "to");
		float[] out = new float[3];
		check((int) invoke(transmission, engine, MemorySegment.ofArray(from), MemorySegment.ofArray(to),
				MemorySegment.ofArray(out)));
		return out;
	}

	/** What {@link #tracePaths} found: the legs it could fit, and stats over all of them. */
	public record Trace(double[] segments, int written, long legs, long escapedFirst, long escaped,
			double meanFreePath) {}

	/** Traces {@code rays} paths from {@code origin}; keeps at most {@code maxSegments} legs for drawing. */
	public Trace tracePaths(MemorySegment engine, double[] origin, int rays, int bounces, double reach, long seed,
			int maxSegments) {
		requireLength(origin, 3, "origin");
		double[] segments = new double[maxSegments * SEGMENT_DOUBLES];
		long[] written = new long[1];
		double[] stats = new double[4];
		check((int) invoke(tracePaths, engine, MemorySegment.ofArray(origin), rays, bounces, reach, seed,
				MemorySegment.ofArray(segments), (long) maxSegments, MemorySegment.ofArray(written),
				MemorySegment.ofArray(stats)));
		return new Trace(segments, (int) written[0], (long) stats[0], (long) stats[1], (long) stats[2], stats[3]);
	}

	/** Throws with this thread's {@code earshot_last_error} message unless {@code code} is 0. */
	private void check(int code) {
		if (code == 0) return;
		try (Arena arena = Arena.ofConfined()) {
			long cap = 1024;
			MemorySegment buf = arena.allocate(cap);
			long length = Math.min((long) invoke(lastError, buf, cap), cap);
			throw new EarshotException(code,
					new String(buf.asSlice(0, length).toArray(JAVA_BYTE), StandardCharsets.UTF_8));
		}
	}

	private static void requireLength(double[] array, int length, String what) {
		if (array.length != length) throw new IllegalArgumentException(what + " has " + array.length + " values");
	}

	/** Calls {@code handle}. A downcall never throws, so anything thrown is a bug in this class. */
	private static Object invoke(MethodHandle handle, Object... args) {
		try {
			return handle.invokeWithArguments(args);
		} catch (Throwable t) {
			throw new IllegalStateException("bad downcall " + handle.type(), t);
		}
	}

	private static MemorySegment find(SymbolLookup library, String name) {
		return library.find(name).orElseThrow(() -> new UnsatisfiedLinkError("missing native symbol " + name));
	}
}
