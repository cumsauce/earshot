package io.github.cumsauce.earshot.ui;

import com.mojang.serialization.Codec;
import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.config.EarshotConfig;
import io.github.cumsauce.earshot.config.EarshotConfig.Preset;
import io.github.cumsauce.earshot.config.EarshotConfig.Reverb;
import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import java.lang.foreign.MemorySegment;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;
import net.minecraft.client.Minecraft;
import net.minecraft.client.OptionInstance;
import net.minecraft.client.Options;
import net.minecraft.client.gui.components.MultiLineTextWidget;
import net.minecraft.client.gui.components.Tooltip;
import net.minecraft.client.gui.screens.AlertScreen;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.gui.screens.options.OptionsSubScreen;
import net.minecraft.network.chat.Component;

/** Earshot's settings, under Options → Music &amp; Sounds. They are applied and saved when the screen closes. */
public final class AcousticsScreen extends OptionsSubScreen {
	private static final Component TITLE = Component.translatable("earshot.settings.title");

	private final EarshotConfig initial;
	private final OptionInstance<String> preset;
	private final OptionInstance<Boolean> occlusion;
	private final OptionInstance<Boolean> pathing;
	private final OptionInstance<String> reverb;
	private final OptionInstance<Boolean> airAbsorption;
	private final OptionInstance<Boolean> underwater;
	private final OptionInstance<Boolean> doppler;
	private final OptionInstance<Boolean> voice;
	private final OptionInstance<Boolean> ownVoice;
	private final OptionInstance<String> hrtf;

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
		reverb = choice("earshot.settings.reverb", Arrays.stream(Reverb.values()).map(Reverb::key).toList(),
				config.reverb().key());
		airAbsorption = toggle("earshot.settings.air_absorption", config.airAbsorption());
		underwater = toggle("earshot.settings.underwater", config.underwater());
		doppler = toggle("earshot.settings.doppler", config.doppler());
		voice = toggle("earshot.settings.voice", config.voice());
		ownVoice = toggle("earshot.settings.own_voice", config.ownVoice());
		List<String> hrtfs = new ArrayList<>();
		hrtfs.add("");
		OpenAlProbe.Report report = OpenAlProbe.last();
		if (report != null) hrtfs.addAll(report.hrtfs());
		if (!hrtfs.contains(config.hrtf())) hrtfs.add(config.hrtf());
		hrtf = new OptionInstance<>("earshot.settings.hrtf",
				OptionInstance.cachedConstantTooltip(Component.translatable("earshot.settings.hrtf.tooltip")),
				(caption, value) -> value.isEmpty() ? Component.translatable("earshot.settings.hrtf.default")
						: Component.literal(value),
				new OptionInstance.Enum<>(hrtfs, Codec.STRING), config.hrtf(), OptionInstance.NO_ACTION);
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
		list.addBig(new MultiLineTextWidget(status(), font).setMaxWidth(310).setCentered(true));
		list.addSmall(occlusion, pathing);
		list.addSmall(reverb, airAbsorption);
		list.addSmall(underwater, doppler);
		list.addSmall(voice, ownVoice);
		list.addBig(hrtf);
	}

	@Override
	public void removed() {
		EarshotConfig next = new EarshotConfig(Preset.valueOf(preset.get().toUpperCase(Locale.ROOT)), occlusion.get(),
				pathing.get(), Reverb.valueOf(reverb.get().toUpperCase(Locale.ROOT)), airAbsorption.get(),
				underwater.get(), doppler.get(), voice.get(), hrtf.get(), ownVoice.get());
		if (!next.equals(initial)) EarshotClient.apply(next);
		super.removed();
	}

	/** What Auto picks here, and what the engine is spending now. */
	private Component status() {
		Component picked = Component.translatable("earshot.settings.status.auto",
				Component.translatable("earshot.settings.preset." + Preset.AUTO.resolve().key()),
				Runtime.getRuntime().availableProcessors());
		NativeBridge bridge = EarshotClient.bridge();
		MemorySegment engine = EarshotClient.engine();
		if (bridge == null || engine == null) return picked;
		double[] stats = bridge.stats(engine);
		return picked.copy().append("\n").append(Component.translatable("earshot.settings.status.engine",
				(long) stats[0], String.format(Locale.ROOT, "%.0f", 100 * stats[1]), (long) stats[3]));
	}
}
