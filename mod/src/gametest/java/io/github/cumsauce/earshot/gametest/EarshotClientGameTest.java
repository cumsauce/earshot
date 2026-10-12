package io.github.cumsauce.earshot.gametest;

import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import io.github.cumsauce.earshot.world.AcousticMaterial;
import io.github.cumsauce.earshot.world.MaterialMap;
import io.github.cumsauce.earshot.world.WorldMirror;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.lang.foreign.MemorySegment;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.Map;
import net.fabricmc.fabric.api.client.gametest.v1.FabricClientGameTest;
import net.fabricmc.fabric.api.client.gametest.v1.context.ClientGameTestContext;
import net.fabricmc.fabric.api.client.gametest.v1.context.TestSingleplayerContext;
import net.minecraft.core.BlockPos;
import net.minecraft.world.item.DyeColor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;

/**
 * Starts the real client and checks M0 and M1:
 * <ul>
 * <li>the native library loaded, and the Library mixin probed the game's own OpenAL (the report goes to
 * {@code openal-probe.txt} in the run directory, which CI keeps);
 * <li>blocks map to sensible materials, once the world's tags are in;
 * <li>the world mirror copies a superflat world, follows block changes, and a wall built into it muffles;
 * <li>{@code /earshot rays} draws traced paths (screenshot {@code rays-in-a-glass-room}).
 * </ul>
 *
 * <p>CI has no sound card, so it runs this with {@code ALSOFT_DRIVERS=null}: OpenAL Soft's null backend still opens
 * a device and context with all of its features.
 */
@SuppressWarnings("UnstableApiUsage")
public class EarshotClientGameTest implements FabricClientGameTest {
	/** The listener stands on the superflat ground (grass at y -61) at x 0.5, z 0.5. */
	private static final double[] LISTENER = {0.5, -59.5, 0.5};
	/** A source 8 blocks east, beyond where the wall goes. */
	private static final double[] SOURCE = {8.5, -59.5, 0.5};

	@Override
	public void runTest(ClientGameTestContext context) {
		if (EarshotClient.nativeAbi() != NativeBridge.ABI_VERSION) {
			throw new AssertionError("native library ABI " + EarshotClient.nativeAbi() + ", expected "
					+ NativeBridge.ABI_VERSION);
		}
		OpenAlProbe.Report report = OpenAlProbe.last();
		if (report == null) {
			throw new AssertionError("the OpenAL probe never ran: no sound device opened, or the Library mixin missed");
		}
		try {
			Files.writeString(Path.of("openal-probe.txt"), report.describe() + "\n");
		} catch (IOException e) {
			throw new UncheckedIOException(e);
		}
		if (!report.efx()) throw new AssertionError("the game's OpenAL has no EFX");

		WorldMirror mirror = EarshotClient.mirror();
		NativeBridge bridge = EarshotClient.bridge();
		MemorySegment engine = EarshotClient.engine();
		if (mirror == null || bridge == null || engine == null) throw new AssertionError("the engine didn't start");
		try (TestSingleplayerContext singleplayer = context.worldBuilder().create()) {
			singleplayer.getServer().runCommand("tp @a 0.5 -60 0.5 0 0");
			singleplayer.getConnection().waitForChunksRender();
			// Tags, which some materials depend on, come with the world.
			context.runOnClient(mc -> checkMaterials(mirror.materials()));
			BlockPos wall = new BlockPos(4, -60, 0);
			context.waitFor(mc -> mirror.isMirrored(wall) && mirror.isMirrored(new BlockPos(8, -60, 0)));
			expectTransmission(context, bridge, engine, "open air", 0.99f, 1.01f);

			singleplayer.getServer().runCommand("fill 4 -60 -4 4 -56 4 stone");
			expectTransmission(context, bridge, engine, "a stone wall", 0f, 0.05f);

			// A wooden door in the wall: closed, it still muffles; open, it lets sound straight through.
			singleplayer.getServer().runCommand("setblock 4 -60 0 oak_door[facing=east,half=lower,open=false]");
			singleplayer.getServer().runCommand("setblock 4 -59 0 oak_door[facing=east,half=upper,open=false]");
			expectTransmission(context, bridge, engine, "a closed door", 0.01f, 0.2f);
			singleplayer.getServer().runCommand("setblock 4 -60 0 oak_door[facing=east,half=lower,open=true]");
			singleplayer.getServer().runCommand("setblock 4 -59 0 oak_door[facing=east,half=upper,open=true]");
			expectTransmission(context, bridge, engine, "an open door", 0.99f, 1.01f);

			// In the open about half the rays go up and away; the rest bounce off the ground and escape after.
			NativeBridge.Trace open = context.computeOnClient(mc -> bridge.tracePaths(engine,
					new double[] {0.5, -59.5, -10.5}, 256, 4, 64, 1, 0));
			if (open.escapedFirst() < 100 || open.escapedFirst() > 160) {
				throw new AssertionError("open superflat: " + open);
			}

			// The rays drawn by /earshot rays, in a glass room: daylight gets in, and glass reflects.
			singleplayer.getServer().runCommand("time set noon");
			singleplayer.getServer().runCommand("fill -6 -61 -4 6 -53 12 glass hollow");
			double[] beyondGlass = {0.5, -59.5, 14.5};
			context.waitFor(mc -> bridge.transmission(engine, LISTENER, beyondGlass)[0] < 0.5f, 5 * 20);
			context.runOnClient(mc -> mc.player.connection.sendCommand("earshot rays 256 6"));
			context.waitTicks(2);
			context.takeScreenshot("rays-in-a-glass-room");
		}
	}

	private static void expectTransmission(ClientGameTestContext context, NativeBridge bridge, MemorySegment engine,
			String what, float min, float max) {
		try {
			context.waitFor(mc -> {
				float low = bridge.transmission(engine, LISTENER, SOURCE)[0];
				return low >= min && low <= max;
			}, 5 * 20);
		} catch (RuntimeException e) {
			float[] through = context.computeOnClient(mc -> bridge.transmission(engine, LISTENER, SOURCE));
			throw new AssertionError("through " + what + ": " + Arrays.toString(through) + ", wanted the low band in "
					+ min + ".." + max, e);
		}
	}

	private static void checkMaterials(MaterialMap materials) {
		Map<BlockState, AcousticMaterial> expected = Map.ofEntries(
				Map.entry(Blocks.AIR.defaultBlockState(), AcousticMaterial.AIR),
				Map.entry(Blocks.STONE.defaultBlockState(), AcousticMaterial.ROCK),
				Map.entry(Blocks.DEEPSLATE_BRICKS.defaultBlockState(), AcousticMaterial.ROCK),
				Map.entry(Blocks.OAK_PLANKS.defaultBlockState(), AcousticMaterial.WOOD),
				Map.entry(Blocks.OAK_SLAB.defaultBlockState(), AcousticMaterial.WOOD),
				Map.entry(Blocks.GLASS.defaultBlockState(), AcousticMaterial.GLASS),
				Map.entry(Blocks.GLASS_PANE.defaultBlockState(), AcousticMaterial.GLASS),
				Map.entry(Blocks.IRON_BARS.defaultBlockState(), AcousticMaterial.AIR),
				Map.entry(Blocks.WOOL.pick(DyeColor.WHITE).defaultBlockState(), AcousticMaterial.CLOTH),
				Map.entry(Blocks.CARPET.pick(DyeColor.WHITE).defaultBlockState(), AcousticMaterial.AIR),
				Map.entry(Blocks.OAK_LEAVES.defaultBlockState(), AcousticMaterial.LEAVES),
				Map.entry(Blocks.DIRT.defaultBlockState(), AcousticMaterial.SOIL),
				Map.entry(Blocks.GRASS_BLOCK.defaultBlockState(), AcousticMaterial.SOIL),
				Map.entry(Blocks.SAND.defaultBlockState(), AcousticMaterial.SAND),
				Map.entry(Blocks.IRON_BLOCK.defaultBlockState(), AcousticMaterial.METAL),
				Map.entry(Blocks.WATER.defaultBlockState(), AcousticMaterial.WATER),
				Map.entry(Blocks.LAVA.defaultBlockState(), AcousticMaterial.LAVA),
				Map.entry(Blocks.ICE.defaultBlockState(), AcousticMaterial.ICE),
				Map.entry(Blocks.SNOW_BLOCK.defaultBlockState(), AcousticMaterial.SNOW),
				Map.entry(Blocks.TORCH.defaultBlockState(), AcousticMaterial.AIR),
				Map.entry(Blocks.SHORT_GRASS.defaultBlockState(), AcousticMaterial.AIR),
				Map.entry(Blocks.OAK_FENCE.defaultBlockState(), AcousticMaterial.AIR),
				Map.entry(Blocks.OAK_DOOR.defaultBlockState().setValue(BlockStateProperties.OPEN, false), AcousticMaterial.WOOD),
				Map.entry(Blocks.OAK_DOOR.defaultBlockState().setValue(BlockStateProperties.OPEN, true), AcousticMaterial.AIR));
		StringBuilder wrong = new StringBuilder();
		expected.forEach((state, material) -> {
			AcousticMaterial actual = materials.material(state);
			if (actual != material) wrong.append("\n  ").append(state).append(": ").append(actual);
		});
		if (!wrong.isEmpty()) throw new AssertionError("wrong materials:" + wrong);
	}
}
