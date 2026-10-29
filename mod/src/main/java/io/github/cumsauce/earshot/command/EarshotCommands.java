package io.github.cumsauce.earshot.command;

import com.mojang.brigadier.arguments.IntegerArgumentType;
import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.audio.SoundShaper;
import io.github.cumsauce.earshot.config.EarshotConfig;
import io.github.cumsauce.earshot.world.MaterialOverrides;
import io.github.cumsauce.earshot.audio.VoicePlayer;
import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.world.WorldMirror;
import java.lang.foreign.MemorySegment;
import java.util.Locale;
import net.fabricmc.fabric.api.client.command.v2.ClientCommandRegistrationCallback;
import net.fabricmc.fabric.api.client.command.v2.ClientCommands;
import net.fabricmc.fabric.api.client.command.v2.FabricClientCommandSource;
import net.minecraft.core.particles.DustParticleOptions;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.HitResult;
import net.minecraft.network.chat.Component;
import net.minecraft.world.phys.Vec3;

/**
 * Client commands for looking inside the acoustics while developing it:
 * <ul>
 * <li>{@code /earshot rays [count] [bounces]} traces paths from your eyes and draws them as particles, bright where
 * the sound is still strong, blue where it escaped to open air;
 * <li>{@code /earshot occlusion} says how much of the sound from 16 blocks ahead gets straight through whatever is in
 * between;
 * <li>{@code /earshot sources} lists the playing sounds and how each is heard: its gains and, when it comes round
 * a corner, where from;
 * <li>{@code /earshot room} describes the room around you as the reverb hears it;
 * <li>{@code /earshot reverb auto|convolution|eax|off} picks the reverb, for listening to them side by side;
 * <li>{@code /earshot stats} says how much of the world is mirrored.
 * </ul>
 */
public final class EarshotCommands {
	private static final double REACH = 64;
	/** Legs kept for drawing, and particles per block along them. */
	private static final int MAX_SEGMENTS = 4096;
	private static final double PARTICLES_PER_BLOCK = 4;
	private static final int MAX_PARTICLES = 12000;
	private static final float PARTICLE_SIZE = 0.3f;
	private static final int ESCAPED_COLOR = 0x4fa8ff;

	private EarshotCommands() {}

	public static void register(NativeBridge bridge, MemorySegment engine, WorldMirror mirror, SoundShaper shaper) {
		ClientCommandRegistrationCallback.EVENT.register((dispatcher, registryAccess) -> dispatcher.register(
				ClientCommands.literal("earshot")
						.then(ClientCommands.literal("rays")
								.executes(c -> rays(bridge, engine, c.getSource(), 64, 4))
								.then(ClientCommands.argument("count", IntegerArgumentType.integer(1, 4096))
										.executes(c -> rays(bridge, engine, c.getSource(),
												IntegerArgumentType.getInteger(c, "count"), 4))
										.then(ClientCommands.argument("bounces", IntegerArgumentType.integer(0, 32))
												.executes(c -> rays(bridge, engine, c.getSource(),
														IntegerArgumentType.getInteger(c, "count"),
														IntegerArgumentType.getInteger(c, "bounces"))))))
						.then(ClientCommands.literal("occlusion").executes(c -> occlusion(bridge, engine, c.getSource())))
						.then(ClientCommands.literal("sources").executes(c -> sources(shaper, c.getSource())))
						.then(ClientCommands.literal("room").executes(c -> room(shaper, c.getSource())))
						.then(reverbModes(shaper))
						.then(ClientCommands.literal("voice").executes(c -> voice(shaper.voice(), c.getSource()))
								.then(ClientCommands.literal("on")
										.executes(c -> setVoice(shaper.voice(), c.getSource(), true)))
								.then(ClientCommands.literal("off")
										.executes(c -> setVoice(shaper.voice(), c.getSource(), false))))
						.then(ClientCommands.literal("overlay").executes(c -> {
							io.github.cumsauce.earshot.ui.DebugOverlay.toggle();
							return 1;
						}))
						.then(ClientCommands.literal("material").executes(c -> material(mirror, c.getSource())))
						.then(ClientCommands.literal("materials").then(ClientCommands.literal("reload")
								.executes(c -> reloadMaterials(mirror, c.getSource()))))
						.then(ClientCommands.literal("stats").executes(c -> stats(bridge, engine, mirror, c.getSource())))));
	}

	private static int rays(NativeBridge bridge, MemorySegment engine, FabricClientCommandSource source, int count,
			int bounces) {
		Vec3 eye = source.getPlayer().getEyePosition();
		long seed = source.getLevel().getGameTime();
		NativeBridge.Trace trace = bridge.tracePaths(engine, new double[] {eye.x, eye.y, eye.z}, count, bounces, REACH,
				seed, MAX_SEGMENTS);
		draw(source, trace);
		source.sendFeedback(Component.literal(String.format(Locale.ROOT,
				"%d rays, %d legs: %.0f %% escaped at once (open sky), mean free path %.1f blocks", count, trace.legs(),
				100.0 * trace.escapedFirst() / count, trace.meanFreePath())));
		return 1;
	}

	/** Draws the traced legs as dust: yellow fading to red as the sound loses energy, blue where it escaped. */
	private static void draw(FabricClientCommandSource source, NativeBridge.Trace trace) {
		double[] s = trace.segments();
		double total = 0;
		for (int i = 0; i < trace.written(); i++) total += length(s, i * NativeBridge.SEGMENT_DOUBLES);
		double spacing = Math.max(1 / PARTICLES_PER_BLOCK, total / MAX_PARTICLES);
		for (int i = 0; i < trace.written(); i++) {
			int at = i * NativeBridge.SEGMENT_DOUBLES;
			double energy = s[at + 6];
			boolean escaped = s[at + 7] != 0;
			int color = escaped ? ESCAPED_COLOR : warmth(energy);
			DustParticleOptions dust = new DustParticleOptions(color, PARTICLE_SIZE);
			double length = length(s, at);
			for (double t = 0; t < length; t += spacing) {
				double f = t / length;
				source.getLevel().addParticle(dust, true, true, s[at] + (s[at + 3] - s[at]) * f,
						s[at + 1] + (s[at + 4] - s[at + 1]) * f, s[at + 2] + (s[at + 5] - s[at + 2]) * f, 0, 0, 0);
			}
		}
	}

	/** Yellow at full energy to dark red at 60 dB down. */
	private static int warmth(double energy) {
		double db = Math.max(-60, 10 * Math.log10(Math.max(energy, 1e-9)));
		double f = 1 + db / 60;
		int red = (int) (128 + 127 * f);
		int green = (int) (220 * f);
		return red << 16 | green << 8;
	}

	private static double length(double[] s, int at) {
		double dx = s[at + 3] - s[at], dy = s[at + 4] - s[at + 1], dz = s[at + 5] - s[at + 2];
		return Math.sqrt(dx * dx + dy * dy + dz * dz);
	}

	private static int occlusion(NativeBridge bridge, MemorySegment engine, FabricClientCommandSource source) {
		Vec3 eye = source.getPlayer().getEyePosition();
		Vec3 ahead = eye.add(source.getPlayer().getLookAngle().scale(16));
		float[] through = bridge.transmission(engine, new double[] {eye.x, eye.y, eye.z},
				new double[] {ahead.x, ahead.y, ahead.z});
		source.sendFeedback(Component.literal(String.format(Locale.ROOT,
				"From 16 blocks ahead, straight through: low %s, mid %s, high %s", db(through[0]), db(through[1]),
				db(through[2]))));
		return 1;
	}

	private static String db(float share) {
		// Shares over unity are real (a lift or ground gain can exceed 1), so print the value; only the
		// log of a near-unity share would come out as "-0.0", which the snap to 0 hides.
		double db = 10 * Math.log10(Math.max(share, 1e-12));
		if (Math.abs(db) < 0.05) db = 0;
		return String.format(Locale.ROOT, "%.1f dB", db);
	}

	private static int sources(SoundShaper shaper, FabricClientCommandSource source) {
		var sounds = shaper.snapshot();
		source.sendFeedback(Component.literal(sounds.size() + " positional sounds playing"));
		Vec3 eye = source.getPlayer().getEyePosition();
		sounds.stream()
				.sorted(java.util.Comparator.comparingDouble(s -> s.position().distanceTo(eye)))
				.limit(8)
				.forEach(s -> {
					boolean round = s.heardFrom().distanceTo(s.position()) > 0.5;
					source.sendFeedback(Component.literal(String.format(Locale.ROOT,
							"  %s%.0f blocks away: low %s, mid %s, high %s%s", s.voice() ? "voice " : "",
							s.position().distanceTo(eye),
							db(s.gains()[0]), db(s.gains()[1]), db(s.gains()[2]),
							(round ? String.format(Locale.ROOT, ", round from %.0f %.0f %.0f", s.heardFrom().x,
									s.heardFrom().y, s.heardFrom().z) : "")
									+ (s.elsewhere() ? ", with its own space's reverb" : ""))));
				});
		return 1;
	}

	private static int voice(VoicePlayer voice, FabricClientCommandSource source) {
		source.sendFeedback(Component.literal(voice.offered()
				? "Yak VC's voice plays through Earshot (if Yak VC is installed)"
				: "Yak VC's voice plays through its own speakers"));
		Vec3 eye = source.getPlayer().getEyePosition();
		for (VoicePlayer.Status v : voice.status()) {
			var info = source.getClient().getConnection() == null ? null
					: source.getClient().getConnection().getPlayerInfo(v.speaker());
			String who = info != null ? info.getProfile().name() : v.speaker().toString();
			String where = v.position() == null ? "centred"
					: String.format(Locale.ROOT, "%.0f blocks away, low %s, high %s", v.position().distanceTo(eye),
							db(v.gains()[0]), db(v.gains()[2]));
			source.sendFeedback(Component.literal(String.format(Locale.ROOT, "  %s: %s, %s, %.0f ms queued", who,
					v.playing() ? "playing" : "stopped", where, 1000 * v.queuedSeconds())));
		}
		return 1;
	}

	private static int setVoice(VoicePlayer voice, FabricClientCommandSource source, boolean on) {
		EarshotClient.apply(EarshotClient.config().withVoice(on));
		return voice(voice, source);
	}

	/** The material of the block the player looks at, and whether it was set by hand. */
	private static int material(WorldMirror mirror, FabricClientCommandSource source) {
		if (!(source.getClient().hitResult instanceof BlockHitResult hit) || hit.getType() == HitResult.Type.MISS) {
			source.sendError(Component.literal("Look at a block"));
			return 0;
		}
		BlockState state = source.getPlayer().level().getBlockState(hit.getBlockPos());
		boolean set = mirror.materials().overrides().find(state) != null;
		source.sendFeedback(Component.literal(String.format(Locale.ROOT, "%s: %s%s",
				BuiltInRegistries.BLOCK.getKey(state.getBlock()), mirror.materials().material(state).nativeName(),
				set ? " (set in materials.json)" : "")));
		return 1;
	}

	private static int reloadMaterials(WorldMirror mirror, FabricClientCommandSource source) {
		MaterialOverrides overrides = MaterialOverrides.load(EarshotClient.materialsFile());
		mirror.setOverrides(overrides);
		source.sendFeedback(Component.literal(overrides.size() + " material overrides from "
				+ EarshotClient.materialsFile() + "; copying the world again"));
		return 1;
	}

	private static com.mojang.brigadier.builder.LiteralArgumentBuilder<FabricClientCommandSource> reverbModes(
			SoundShaper shaper) {
		var reverb = ClientCommands.literal("reverb").executes(c -> {
			c.getSource().sendFeedback(Component.literal("Reverb: " + shaper.mode()));
			return 1;
		});
		for (EarshotConfig.Reverb choice : EarshotConfig.Reverb.values()) {
			reverb.then(ClientCommands.literal(choice.key()).executes(c -> {
				EarshotClient.apply(EarshotClient.config().withReverb(choice));
				c.getSource().sendFeedback(Component.literal("Reverb: " + (choice == EarshotConfig.Reverb.AUTO
						? "auto (the best this OpenAL has)" : choice.key())));
				return 1;
			}));
		}
		return reverb;
	}

	private static int room(SoundShaper shaper, FabricClientCommandSource source) {
		double[] room = shaper.room();
		if (room.length == 0) {
			source.sendError(Component.literal("No room measured yet"));
			return 0;
		}
		int eax = NativeBridge.EAX_FIRST;
		source.sendFeedback(Component.literal(String.format(Locale.ROOT,
				"Decay %.2f / %.2f / %.2f s (low / mid / high), mean free path %.1f blocks, %.0f %% enclosed", room[0],
				room[1], room[2], room[3], room[4] * 100)));
		source.sendFeedback(Component.literal(String.format(Locale.ROOT,
				"Reverb: gain %.2f, reflections %.2f after %.0f ms, late %.2f after %.0f ms, echo %.2f", room[eax + 2],
				room[eax + 8], room[eax + 9] * 1000, room[eax + 10], room[eax + 11] * 1000, room[eax + 13])));
		source.sendFeedback(Component.literal("Mode " + shaper.mode() + ", impulse responses loaded: "
				+ shaper.convolution().loads()));
		return 1;
	}

	private static int stats(NativeBridge bridge, MemorySegment engine, WorldMirror mirror,
			FabricClientCommandSource source) {
		source.sendFeedback(Component.literal(String.format(Locale.ROOT,
				"Mirrored %d chunks (%d sections with blocks), %d waiting", mirror.mirrored(),
				bridge.sectionCount(engine), mirror.pending())));
		double[] stats = bridge.stats(engine);
		EarshotConfig config = EarshotClient.config();
		source.sendFeedback(Component.literal(String.format(Locale.ROOT,
				"Preset %s (%s): room trace %.0f rays at %.0f %% of a core; pathing field %.0f blocks each way, built in"
						+ " %.1f ms", config.preset().key(), config.preset().resolve().key(), stats[0], 100 * stats[1],
				stats[3], stats[2])));
		return 1;
	}
}
