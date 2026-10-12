package io.github.cumsauce.earshot.ui;

import java.util.List;
import java.util.Optional;
import net.fabricmc.fabric.api.client.screen.v1.ScreenEvents;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.components.AbstractButton;
import net.minecraft.client.gui.components.Button;
import net.minecraft.client.gui.components.OptionsList;
import net.minecraft.client.gui.components.events.ContainerEventHandler;
import net.minecraft.client.gui.components.events.GuiEventListener;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.gui.screens.options.SoundOptionsScreen;
import net.minecraft.network.chat.Component;

/**
 * An "Acoustics…" button at the end of the game's Music &amp; Sounds options, added the way Yak VC adds its own: into
 * the screen's options list, so it scrolls with the other options and never overlaps them.
 */
public final class SoundOptionsButton {
	private static final Component LABEL = Component.translatable("earshot.sound_options.acoustics");

	private SoundOptionsButton() {}

	public static void register() {
		// This runs whenever the screen is initialised: when it opens, on resize, and when a screen on top of it
		// closes. The options list can survive that, so the button is added only once.
		ScreenEvents.AFTER_INIT.register((minecraft, screen, width, height) -> {
			if (!(screen instanceof SoundOptionsScreen)) return;
			for (GuiEventListener widget : screen.children()) {
				if (widget instanceof OptionsList list && find(list.children()).isEmpty()) {
					list.addBig(Button.builder(LABEL,
							button -> Minecraft.getInstance().gui.setScreen(AcousticsScreen.open(screen))).build());
				}
			}
		});
	}

	/** The Acoustics button on {@code screen}, if it has one. For the client gametest. */
	public static Optional<AbstractButton> find(Screen screen) {
		return find(screen.children());
	}

	private static Optional<AbstractButton> find(List<? extends GuiEventListener> widgets) {
		for (GuiEventListener widget : widgets) {
			if (widget instanceof AbstractButton button && button.getMessage().equals(LABEL)) return Optional.of(button);
			if (widget instanceof ContainerEventHandler container) {
				Optional<AbstractButton> inside = find(container.children());
				if (inside.isPresent()) return inside;
			}
		}
		return Optional.empty();
	}
}
