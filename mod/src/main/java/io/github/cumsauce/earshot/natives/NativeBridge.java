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
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.UUID;

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
	public static final int ABI_VERSION = 9;
	/** Blocks in a section, in Minecraft's order. */
	public static final int SECTION_BLOCKS = 4096;
	/** Doubles per traced leg: from x, y, z, to x, y, z, energy, escaped (1 or 0). */
	public static final int SEGMENT_DOUBLES = 8;
	/** Doubles per source from {@link #hear}: low, mid and high gains, where it seems to come from, how far it travels. */
	public static final int HEARD_DOUBLES = 7;
	/**
	 * Doubles from {@link #room}: decay low, mid and high (seconds), mean free path, enclosure, first reflection
	 * distance, scattering, then the 14 EAX reverb parameters from {@link #EAX_FIRST} on.
	 */
	public static final int ROOM_DOUBLES = 21;
	/** Where the EAX reverb parameters start in {@link #room}'s doubles. */
	public static final int EAX_FIRST = 7;
	/**
	 * Doubles for {@link #setQuality}: the room measurement's rays (0 for none), bounces and interval in seconds,
	 * occlusion rays per source (0, 1, 5 or 9), the pathing field's reach in blocks (0 for none), and the share of one
	 * core the room measurement may take.
	 */
	public static final int QUALITY_DOUBLES = 6;
	/**
	 * Doubles from {@link #stats}: rays in the latest room measurement, its share of one core, the latest pathing
	 * field's build time in milliseconds, and its reach (0 for none).
	 */
	public static final int STATS_DOUBLES = 4;
	/** Doubles per stream from {@code earshot_voice_streams}. */
	private static final int VOICE_DOUBLES = 10;

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
	private final MethodHandle setListener;
	private final MethodHandle hear;
	private final MethodHandle room;
	private final MethodHandle roomAt;
	private final MethodHandle impulseResponse;
	private final MethodHandle setQuality;
	private final MethodHandle stats;
	private final MethodHandle voiceSink;
	private final MethodHandle voicePolls;
	private final MethodHandle voiceOwn;
	private final MethodHandle voiceCallback;
	private final MethodHandle voiceStream;
	private final MethodHandle voiceStreams;
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
		setListener = linker.downcallHandle(find(library, "earshot_set_listener"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS), critical);
		hear = linker.downcallHandle(find(library, "earshot_hear"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, ADDRESS, SIZE_T, ADDRESS), critical);
		room = linker.downcallHandle(find(library, "earshot_room"), FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS),
				critical);
		roomAt = linker.downcallHandle(find(library, "earshot_room_at"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, JAVA_INT, JAVA_INT, JAVA_LONG, ADDRESS), critical);
		impulseResponse = linker.downcallHandle(find(library, "earshot_impulse_response"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, JAVA_INT, JAVA_FLOAT, JAVA_LONG, ADDRESS, SIZE_T, ADDRESS),
				critical);
		setQuality = linker.downcallHandle(find(library, "earshot_set_quality"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS), critical);
		stats = linker.downcallHandle(find(library, "earshot_stats"), FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS),
				critical);
		voiceSink = linker.downcallHandle(find(library, "earshot_voice_sink"), FunctionDescriptor.of(ADDRESS, JAVA_INT));
		voicePolls = linker.downcallHandle(find(library, "earshot_voice_polls"), FunctionDescriptor.of(JAVA_LONG));
		voiceOwn = linker.downcallHandle(find(library, "earshot_voice_own"),
				FunctionDescriptor.ofVoid(ValueLayout.JAVA_BOOLEAN));
		voiceCallback = linker.downcallHandle(find(library, "earshot_voice_callback"), FunctionDescriptor.of(ADDRESS));
		voiceStream = linker.downcallHandle(find(library, "earshot_voice_stream"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS), critical);
		voiceStreams = linker.downcallHandle(find(library, "earshot_voice_streams"),
				FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, SIZE_T, ADDRESS), critical);
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

	/** Where the listener is now; the pathing field around it follows on the engine's own thread. */
	public void setListener(MemorySegment engine, double[] listener) {
		requireLength(listener, 3, "listener");
		check((int) invoke(setListener, engine, MemorySegment.ofArray(listener)));
	}

	/**
	 * How each of {@code count} sources reaches {@code listener}.
	 *
	 * @param sources x, y, z per source
	 * @param out     {@value #HEARD_DOUBLES} doubles per source
	 */
	public void hear(MemorySegment engine, double[] listener, double[] sources, int count, double[] out) {
		requireLength(listener, 3, "listener");
		if (sources.length < 3 * count || out.length < HEARD_DOUBLES * count) {
			throw new IllegalArgumentException("hear: " + count + " sources, but " + sources.length + " and "
					+ out.length + " doubles");
		}
		check((int) invoke(hear, engine, MemorySegment.ofArray(listener), MemorySegment.ofArray(sources),
				(long) count, MemorySegment.ofArray(out)));
	}

	/** The room around the listener and the EAX reverb for it, into {@code out} ({@value #ROOM_DOUBLES} doubles). */
	public void room(MemorySegment engine, double[] out) {
		if (out.length < ROOM_DOUBLES) throw new IllegalArgumentException("room needs " + ROOM_DOUBLES + " doubles");
		check((int) invoke(room, engine, MemorySegment.ofArray(out)));
	}

	/**
	 * Measures the room around {@code at} now, on this thread, with {@code rays} × {@code bounces}, into {@code out}
	 * ({@value #ROOM_DOUBLES} doubles, as {@link #room}). A point inside a solid block is measured from beside it.
	 */
	public void roomAt(MemorySegment engine, double[] at, int rays, int bounces, long seed, double[] out) {
		requireLength(at, 3, "at");
		if (out.length < ROOM_DOUBLES) throw new IllegalArgumentException("room needs " + ROOM_DOUBLES + " doubles");
		check((int) invoke(roomAt, engine, MemorySegment.ofArray(at), rays, bounces, seed, MemorySegment.ofArray(out)));
	}

	/**
	 * An impulse response for the room around the listener: first-order ambisonics, W, X, Y, Z interleaved, FuMa, in
	 * world axes, for {@code AL_FORMAT_BFORMAT3D_FLOAT32}. Empty before the room has been measured.
	 */
	public float[] impulseResponse(MemorySegment engine, int sampleRate, float maxSeconds, long seed) {
		long[] needed = new long[1];
		float[] none = new float[0];
		check((int) invoke(impulseResponse, engine, sampleRate, maxSeconds, seed, MemorySegment.ofArray(none), 0L,
				MemorySegment.ofArray(needed)));
		while (true) {
			float[] out = new float[(int) needed[0]];
			check((int) invoke(impulseResponse, engine, sampleRate, maxSeconds, seed, MemorySegment.ofArray(out),
					(long) out.length, MemorySegment.ofArray(needed)));
			// The room may have changed between the calls, and with it the length.
			if (needed[0] <= out.length) return needed[0] == out.length ? out : Arrays.copyOf(out, (int) needed[0]);
		}
	}

	/** How much the engine traces from now on ({@value #QUALITY_DOUBLES} doubles). */
	public void setQuality(MemorySegment engine, double[] quality) {
		requireLength(quality, QUALITY_DOUBLES, "quality");
		check((int) invoke(setQuality, engine, MemorySegment.ofArray(quality)));
	}

	/** What the engine is spending ({@value #STATS_DOUBLES} doubles). */
	public double[] stats(MemorySegment engine) {
		double[] out = new double[STATS_DOUBLES];
		check((int) invoke(stats, engine, MemorySegment.ofArray(out)));
		return out;
	}

	/**
	 * The native {@code YakVcVoiceSink} ({@code yakvc.h}) for Yak VC's voice, to offer it in Fabric's object share:
	 * {@code version} 1 for other players' voices, 2 to also get the player's own. Both live for the rest of the
	 * process.
	 */
	public MemorySegment voiceSink(int version) {
		MemorySegment sink = (MemorySegment) invoke(voiceSink, version);
		if (sink.address() == 0) throw new IllegalArgumentException("no voice sink version " + version);
		return sink;
	}

	/** How often Yak VC has asked a voice sink for a frame: a count that stays put means it refused the sink. */
	public long voicePolls() {
		return (long) invoke(voicePolls);
	}

	/** Keeps the player's own voice as a stream from now on, or drops it as it comes. */
	public void setOwnVoice(boolean keep) {
		invoke(voiceOwn, keep);
	}

	/**
	 * The address of the native OpenAL buffer callback that plays a voice stream, for {@code alBufferCallbackSOFT}
	 * with mono float32 at 48 kHz and {@link #voiceStream}'s pointer.
	 */
	public long voiceCallback() {
		return ((MemorySegment) invoke(voiceCallback)).address();
	}

	/** The pointer {@link #voiceCallback} plays {@code speaker}'s voice from; valid for the rest of the process. */
	public long voiceStream(UUID speaker) {
		long[] out = new long[1];
		check((int) invoke(voiceStream, MemorySegment.ofArray(uuidBytes(speaker)), MemorySegment.ofArray(out)));
		return out[0];
	}

	/**
	 * One speaker's voice stream, as its latest frame left it.
	 *
	 * @param flags             Yak VC's {@code YAKVC_VOICE_*} flags
	 * @param heardFrom         where Yak VC's speakers would place the voice, x, y, z
	 * @param secondsSinceFrame infinite before the first frame
	 * @param ended             whether Yak VC said the speaker's link is gone
	 * @param queuedSeconds     voice waiting for OpenAL to play it
	 */
	public record VoiceStream(UUID speaker, int flags, float range, float volume, float occlusion,
			double[] heardFrom, double secondsSinceFrame, boolean ended, double queuedSeconds) {}

	/** Every voice stream heard this session. */
	public List<VoiceStream> voiceStreams() {
		int cap = 16;
		while (true) {
			byte[] uuids = new byte[16 * cap];
			double[] info = new double[VOICE_DOUBLES * cap];
			long[] count = new long[1];
			check((int) invoke(voiceStreams, MemorySegment.ofArray(uuids), MemorySegment.ofArray(info), (long) cap,
					MemorySegment.ofArray(count)));
			if (count[0] > cap) {
				cap = (int) count[0];
				continue;
			}
			List<VoiceStream> streams = new ArrayList<>((int) count[0]);
			ByteBuffer ids = ByteBuffer.wrap(uuids);
			for (int i = 0; i < count[0]; i++) {
				int at = VOICE_DOUBLES * i;
				streams.add(new VoiceStream(new UUID(ids.getLong(16 * i), ids.getLong(16 * i + 8)), (int) info[at],
						(float) info[at + 1], (float) info[at + 2], (float) info[at + 3],
						Arrays.copyOfRange(info, at + 4, at + 7), info[at + 7], info[at + 8] != 0, info[at + 9]));
			}
			return streams;
		}
	}

	/** A UUID's 16 bytes, most significant first, as Yak VC has them. */
	public static byte[] uuidBytes(UUID uuid) {
		return ByteBuffer.allocate(16).putLong(uuid.getMostSignificantBits()).putLong(uuid.getLeastSignificantBits())
				.array();
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
