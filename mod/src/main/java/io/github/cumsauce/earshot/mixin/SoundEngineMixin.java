package io.github.cumsauce.earshot.mixin;

import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.audio.SoundShaper;
import net.minecraft.client.sounds.ChannelAccess;
import net.minecraft.client.sounds.SoundEngine;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Shapes the playing sounds once a tick, after the game has updated them. */
@Mixin(SoundEngine.class)
abstract class SoundEngineMixin {
	@Shadow
	@Final
	private ChannelAccess channelAccess;

	@Shadow
	private boolean loaded;

	@Inject(method = "tick", at = @At("TAIL"))
	private void earshot$shape(boolean paused, CallbackInfo ci) {
		SoundShaper shaper = EarshotClient.shaper();
		if (loaded && !paused && shaper != null) shaper.tick(channelAccess);
	}
}
