package io.github.cumsauce.earshot.mixin;

import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.audio.SoundShaper;
import io.github.cumsauce.earshot.audio.Thunder;
import net.minecraft.client.Camera;
import net.minecraft.client.Minecraft;
import net.minecraft.client.resources.sounds.SoundInstance;
import net.minecraft.client.sounds.ChannelAccess;
import net.minecraft.client.sounds.SoundEngine;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

/** Shapes the playing sounds once a tick, after the game has updated them, and holds thunder back by distance. */
@Mixin(SoundEngine.class)
abstract class SoundEngineMixin {
	@Shadow
	@Final
	private ChannelAccess channelAccess;

	@Shadow
	private boolean loaded;

	@Shadow
	public abstract void playDelayed(SoundInstance sound, int delay);

	@Inject(method = "play", at = @At("HEAD"), cancellable = true)
	private void earshot$thunderLater(SoundInstance sound, CallbackInfoReturnable<SoundEngine.PlayResult> cir) {
		if (!EarshotClient.shaping() || !Thunder.isThunder(sound)) return;
		Camera camera = Minecraft.getInstance().gameRenderer.mainCamera();
		if (!camera.isInitialized()) return;
		int ticks = Thunder.holdTicks(sound, camera.position());
		if (ticks <= 0) return;
		playDelayed(sound, ticks);
		cir.setReturnValue(SoundEngine.PlayResult.NOT_STARTED);
	}

	@Inject(method = "tick", at = @At("TAIL"))
	private void earshot$shape(boolean paused, CallbackInfo ci) {
		SoundShaper shaper = EarshotClient.shaper();
		if (loaded && !paused && shaper != null) shaper.tick(channelAccess);
	}
}
