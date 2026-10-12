package io.github.cumsauce.earshot.ui;

import com.mojang.blaze3d.platform.InputConstants;
import io.github.cumsauce.earshot.EarshotClient;
import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientTickEvents;
import net.fabricmc.fabric.api.client.keymapping.v1.KeyMappingHelper;
import net.minecraft.client.KeyMapping;
import net.minecraft.network.chat.Component;
import net.minecraft.resources.Identifier;

/**
 * A key, unbound until the player binds it, that switches Earshot off and on again at once, for listening to the
 * difference. The action bar says which it is.
 */
public final class BypassKey {
	private static KeyMapping key;
	private static KeyMapping overlayKey;

	private BypassKey() {}

	/** The key, once registered. */
	public static KeyMapping key() {
		return key;
	}

	public static void register() {
		KeyMapping.Category category = KeyMapping.Category.register(Identifier.fromNamespaceAndPath("earshot", "keys"));
		key = KeyMappingHelper.registerKeyMapping(
				new KeyMapping("key.earshot.bypass", InputConstants.UNKNOWN.getValue(), category));
		overlayKey = KeyMappingHelper.registerKeyMapping(
				new KeyMapping("key.earshot.debug_overlay", InputConstants.UNKNOWN.getValue(), category));
		ClientTickEvents.END_CLIENT_TICK.register(minecraft -> {
			while (overlayKey.consumeClick()) DebugOverlay.toggle();
			boolean pressed = false;
			while (key.consumeClick()) pressed = !pressed;
			if (!pressed || EarshotClient.shaper() == null) return;
			EarshotClient.setBypassed(!EarshotClient.bypassed());
			if (minecraft.player != null) {
				minecraft.player.sendOverlayMessage(Component.translatable(
						EarshotClient.bypassed() ? "earshot.bypass.on" : "earshot.bypass.off"));
			}
		});
	}
}
