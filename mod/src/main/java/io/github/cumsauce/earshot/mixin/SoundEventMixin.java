package io.github.cumsauce.earshot.mixin;

import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.audio.SoundShaper;
import net.minecraft.sounds.SoundEvent;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

/**
 * With realistic falloff, a world this game hosts (singleplayer, or opened to LAN) sends its sounds
 * {@value SoundShaper#REACH} times as far as usual, so far ones carry, quietly, as real ones do. Sounds that already
 * carry far (thunder, a wither) are left as they are. Another server sends what it sends.
 */
@Mixin(SoundEvent.class)
abstract class SoundEventMixin {
	@Inject(method = "getRange", at = @At("RETURN"), cancellable = true)
	private void earshot$carry(float volume, CallbackInfoReturnable<Float> cir) {
		float range = cir.getReturnValueF();
		if (range <= 64 && EarshotClient.shaping() && EarshotClient.config().realisticFalloff()) {
			cir.setReturnValue(range * SoundShaper.REACH);
		}
	}
}
