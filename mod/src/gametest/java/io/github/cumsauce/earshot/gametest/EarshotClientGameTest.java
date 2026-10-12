package io.github.cumsauce.earshot.gametest;

import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.audio.SoundShaper;
import io.github.cumsauce.earshot.audio.VoicePlayer;
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
import java.util.Optional;
import java.util.UUID;
import java.util.function.Predicate;
import net.fabricmc.fabric.api.client.gametest.v1.FabricClientGameTest;
import net.fabricmc.fabric.api.client.gametest.v1.context.ClientGameTestContext;
import net.fabricmc.fabric.api.client.gametest.v1.context.TestSingleplayerContext;
import net.minecraft.client.resources.sounds.SimpleSoundInstance;
import net.minecraft.client.resources.sounds.SoundInstance;
import net.minecraft.core.BlockPos;
import net.minecraft.sounds.SoundEvents;
import net.minecraft.sounds.SoundSource;
import net.minecraft.world.item.DyeColor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.phys.Vec3;

/**
 * Starts the real client and checks M0 and M1:
 * <ul>
 * <li>the native library loaded, and the Library mixin probed the game's own OpenAL (the report goes to
 * {@code openal-probe.txt} in the run directory, which CI keeps);
 * <li>blocks map to sensible materials, once the world's tags are in;
 * <li>the world mirror copies a superflat world, follows block changes, and a wall built into it muffles;
 * <li>a playing sound is muffled in a closed box, comes round through a doorway, and is clear in the open (M2);
 * <li>Yak VC's voice, through Earshot's voice sink, is shaped the same way and played as it comes (M4);
 * <li>the reverb is nearly dry on open ground and rings in a stone hall (M3);
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

			// M2: a sound in a closed stone box is muffled; a doorway brings it round, heard from the doorway's side;
			// in the open it is heard as it is.
			SoundShaper shaper = EarshotClient.shaper();
			if (shaper == null) throw new AssertionError("no sound shaper");
			singleplayer.getServer().runCommand("fill -10 -60 -10 20 -50 10 air");
			singleplayer.getServer().runCommand("fill 6 -61 -3 11 -55 4 stone hollow");
			Vec3 inside = new Vec3(8.5, -59.5, 0.5);
			double[] insideXyz = {inside.x, inside.y, inside.z};
			context.waitFor(mc -> bridge.transmission(engine, LISTENER, insideXyz)[0] < 0.05f, 5 * 20);
			context.runOnClient(mc -> mc.getSoundManager().play(new SimpleSoundInstance(
					SoundEvents.BEACON_AMBIENT.location(), SoundSource.BLOCKS, 1f, 1f,
					SoundInstance.createUnseededRandom(), true, 0, SoundInstance.Attenuation.LINEAR, inside.x, inside.y,
					inside.z, false)));
			awaitSound(context, shaper, inside, s -> s.gains()[0] < 0.1f, "in a closed box");
			singleplayer.getServer().runCommand("fill 8 -60 -3 8 -59 -3 air");
			awaitSound(context, shaper, inside, s -> s.gains()[1] > 0.1f && s.heardFrom().z < 0.3,
					"round through a doorway on the far side");
			singleplayer.getServer().runCommand("fill 6 -61 -3 11 -55 4 air");
			awaitSound(context, shaper, inside, s -> s.gains()[0] > 0.95f && s.heardFrom().distanceTo(inside) < 0.3,
					"in the open");
			context.runOnClient(mc -> mc.getSoundManager().stop());

			// M4: a voice handed to Earshot's voice sink, as Yak VC would, is shaped like any other sound: muffled in
			// the closed box, clear in the open. OpenAL plays it as it comes, so its queue stays short; once the
			// speaker stops, its source goes.
			if (!report.callbackBuffer()) throw new AssertionError("the game's OpenAL has no callback buffers");
			UUID speaker = UUID.fromString("00000000-0000-4000-8000-0000000000e5");
			singleplayer.getServer().runCommand("fill 6 -61 -3 11 -55 4 stone hollow");
			context.waitFor(mc -> bridge.transmission(engine, LISTENER, insideXyz)[0] < 0.05f, 5 * 20);
			try (FakeSpeaker ignored = FakeSpeaker.start(speaker, inside)) {
				awaitSound(context, shaper, inside, s -> s.voice() && s.gains()[0] < 0.1f, "voice in a closed box");
				singleplayer.getServer().runCommand("fill 6 -61 -3 11 -55 4 air");
				awaitSound(context, shaper, inside, s -> s.voice() && s.gains()[0] > 0.95f, "voice in the open");
				context.waitTicks(10);
				VoicePlayer.Status voice = shaper.voice().status().stream()
						.filter(v -> v.speaker().equals(speaker))
						.findFirst()
						.orElseThrow(() -> new AssertionError("no voice: " + shaper.voice().status()));
				if (!voice.playing() || voice.queuedSeconds() > 0.15) {
					throw new AssertionError("the voice isn't played as it comes: " + voice);
				}
			}
			try {
				context.waitFor(mc -> shaper.voice().status().stream().noneMatch(v -> v.speaker().equals(speaker)),
						5 * 20);
			} catch (RuntimeException | AssertionError e) {
				throw new AssertionError("a silent voice kept its source: " + shaper.voice().status(), e);
			}

			// M3: the reverb follows the room. Out on the superflat it is nearly dry; shut in a stone hall it rings.
			double[] outside = awaitRoom(context, shaper, r -> r[4] < 0.7 && r[1] < 0.6, "on open ground");
			singleplayer.getServer().runCommand("fill -10 -61 -10 10 -50 10 stone hollow");
			double[] hall = awaitRoom(context, shaper, r -> r[4] > 0.98 && r[1] > 2, "in a stone hall");
			if (!(hall[NativeBridge.EAX_FIRST + 2] > 3 * outside[NativeBridge.EAX_FIRST + 2])) {
				throw new AssertionError("the hall's reverb isn't louder: " + Arrays.toString(hall) + " vs "
						+ Arrays.toString(outside));
			}
			// Where OpenAL Soft has the convolution effect, the hall's impulse response loads and plays.
			if (report.convolutionEffect()) {
				int before = context.computeOnClient(mc -> shaper.convolution().loads());
				try {
					context.waitFor(mc -> shaper.mode() == SoundShaper.ReverbMode.CONVOLUTION
							&& shaper.convolution().loads() > before && shaper.convolution().playing(), 10 * 20);
				} catch (RuntimeException | AssertionError e) {
					throw new AssertionError("no convolution reverb: mode " + shaper.mode() + ", loads "
							+ shaper.convolution().loads(), e);
				}
			}
			singleplayer.getServer().runCommand("fill -10 -61 -10 10 -50 10 air");
			singleplayer.getServer().runCommand("fill -10 -61 -10 10 -61 10 grass_block");

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

	/** Waits up to 5 s for the shaped sound at {@code at} to be {@code wanted}, and returns it. */
	private static SoundShaper.Snapshot awaitSound(ClientGameTestContext context, SoundShaper shaper, Vec3 at,
			Predicate<SoundShaper.Snapshot> wanted, String what) {
		try {
			context.waitFor(mc -> find(shaper, at).filter(wanted).isPresent(), 5 * 20);
		} catch (RuntimeException | AssertionError e) {
			throw new AssertionError("the sound " + what + ": "
					+ find(shaper, at).map(SoundShaper.Snapshot::toString).orElse("not playing"), e);
		}
		return find(shaper, at).orElseThrow();
	}

	/** Waits up to 10 s for the room the reverb uses to be {@code wanted}, and returns it. */
	private static double[] awaitRoom(ClientGameTestContext context, SoundShaper shaper, Predicate<double[]> wanted,
			String what) {
		try {
			context.waitFor(mc -> {
				double[] room = shaper.room();
				return room.length > 0 && wanted.test(room);
			}, 10 * 20);
		} catch (RuntimeException | AssertionError e) {
			throw new AssertionError("the room " + what + ": " + Arrays.toString(shaper.room()), e);
		}
		return shaper.room();
	}

	private static Optional<SoundShaper.Snapshot> find(SoundShaper shaper, Vec3 at) {
		return shaper.snapshot().stream().filter(s -> s.position().distanceTo(at) < 0.5).findFirst();
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
