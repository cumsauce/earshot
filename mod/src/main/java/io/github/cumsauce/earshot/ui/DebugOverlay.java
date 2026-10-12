package io.github.cumsauce.earshot.ui;

import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.audio.SoundShaper;
import io.github.cumsauce.earshot.audio.VoicePlayer;
import io.github.cumsauce.earshot.config.EarshotConfig;
import io.github.cumsauce.earshot.natives.NativeBridge;
import java.lang.foreign.MemorySegment;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Locale;
import net.fabricmc.fabric.api.client.rendering.v1.hud.HudElement;
import net.fabricmc.loader.api.FabricLoader;
import net.minecraft.client.DeltaTracker;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.Font;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.world.phys.Vec3;

/**
 * What Earshot is doing, in the top-left corner, in the spirit of F3 (Yak VC's overlay takes the top right): the quality and what tracing costs, the room
 * the reverb hears, which reverbs play, and the nearest sounds with how they are heard. English only, like F3, so
 * screenshots read the same in every bug report. Toggled by its key or {@code /earshot overlay}.
 */
public final class DebugOverlay implements HudElement {
	private static final int TEXT = 0xFFE0E0E0;
	private static final int BACKGROUND = 0x90505050;
	/** How many of the nearest sounds are listed. */
	private static final int SOUNDS = 6;

	private static volatile boolean shown;

	/** Whether the overlay is on. */
	public static boolean shown() {
		return shown;
	}

	public static void toggle() {
		shown = !shown;
	}

	@Override
	public void extractRenderState(GuiGraphicsExtractor graphics, DeltaTracker deltaTracker) {
		Minecraft minecraft = Minecraft.getInstance();
		if (!shown || minecraft.player == null) return;
		Font font = minecraft.font;
		List<String> lines = lines(minecraft);
		int y = 2;
		for (String line : lines) {
			int width = font.width(line);
			int x = 2;
			graphics.fill(x - 1, y - 1, x + width + 1, y + font.lineHeight - 1, BACKGROUND);
			graphics.text(font, line, x, y, TEXT, false);
			y += font.lineHeight;
		}
	}

	static List<String> lines(Minecraft minecraft) {
		List<String> lines = new ArrayList<>();
		String version = FabricLoader.getInstance().getModContainer("earshot")
				.map(mod -> mod.getMetadata().getVersion().getFriendlyString()).orElse("?");
		EarshotConfig config = EarshotClient.config();
		lines.add("Earshot " + version + ": " + config.preset().key()
				+ (config.preset() == EarshotConfig.Preset.AUTO ? " (" + config.preset().resolve().key() + ")" : "")
				+ (EarshotClient.bypassed() ? ", BYPASSED" : ""));
		SoundShaper shaper = EarshotClient.shaper();
		NativeBridge bridge = EarshotClient.bridge();
		MemorySegment engine = EarshotClient.engine();
		if (shaper == null || bridge == null || engine == null) {
			lines.add("Engine not running");
			return lines;
		}
		double[] stats = bridge.stats(engine);
		lines.add(String.format(Locale.ROOT, "Trace: %,d rays, %.0f%% of a core; paths %.0f blocks, %.1f ms", (long) stats[0],
				100 * stats[1], stats[3], stats[2]));
		double[] room = shaper.room();
		if (room.length > 0) {
			lines.add(String.format(Locale.ROOT, "Room: decay %.2f / %.2f / %.2f s, free path %.1f, %.0f%% enclosed",
					room[0], room[1], room[2], room[3], 100 * room[4]));
		}
		lines.add(String.format(Locale.ROOT, "Reverb: %s, %d responses loaded, %d other spaces",
				shaper.mode().name().toLowerCase(Locale.ROOT), shaper.convolution().loads(), shaper.spaceRooms().size()));
		List<VoicePlayer.Status> voices = shaper.voice().status();
		if (shaper.voice().offered() || !voices.isEmpty()) {
			long own = voices.stream().filter(VoicePlayer.Status::own).count();
			lines.add("Voice: " + (voices.size() - own) + " speakers" + (own > 0 ? ", own voice echoing" : ""));
		}

		List<SoundShaper.Snapshot> sounds = new ArrayList<>(shaper.snapshot());
		Vec3 eye = minecraft.player.getEyePosition();
		sounds.sort(Comparator.comparingDouble(s -> s.position().distanceTo(eye)));
		lines.add("Sounds: " + sounds.size());
		for (SoundShaper.Snapshot s : sounds.subList(0, Math.min(SOUNDS, sounds.size()))) {
			StringBuilder line = new StringBuilder(String.format(Locale.ROOT, "%s%.0f m: %s / %s / %s dB",
					s.voice() ? "voice " : "", s.position().distanceTo(eye), db(s.gains()[0]), db(s.gains()[1]),
					db(s.gains()[2])));
			if (s.heardFrom().distanceTo(s.position()) > 0.5) line.append(", round a corner");
			if (s.elsewhere()) line.append(", own space");
			if (s.early()) line.append(", early echoes");
			lines.add(line.toString());
		}
		return lines;
	}

	private static String db(float energy) {
		return energy >= 0.999f ? "0" : String.format(Locale.ROOT, "%.0f", 10 * Math.log10(Math.max(energy, 1e-6)));
	}
}
