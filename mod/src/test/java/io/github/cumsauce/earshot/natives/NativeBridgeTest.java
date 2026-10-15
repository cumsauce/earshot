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
