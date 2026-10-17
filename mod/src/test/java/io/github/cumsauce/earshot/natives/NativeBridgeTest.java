package io.github.cumsauce.earshot.natives;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.cumsauce.earshot.world.AcousticMaterial;
import java.lang.foreign.MemorySegment;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class NativeBridgeTest {
	/**
	 * The bound check behind every native count this class trusts: an out-of-range value fails fast naming itself
	 * and the bound, rather than being truncated into a wrong-sized array or index.
	 */
	@Test
	void nativeCountsOutsideTheirBoundFailFast() {
		// Inside the bound, both ends included: nothing is thrown.
		NativeBridge.checkNativeLength(0, 10, "tracePaths segments written");
		NativeBridge.checkNativeLength(10, 10, "tracePaths segments written");
		NativeBridge.checkNativeLength(NativeBridge.MAX_IR_FLOATS, NativeBridge.MAX_IR_FLOATS,
				"impulse response floats");

		IllegalStateException over = assertThrows(IllegalStateException.class, () -> NativeBridge
				.checkNativeLength(NativeBridge.MAX_IR_FLOATS + 1L, NativeBridge.MAX_IR_FLOATS, "impulse response floats"));
		assertTrue(over.getMessage().contains(Long.toString(NativeBridge.MAX_IR_FLOATS + 1L)), over.getMessage());
		assertTrue(over.getMessage().contains(Long.toString(NativeBridge.MAX_IR_FLOATS)), over.getMessage());

		IllegalStateException negative = assertThrows(IllegalStateException.class,
				() -> NativeBridge.checkNativeLength(-1, NativeBridge.MAX_VOICE_STREAMS, "voice streams"));
		assertTrue(negative.getMessage().contains("-1"), negative.getMessage());
		assertTrue(negative.getMessage().contains(Integer.toString(NativeBridge.MAX_VOICE_STREAMS)),
				negative.getMessage());
	}

	/** The impulse response bound is derived from the API's own sizes, and small enough that (int) stays exact. */
	@Test
	void theImpulseResponseBoundCoversEveryCallTheModMakes() {
		int channels = 4; // First-order ambisonics: W, X, Y, Z interleaved.
		assertTrue(NativeBridge.MAX_IR_FLOATS >= channels * 48_000 * 4L,
				"four seconds of 48 kHz impulse response must fit");
		assertTrue(NativeBridge.MAX_IR_FLOATS <= Integer.MAX_VALUE, "the (int) cast must stay exact");
		// The arrays voiceStreams makes for its cap: 16 bytes and VOICE_DOUBLES doubles per stream.
		assertTrue(NativeBridge.MAX_VOICE_STREAMS * 16L <= Integer.MAX_VALUE, "the uuid array must fit an int");
		assertTrue(NativeBridge.MAX_VOICE_STREAMS * 10L <= Integer.MAX_VALUE, "the info array must fit an int");
	}

	/**
	 * The bundled names stay well inside the bound the bridge checks, so the guard never fires on a real call and
	 * the first 1 KiB buffer is always enough. This is the evidence for {@link NativeBridge#MAX_MATERIAL_JSON_BYTES}.
	 */
	@Test
	void theMaterialNamesFitTheBoundTheBridgeChecks(@TempDir Path configDir) throws Exception {
		NativeBridge bridge = new NativeBridge(NativeLoader.load(configDir));
		String json = bridge.materialNames();
		assertTrue(json.length() <= NativeBridge.MAX_MATERIAL_JSON_BYTES, json.length() + " bytes: " + json);
		assertTrue(json.length() <= 1024, "one buffer should suffice: " + json.length() + " bytes");
		assertEquals(AcousticMaterial.values().length, json.split(",").length, json);
	}

	@Test
	void aMirroredWallMuffles(@TempDir Path configDir) throws Exception {
		NativeBridge bridge = new NativeBridge(NativeLoader.load(configDir));
		assertEquals(NativeBridge.ABI_VERSION, bridge.abiVersion());
		MemorySegment engine = bridge.create();
		try {
			double[] from = {0.5, 0.5, 0.5};
			double[] to = {10.5, 0.5, 0.5};
			assertArrayEquals(new float[] {1, 1, 1}, bridge.transmission(engine, from, to));

			bridge.setBlock(engine, 5, 0, 0, AcousticMaterial.ROCK.id());
			float[] walled = bridge.transmission(engine, from, to);
			assertTrue(walled[0] < 0.1f && walled[2] < walled[0], java.util.Arrays.toString(walled));
			assertEquals(1, bridge.sectionCount(engine));

			NativeBridge.Trace trace = bridge.tracePaths(engine, from, 16, 2, 32, 1, 100);
			assertTrue(trace.legs() >= 16 && trace.written() > 0, trace.toString());

			bridge.clearWorld(engine);
			assertEquals(0, bridge.sectionCount(engine));
			// A block byte whose low nibble is no material: 0xE is 14, past
			// the table's last. (0x63 would be a *valid* packed byte — glass
			// at six sixteenths — since the nibble scheme arrived.)
			EarshotException e = assertThrows(EarshotException.class,
					() -> bridge.setBlock(engine, 0, 0, 0, (byte) 0x0E));
			assertEquals(EarshotException.INVALID_ARGUMENT, e.code());
			assertTrue(e.getMessage().contains("unknown material"), e.getMessage());
		} finally {
			bridge.destroy(engine);
		}
	}
}
