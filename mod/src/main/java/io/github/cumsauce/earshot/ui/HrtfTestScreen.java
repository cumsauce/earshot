package io.github.cumsauce.earshot.ui;

import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.openal.ContextAttributes;
import io.github.cumsauce.earshot.openal.HrtfFiles;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import java.util.ArrayList;
import java.util.List;
import net.minecraft.ChatFormatting;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.components.Button;
import net.minecraft.client.gui.components.MultiLineTextWidget;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.gui.screens.options.OptionsSubScreen;
import net.minecraft.client.resources.sounds.SimpleSoundInstance;
import net.minecraft.client.resources.sounds.SoundInstance;
import net.minecraft.network.chat.Component;
import net.minecraft.sounds.SoundEvents;
import net.minecraft.sounds.SoundSource;
import org.jspecify.annotations.Nullable;

/**
 * A listening test for picking an HRTF: for each one, a snare circles the player's head (ahead, right, behind, left,
 * above, below), and they keep the one that places it best. HRTFs fit some ears better than others, and listening is
 * the only way to tell. Directional Audio is on for the test, and back as it was if no HRTF is kept.
 */
public final class HrtfTestScreen extends OptionsSubScreen {
	/** Where the snare plays, relative to the head (x right, y up, -z ahead), in order. */
	private static final double[][] AROUND = {{0, 0, -2}, {2, 0, 0}, {0, 0, 2}, {-2, 0, 0}, {0, 2, 0}, {0, -2, 0}};
	/** Ticks between hits: long enough to tell each place apart. */
	private static final int SPACING = 12;

	private final boolean directionalBefore;
	private final String hrtfBefore;
	private final List<String> hrtfs = new ArrayList<>();
	private @Nullable MultiLineTextWidget status;
	/** The HRTF waiting to be played once the sound engine has it, or null. */
	private @Nullable String pending;
	private boolean kept;
	private int played;

	public HrtfTestScreen(Screen parent) {
		super(parent, Minecraft.getInstance().options, Component.translatable("earshot.hrtf_test.title"));
		directionalBefore = options.directionalAudio().get();
		hrtfBefore = EarshotClient.config().hrtf();
		hrtfs.add("");
		OpenAlProbe.Report report = OpenAlProbe.last();
		if (report != null) {
			report.hrtfs().stream().filter(h -> !h.equals(ContextAttributes.BUILT_IN_HRTF)).forEach(hrtfs::add);
		}
		// The 256-tap SOFAs, which OpenAL only lists once on disk; playing one before that falls back to its
		// bundled 64-tap MHR (tick() counts that as the set wanted), so the test needs their bare names too.
		for (String name : HrtfFiles.NAMES) {
			if (!hrtfs.contains(name)) hrtfs.add(name);
		}
	}

	@Override
	protected void addOptions() {
		for (int line = 1; line <= 3; line++) list.addBig(note(Component.translatable("earshot.hrtf_test.how." + line)));
		for (String hrtf : hrtfs) {
			Component name = name(hrtf);
			list.addSmall(
					Button.builder(Component.translatable("earshot.hrtf_test.play", name), b -> play(hrtf)).build(),
					Button.builder(Component.translatable("earshot.hrtf_test.keep", name), b -> keep(hrtf)).build());
		}
		status = note(Component.empty());
		list.addBig(status);
		if (!options.directionalAudio().get()) {
			// The test needs HRTF on; this restarts the sound engine.
			options.directionalAudio().set(true);
		}
	}

	private MultiLineTextWidget note(Component text) {
		MultiLineTextWidget note = new MultiLineTextWidget(text.copy().withStyle(ChatFormatting.GRAY), font) {
			@Override
			public int getWidth() {
				return 310;
			}
		};
		return note.setMaxWidth(310).setCentered(true);
	}

	private static Component name(String hrtf) {
		return hrtf.isEmpty() ? Component.translatable("earshot.settings.hrtf.default") : Component.literal(hrtf);
	}

	/** Switches to {@code hrtf} if it isn't loaded (restarting the sound engine), then plays the circle. */
	public void play(String hrtf) {
		pending = hrtf;
		if (!hrtf.equals(EarshotClient.config().hrtf())) EarshotClient.apply(EarshotClient.config().withHrtf(hrtf));
		setStatus(Component.translatable("earshot.hrtf_test.loading", name(hrtf)));
	}

	/** Keeps {@code hrtf}: it stays picked, with Directional Audio on. */
	public void keep(String hrtf) {
		kept = true;
		if (!hrtf.equals(EarshotClient.config().hrtf())) EarshotClient.apply(EarshotClient.config().withHrtf(hrtf));
		setStatus(Component.translatable("earshot.hrtf_test.kept", name(hrtf)));
	}

	/** How many circles have played. For the client gametest. */
	public int played() {
		return played;
	}

	@Override
	public void tick() {
		super.tick();
		if (pending == null) return;
		OpenAlProbe.Report report = OpenAlProbe.last();
		String wanted = pending.isEmpty() ? ContextAttributes.BUILT_IN_HRTF : pending;
		// A bare SADIE II name falls back to the bundled 64-tap MHR when its SOFA isn't on disk yet; that
		// is the set loaded, so it counts as the one wanted.
		if (report == null
				|| !(report.hrtf().equals(wanted) || report.hrtf().equals(wanted + HrtfFiles.MHR_SUFFIX)))
			return;
		var sounds = Minecraft.getInstance().getSoundManager();
		for (int i = 0; i < AROUND.length; i++) {
			double[] at = AROUND[i];
			sounds.playDelayed(new SimpleSoundInstance(SoundEvents.NOTE_BLOCK_SNARE.value().location(), SoundSource.MASTER,
					1f, 1f, SoundInstance.createUnseededRandom(), false, 0, SoundInstance.Attenuation.NONE, at[0], at[1],
					at[2], true), 1 + i * SPACING);
		}
		setStatus(Component.translatable("earshot.hrtf_test.playing", name(pending)));
		pending = null;
		played++;
	}

	private void setStatus(Component text) {
		if (status != null) status.setMessage(text.copy().withStyle(ChatFormatting.GRAY));
	}

	@Override
	public void removed() {
		if (!kept) {
			// Nothing kept: back as it was.
			if (!hrtfBefore.equals(EarshotClient.config().hrtf())) {
				EarshotClient.apply(EarshotClient.config().withHrtf(hrtfBefore));
			}
			if (options.directionalAudio().get() != directionalBefore) options.directionalAudio().set(directionalBefore);
		}
		super.removed();
	}
}
