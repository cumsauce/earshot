package io.github.cumsauce.earshot.ui;

import com.terraformersmc.modmenu.api.ConfigScreenFactory;
import com.terraformersmc.modmenu.api.ModMenuApi;

/**
 * Mod Menu's config button for Earshot, opening the same settings as Music &amp; Sounds → Acoustics…. Fabric only
 * loads this class through the {@code modmenu} entrypoint, so Earshot runs fine without Mod Menu installed.
 */
public final class ModMenuEntry implements ModMenuApi {
	@Override
	public ConfigScreenFactory<?> getModConfigScreenFactory() {
		return AcousticsScreen::open;
	}
}
