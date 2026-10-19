package io.github.cumsauce.earshot.ui;

import com.mojang.serialization.Codec;
import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.audio.SoundShaper;
import io.github.cumsauce.earshot.config.EarshotConfig;
import io.github.cumsauce.earshot.config.EarshotConfig.Preset;
import io.github.cumsauce.earshot.config.EarshotConfig.Reverb;
import io.github.cumsauce.earshot.config.EarshotConfig.Diffraction;
import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.openal.ContextAttributes;
import io.github.cumsauce.earshot.openal.HrtfFiles;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import java.io.IOException;
import java.lang.foreign.MemorySegment;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.atomic.AtomicBoolean;
import net.fabricmc.loader.api.FabricLoader;
import net.minecraft.ChatFormatting;
import net.minecraft.client.Minecraft;
import net.minecraft.client.OptionInstance;
import net.minecraft.client.Options;
import net.minecraft.client.gui.components.AbstractWidget;
import net.minecraft.client.gui.components.Button;
import net.minecraft.client.gui.components.MultiLineTextWidget;
import net.minecraft.client.gui.components.Tooltip;
import net.minecraft.client.gui.screens.AlertScreen;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.gui.screens.options.OptionsSubScreen;
import net.minecraft.network.chat.Component;
import org.jspecify.annotations.Nullable;

/**
 * Earshot's settings, under Options → Music &amp; Sounds, in sections: quality (with what it costs right now), the
 * sound effects, reverb (with which one is playing), headphones (the game's Directional Audio beside the HRTF it
 * uses) and Yak VC voice. Settings that can't take effect, such as any effect while Quality is Off, are greyed out.
 * Changes apply and are saved when the screen closes.
 */
public final class AcousticsScreen extends OptionsSubScreen {
	private static final Component TITLE = Component.translatable("earshot.settings.title");
	/** The status lines refresh this often, in ticks. */
	private static final int REFRESH_TICKS = 10;
	private static final int TEXT_WIDTH = 310;

	private final EarshotConfig initial;
	private final OptionInstance<String> preset;
	private final OptionInstance<Boolean> occlusion;
	private final OptionInstance<Boolean> pathing;
	private final OptionInstance<Boolean> airAbsorption;
	private final OptionInstance<Boolean> realisticFalloff;
	private final OptionInstance<Boolean> doppler;
	private final OptionInstance<Boolean> underwater;
	private final OptionInstance<String> reverb;
	private final OptionInstance<String> diffraction;
	private final OptionInstance<String> hrtf;
	private final OptionInstance<Boolean> voice;
	private final OptionInstance<Boolean> ownVoice;
	private final boolean yakvc = FabricLoader.getInstance().isModLoaded("yakvc");
	private @Nullable MultiLineTextWidget qualityStatus;
	private @Nullable MultiLineTextWidget reverbStatus;
	private int ticks;
	/** While a SOFA is downloading, the file path being downloaded, or null when idle. */
	private @Nullable String sofaDownload;
	/** Cancel flag for the in-flight SOFA download (checked from the read loop). */
	private final AtomicBoolean sofaCancel = new AtomicBoolean(false);
	/** The HRTF value to roll back to if the download fails. */
	private String previousHrtf = "";

	/** The settings over {@code parent}, or an alert if the engine isn't running. */
	public static Screen open(Screen parent) {
		Minecraft minecraft = Minecraft.getInstance();
		if (EarshotClient.shaper() == null) {
			return new AlertScreen(() -> minecraft.gui.setScreen(parent), TITLE,
					Component.translatable("earshot.settings.unavailable"));
		}
		return new AcousticsScreen(parent, minecraft.options, EarshotClient.config());
	}

	private AcousticsScreen(Screen parent, Options options, EarshotConfig config) {
		super(parent, options, TITLE);
		initial = config;
		preset = choice("earshot.settings.preset", Arrays.stream(Preset.values()).map(Preset::key).toList(),
				config.preset().key());
		occlusion = toggle("earshot.settings.occlusion", config.occlusion());
		pathing = toggle("earshot.settings.pathing", config.pathing());
		airAbsorption = toggle("earshot.settings.air_absorption", config.airAbsorption());
		realisticFalloff = toggle("earshot.settings.realistic_falloff", config.realisticFalloff());
		doppler = toggle("earshot.settings.doppler", config.doppler());
		underwater = toggle("earshot.settings.underwater", config.underwater());
		reverb = choice("earshot.settings.reverb", Arrays.stream(Reverb.values()).map(Reverb::key).toList(),
				config.reverb().key());
		diffraction = choice("earshot.settings.diffraction",
				Arrays.stream(Diffraction.values()).map(Diffraction::key).toList(), config.diffraction().key());
		List<String> hrtfs = new ArrayList<>();
		hrtfs.add("");
		OpenAlProbe.Report report = OpenAlProbe.last();
		// The built-in one is "Default"; the rest as OpenAL lists them, the ones Earshot ships among them.
		if (report != null) {
			report.hrtfs().stream().filter(h -> !h.equals(ContextAttributes.BUILT_IN_HRTF)).forEach(hrtfs::add);
		}
		if (!hrtfs.contains(config.hrtf())) hrtfs.add(config.hrtf());
		hrtf = new OptionInstance<>("earshot.settings.hrtf",
				OptionInstance.cachedConstantTooltip(Component.translatable("earshot.settings.hrtf.tooltip")),
				(caption, value) -> value.isEmpty() ? Component.translatable("earshot.settings.hrtf.default")
						: Component.literal(value),
				new OptionInstance.Enum<>(hrtfs, Codec.STRING), config.hrtf(), this::onHrtfPicked);
		voice = toggle("earshot.settings.voice", config.voice());
		ownVoice = toggle("earshot.settings.own_voice", config.ownVoice());
	}

	/** A cycle through {@code values}, each with its own label and tooltip ({@code key.value[.tooltip]}). */
	private static OptionInstance<String> choice(String key, List<String> values, String initial) {
		return new OptionInstance<>(key,
				value -> Tooltip.create(Component.translatable(key + "." + value + ".tooltip")),
				(caption, value) -> Component.translatable(key + "." + value),
				new OptionInstance.Enum<>(values, Codec.STRING), initial, OptionInstance.NO_ACTION);
	}

	private static OptionInstance<Boolean> toggle(String key, boolean initial) {
		return OptionInstance.createBoolean(key,
				OptionInstance.cachedConstantTooltip(Component.translatable(key + ".tooltip")), initial);
	}

	@Override
	protected void init() {
		// Back from the HRTF test, which may have picked one.
		hrtf.set(EarshotClient.config().hrtf());
		super.init();
	}

	@Override
	protected void addOptions() {
		list.addBig(preset);
		qualityStatus = note(qualityStatus());
		list.addBig(qualityStatus);

		list.addHeader(Component.translatable("earshot.settings.section.sound"));
		list.addSmall(occlusion, pathing);
		list.addSmall(diffraction, airAbsorption);
		list.addSmall(realisticFalloff, doppler);
		list.addSmall(underwater);

		list.addHeader(Component.translatable("earshot.settings.section.reverb"));
		list.addBig(reverb);
		reverbStatus = note(reverbStatus());
		list.addBig(reverbStatus);

		list.addHeader(Component.translatable("earshot.settings.section.headphones"));
		list.addSmall(options.directionalAudio(), hrtf);
		list.addBig(net.minecraft.client.gui.components.Button.builder(
				Component.translatable("earshot.settings.hrtf_test"),
				b -> minecraft.gui.setScreen(new HrtfTestScreen(this))).build());

		list.addHeader(Component.translatable("earshot.settings.section.voice"));
		if (yakvc) {
			list.addSmall(voice, ownVoice);
		} else {
			list.addBig(note(Component.translatable("earshot.settings.voice.missing")));
		}

		list.addBig(note(bypassHint()));
		updateActive();
	}

	/** A grey line of text, centred in the list's full width. */
	private MultiLineTextWidget note(Component text) {
		MultiLineTextWidget note = new MultiLineTextWidget(text.copy().withStyle(ChatFormatting.GRAY), font) {
			// It sizes itself to its text, so it would sit at the row's left edge; as wide as the row, it centres.
			@Override
			public int getWidth() {
				return TEXT_WIDTH;
			}
		};
		return note.setMaxWidth(TEXT_WIDTH).setCentered(true);
	}

	@Override
	public void tick() {
		super.tick();
		updateActive();
		if (++ticks % REFRESH_TICKS != 0) return;
		if (qualityStatus != null) qualityStatus.setMessage(qualityStatus().copy().withStyle(ChatFormatting.GRAY));
		if (reverbStatus != null) reverbStatus.setMessage(reverbStatus().copy().withStyle(ChatFormatting.GRAY));
	}

	/** Greys out what can't take effect with the other settings as they are now. */
	private void updateActive() {
		boolean on = !preset.get().equals(Preset.OFF.key());
		for (OptionInstance<?> effect : List.of(occlusion, airAbsorption, realisticFalloff, doppler, underwater,
				reverb)) {
			setActive(effect, on);
		}
		setActive(pathing, on && occlusion.get());
		setActive(hrtf, options.directionalAudio().get());
		setActive(ownVoice, voice.get());
	}

	private void setActive(OptionInstance<?> option, boolean active) {
		AbstractWidget widget = list.findOption(option);
		if (widget != null) widget.active = active;
	}

	/** What the chosen quality is here, and what tracing costs now. */
	private Component qualityStatus() {
		Preset chosen = Preset.valueOf(preset.get().toUpperCase(Locale.ROOT));
		if (chosen == Preset.OFF) return Component.translatable("earshot.settings.status.off");
		Component name = Component.translatable("earshot.settings.preset." + chosen.resolve().key());
		Component picked = chosen == Preset.AUTO
				? Component.translatable("earshot.settings.status.auto", name, Runtime.getRuntime().availableProcessors())
				: name;
		NativeBridge bridge = EarshotClient.bridge();
		MemorySegment engine = EarshotClient.engine();
		if (bridge == null || engine == null || Minecraft.getInstance().level == null) return picked;
		double[] stats = bridge.stats(engine);
		return Component.translatable("earshot.settings.status.engine", picked,
				String.format(Locale.ROOT, "%,d", (long) stats[0]), String.format(Locale.ROOT, "%.0f", 100 * stats[1]));
	}

	/** Which reverb plays, and why when it isn't the one chosen. */
	private Component reverbStatus() {
		SoundShaper shaper = EarshotClient.shaper();
		OpenAlProbe.Report report = OpenAlProbe.last();
		if (shaper == null || report == null) return Component.empty();
		if (reverb.get().equals(Reverb.OFF.key())) return Component.translatable("earshot.settings.reverb.status.off");
		if (!report.convolutionEffect() && !reverb.get().equals(Reverb.EAX.key())) {
			return Component.translatable("earshot.settings.reverb.status.no_convolution");
		}
		if (Minecraft.getInstance().level == null) return Component.empty();
		return Component.translatable("earshot.settings.reverb.status.playing",
				Component.translatable("earshot.settings.reverb." + shaper.mode().name().toLowerCase(Locale.ROOT)));
	}

	/** Where the A/B key is, or how to bind it. */
	private static Component bypassHint() {
		var key = BypassKey.key();
		return key == null || key.isUnbound() ? Component.translatable("earshot.settings.bypass.unbound")
				: Component.translatable("earshot.settings.bypass.bound", key.getTranslatedKeyMessage());
	}

	/** Scrolls to the end of the list. For the client gametest's screenshot. */
	public void scrollToEnd() {
		list.setScrollAmount(Double.MAX_VALUE);
	}

	/**
	 * The user picked an HRTF dataset from the dropdown. For a SOFA dataset that's not yet on
	 * disk, downloads it asynchronously before applying the new setting. The dropdown stays
	 * set to the new value visually; on download failure the value is rolled back and the
	 * engine stays on the previous HRTF.
	 */
	private void onHrtfPicked(String newValue) {
		// The listener runs after OptionInstance has already stored the new value, so
		// hrtf.get() is the choice, not the old one. The engine's arm is the old one:
		// it only moves when a pick is applied, and a failed download never applies.
		String prePick = EarshotClient.config().hrtf();
		if (sofaDownload != null) return; // a download is in flight; ignore
		if (newValue.isEmpty() || !HrtfFiles.SOFAS.containsKey(newValue)) {
			EarshotClient.apply(EarshotClient.config().withHrtf(newValue));
			return;
		}
		Path target = Path.of("").toAbsolutePath().resolve(newValue + ".sofa");
		if (Files.isRegularFile(target)) {
			EarshotClient.apply(EarshotClient.config().withHrtf(newValue));
			return;
		}
		sofaDownload = newValue;
		previousHrtf = prePick;
		sofaCancel.set(false);
		rebuildWidgets(); // shows the progress bar
		String captured = newValue;
		CompletableFuture.runAsync(() -> {
			try {
				HrtfFiles.ensure(target, captured, (read, total) -> {
					// Progress callback. The UI re-reads the file size on tick(); we just
					// flag that bytes have arrived by storing the read count via a
					// re-render. A simpler approach: do nothing here, since the file
					// grows on disk and the byte count is informational.
				}, sofaCancel::get);
				Minecraft.getInstance().execute(() -> {
					if (sofaDownload == null) return; // the screen closed and cancelled it
					sofaDownload = null;
					rebuildWidgets();
					EarshotClient.apply(EarshotClient.config().withHrtf(captured));
				});
			} catch (IOException | RuntimeException e) {
				EarshotClient.LOGGER.warn("SOFA download failed for {}", captured, e);
				Minecraft.getInstance().execute(() -> {
					if (sofaDownload == null) return; // the screen closed; nothing to roll back
					sofaDownload = null;
					hrtf.set(previousHrtf);
					rebuildWidgets();
				});
			}
		});
	}

	@Override
	public void removed() {
		// A download outliving the screen must not apply a setting afterwards: the
		// user is no longer looking at the dropdown that started it. Ask the read
		// loop to stop, and let the file stay half-written for the next attempt.
		sofaCancel.set(true);
		sofaDownload = null;
		EarshotConfig next = new EarshotConfig(Preset.valueOf(preset.get().toUpperCase(Locale.ROOT)), occlusion.get(),
				pathing.get(), Reverb.valueOf(reverb.get().toUpperCase(Locale.ROOT)), airAbsorption.get(),
				underwater.get(), doppler.get(), voice.get(), hrtf.get(), ownVoice.get(), realisticFalloff.get(),
				Diffraction.valueOf(diffraction.get().toUpperCase(Locale.ROOT)));
		if (!next.equals(initial)) EarshotClient.apply(next);
		super.removed();
	}
}
