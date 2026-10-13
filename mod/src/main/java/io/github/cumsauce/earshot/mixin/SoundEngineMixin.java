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

/**
 * Shapes the playing sounds once a tick, after the game has updated them, and holds sounds back by how long they take
 * to arrive: thunder always, other one-shots with realistic distance.
 */
@Mixin(SoundEngine.class)
abstract class SoundEngineMixin {
	@Shadow
	@Final
	private ChannelAccess channelAccess;

	@Shadow
	private boolean loaded;


	@Inject(method = "play", at = @At("HEAD"), cancellable = true)
	private void earshot$thunderLater(SoundInstance sound, CallbackInfoReturnable<SoundEngine.PlayResult> cir) {
		if (!EarshotClient.shaping()) return;
		boolean thunder = Thunder.isThunder(sound);
		if (!thunder && !EarshotClient.config().realisticFalloff()) return;
		Camera camera = Minecraft.getInstance().gameRenderer.mainCamera();
		if (!camera.isInitialized()) return;
		int ticks = thunder ? Thunder.holdTicks(sound, camera.position())
				: io.github.cumsauce.earshot.audio.Propagation.holdTicks(sound, camera.position());
		if (ticks <= 0) return;
		io.github.cumsauce.earshot.audio.Propagation.hold(sound, ticks);
		cir.setReturnValue(SoundEngine.PlayResult.NOT_STARTED);
	}

	/** Lets out the held sounds now due, before the game's own tick walks its queues. */
	@Inject(method = "tick", at = @At("HEAD"))
	private void earshot$letOut(boolean paused, CallbackInfo ci) {
		if (!loaded || paused) return;
		for (SoundInstance sound : io.github.cumsauce.earshot.audio.Propagation.due()) {
			((SoundEngine) (Object) this).play(sound);
		}
	}

	@Inject(method = "stop(Lnet/minecraft/client/resources/sounds/SoundInstance;)V", at = @At("HEAD"))
	private void earshot$cancel(SoundInstance sound, CallbackInfo ci) {
		io.github.cumsauce.earshot.audio.Propagation.cancel(sound);
	}

	@Inject(method = {"stopAll", "destroy"}, at = @At("HEAD"))
	private void earshot$forgetHeld(CallbackInfo ci) {
		io.github.cumsauce.earshot.audio.Propagation.clear();
	}

	@Inject(method = "tick", at = @At("TAIL"))
	private void earshot$shape(boolean paused, CallbackInfo ci) {
		SoundShaper shaper = EarshotClient.shaper();
		if (loaded && !paused && shaper != null) shaper.tick(channelAccess);
	}
}
