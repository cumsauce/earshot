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

	/** How long a built panel is left up, in nanoseconds. A tick is 50 ms, and the clock reads on wall time. */
	private static final long REBUILD_NS = 50_000_000L;

	/**
	 * The lines as last built, inside an array so a reader never sees it half-built, and when they were built. The
	 * panel is drawn every frame (60 to 120+ a second) but everything on it moves at the tick rate (20 a second) and
	 * costs a native downcall, a sort and a page of formatting to read; the render thread takes the copy the last build
	 * left rather than making its own. 26.3 exposes no tick counter to compare against, so the clock stands in.
	 */
	private static volatile Object[] cached;
	private static long cachedAt;

	/** Whether the overlay is on. */
	public static boolean shown() {
		return shown;
	}

	public static void toggle() {
		shown = !shown;
		// Built again on this frame rather than showing an empty panel for up to a tick, or the last look from before.
		// Dropped rather than aged: System.nanoTime() has no fixed origin, so no timestamp is reliably "long ago".
		cached = null;
	}

	@Override
	public void extractRenderState(GuiGraphicsExtractor graphics, DeltaTracker deltaTracker) {
		Minecraft minecraft = Minecraft.getInstance();
		if (!shown || minecraft.player == null) return;
		Object[] lines = shownLines(minecraft);
		Font font = minecraft.font;
		int y = 2;
		for (Object line : lines) {
			String text = (String) line;
			int width = font.width(text);
			int x = 2;
			// fill's x2/y2 are exclusive, so this covers one pixel beyond the text on every
			// side: rows y-1 .. y+lineHeight around the text's y .. y+lineHeight-1.
			graphics.fill(x - 1, y - 1, x + width + 1, y + font.lineHeight + 1, BACKGROUND);
			graphics.text(font, text, x, y, TEXT, false);
			y += font.lineHeight;
		}
	}

	/** The lines for this tick, built once: on the render thread frames within 50 ms read what was built. */
	private static Object[] shownLines(Minecraft minecraft) {
		long now = System.nanoTime();
		Object[] lines = cached;
		if (lines != null && now - cachedAt < REBUILD_NS) return lines;
		// Built on the render thread, throttled to roughly once per game tick (50 ms).
		lines = lines(minecraft).toArray();
		cachedAt = now;
		cached = lines;
		return lines;
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
		lines.add(String.format(Locale.ROOT, "Air absorption: %.2f x OpenAL's", shaper.airAbsorption()));
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
			if (s.aperture() < 64) line.append(String.format(Locale.ROOT, " through %.0f m²", s.aperture()));
			if (s.elsewhere()) line.append(", own space");
			if (s.early()) line.append(", early echoes");
			if (s.ownReverb()) line.append(", own reverb");
			lines.add(line.toString());
		}
		return lines;
	}

	private static String db(float energy) {
		// Gains over unity are real (ground reflection reaches MAX_GAIN 2, lift MAX_LIFT 2), so print the
		// value; only the log of a near-unity gain would come out as "-0", which the snap to 0 hides.
		double db = 10 * Math.log10(Math.max(energy, 1e-6));
		if (Math.abs(db) < 0.5) db = 0;
		return String.format(Locale.ROOT, "%.0f", db);
	}
}
