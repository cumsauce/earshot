package io.github.cumsauce.earshot.ui;

import com.mojang.serialization.Codec;
import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.audio.SoundShaper;
import io.github.cumsauce.earshot.config.EarshotConfig;
import io.github.cumsauce.earshot.config.EarshotConfig.Preset;
import io.github.cumsauce.earshot.config.EarshotConfig.Reverb;
import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.openal.ContextAttributes;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import java.lang.foreign.MemorySegment;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;
import net.fabricmc.loader.api.FabricLoader;
import net.minecraft.ChatFormatting;
import net.minecraft.client.Minecraft;
import net.minecraft.client.OptionInstance;
import net.minecraft.client.Options;
import net.minecraft.client.gui.components.AbstractWidget;
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
	private final OptionInstance<String> hrtf;
	private final OptionInstance<Boolean> voice;
	private final OptionInstance<Boolean> ownVoice;
	private final boolean yakvc = FabricLoader.getInstance().isModLoaded("yakvc");
	private @Nullable MultiLineTextWidget qualityStatus;
	private @Nullable MultiLineTextWidget reverbStatus;
	private int ticks;

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
				new OptionInstance.Enum<>(hrtfs, Codec.STRING), config.hrtf(), OptionInstance.NO_ACTION);
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
	protected void addOptions() {
		list.addBig(preset);
		qualityStatus = note(qualityStatus());
		list.addBig(qualityStatus);

		list.addHeader(Component.translatable("earshot.settings.section.sound"));
		list.addSmall(occlusion, pathing);
		list.addSmall(airAbsorption, realisticFalloff);
		list.addSmall(doppler, underwater);

		list.addHeader(Component.translatable("earshot.settings.section.reverb"));
		list.addBig(reverb);
		reverbStatus = note(reverbStatus());
		list.addBig(reverbStatus);

		list.addHeader(Component.translatable("earshot.settings.section.headphones"));
		list.addSmall(options.directionalAudio(), hrtf);

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

	@Override
	public void removed() {
		EarshotConfig next = new EarshotConfig(Preset.valueOf(preset.get().toUpperCase(Locale.ROOT)), occlusion.get(),
				pathing.get(), Reverb.valueOf(reverb.get().toUpperCase(Locale.ROOT)), airAbsorption.get(),
				underwater.get(), doppler.get(), voice.get(), hrtf.get(), ownVoice.get(), realisticFalloff.get());
		if (!next.equals(initial)) EarshotClient.apply(next);
		super.removed();
	}
}
