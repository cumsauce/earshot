package io.github.cumsauce.earshot.mixin;

import com.mojang.blaze3d.audio.Library;
import io.github.cumsauce.earshot.openal.ContextAttributes;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import java.nio.IntBuffer;
import org.lwjgl.system.MemoryStack;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

/** Asks for the auxiliary sends reverb needs when the game opens its context, then probes what it got. */
@Mixin(Library.class)
abstract class LibraryMixin {
	@Inject(method = "createAttributes", at = @At("RETURN"), cancellable = true)
	private void earshot$moreSends(MemoryStack stack, boolean hrtf, CallbackInfoReturnable<IntBuffer> cir) {
		cir.setReturnValue(ContextAttributes.withAuxiliarySends(stack, cir.getReturnValue()));
	}

	@Inject(method = "init", at = @At("TAIL"))
	private void earshot$probe(CallbackInfo ci) {
		OpenAlProbe.run("Library.init");
	}
}
