package io.github.cumsauce.earshot.gametest;

import static java.lang.foreign.ValueLayout.ADDRESS;
import static java.lang.foreign.ValueLayout.JAVA_DOUBLE;
import static java.lang.foreign.ValueLayout.JAVA_FLOAT;
import static java.lang.foreign.ValueLayout.JAVA_INT;
import static java.lang.foreign.ValueLayout.JAVA_LONG;

import io.github.cumsauce.earshot.audio.VoicePlayer;
import io.github.cumsauce.earshot.natives.NativeBridge;
import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.invoke.MethodHandle;
import java.util.UUID;
import net.fabricmc.loader.api.FabricLoader;
import net.minecraft.world.phys.Vec3;

/**
 * Talks into Earshot's voice sink as Yak VC's audio thread would: takes the {@code YakVcVoiceSink} from the object
 * share and calls its {@code frame} with a 20 ms frame of a 220 Hz tone every 20 ms, from {@code at}, or as the
 * player's own voice ({@code YAKVC_VOICE_SELF | YAKVC_VOICE_UNPOSITIONED}), which a version 2 sink asks for.
 */
final class FakeSpeaker implements AutoCloseable {
	/** {@code YakVcVoiceFrame}'s layout ({@code yakvc.h}); the native tests check the same offsets. */
	private static final long FRAME_SIZE = 88;
	private static final int SAMPLES = 960;

	private final Thread thread;
	private volatile boolean talking = true;

	private final int version;

	private FakeSpeaker(UUID speaker, Vec3 at, int flags) {
		if (!(FabricLoader.getInstance().getObjectShare().get(VoicePlayer.SINK_KEY) instanceof MemorySegment address)) {
			throw new AssertionError("Earshot offered no voice sink");
		}
		MemorySegment sink = address.reinterpret(40);
		version = sink.get(JAVA_INT, 0);
		if (version != 1 && version != 2) throw new AssertionError("voice sink version " + version);
		MethodHandle frame = Linker.nativeLinker().downcallHandle(sink.get(ADDRESS, 24),
				FunctionDescriptor.ofVoid(ADDRESS, ADDRESS));
		MemorySegment user = sink.get(ADDRESS, 8);
		thread = new Thread(() -> talk(frame, user, speaker, at, flags), "earshot-fake-speaker");
		thread.setDaemon(true);
		thread.start();
	}

	/** The version of the sink in the object share now, or 0 if there is none. */
	static int offeredVersion() {
		return FabricLoader.getInstance().getObjectShare().get(VoicePlayer.SINK_KEY) instanceof MemorySegment address
				? address.reinterpret(4).get(JAVA_INT, 0)
				: 0;
	}

	static FakeSpeaker start(UUID speaker, Vec3 at) {
		return new FakeSpeaker(speaker, at, 0);
	}

	/** The player's own voice, as Yak VC sends it to a version 2 sink while they talk. */
	static FakeSpeaker ownVoice(UUID player) {
		return new FakeSpeaker(player, Vec3.ZERO, (1 << 2) | (1 << 1));
	}

	/** The version of the sink Earshot offered. */
	int sinkVersion() {
		return version;
	}

	private void talk(MethodHandle frame, MemorySegment user, UUID speaker, Vec3 at, int flags) {
		try (Arena arena = Arena.ofConfined()) {
			MemorySegment pcm = arena.allocate(JAVA_FLOAT, SAMPLES);
			MemorySegment f = arena.allocate(FRAME_SIZE, 8);
			f.copyFrom(MemorySegment.ofArray(NativeBridge.uuidBytes(speaker)));
			f.set(ADDRESS, 16, pcm);
			f.set(JAVA_LONG, 24, SAMPLES);
			f.set(JAVA_FLOAT, 40, 1f);
			f.set(JAVA_FLOAT, 44, 48f);
			f.set(JAVA_INT, 48, flags);
			f.set(JAVA_DOUBLE, 56, at.x);
			f.set(JAVA_DOUBLE, 64, at.y);
			f.set(JAVA_DOUBLE, 72, at.z);
			f.set(JAVA_FLOAT, 80, 0f);
			long next = System.nanoTime();
			for (long tick = 0; talking; tick++) {
				for (int i = 0; i < SAMPLES; i++) {
					long n = tick * SAMPLES + i;
					pcm.setAtIndex(JAVA_FLOAT, i, (float) (0.3 * Math.sin(2 * Math.PI * 220 * n / 48_000.0)));
				}
				f.set(JAVA_LONG, 32, tick);
				frame.invokeExact(user, f);
				next += 20_000_000L;
				long wait = next - System.nanoTime();
				if (wait > 0) Thread.sleep(wait / 1_000_000L, (int) (wait % 1_000_000L));
			}
		} catch (Throwable t) {
			throw new IllegalStateException("the fake speaker failed", t);
		}
	}

	@Override
	public void close() {
		talking = false;
		try {
			thread.join(1000);
		} catch (InterruptedException e) {
			Thread.currentThread().interrupt();
		}
	}
}
