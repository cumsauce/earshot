package io.github.cumsauce.earshot.mixin;

import com.mojang.blaze3d.audio.Library;
import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.audio.SoundShaper;
import io.github.cumsauce.earshot.openal.ContextAttributes;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import java.nio.IntBuffer;
import org.lwjgl.system.MemoryStack;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

/**
 * Asks for the auxiliary sends reverb needs and sources for voices when the game opens its context, probes what it
 * got, and has Earshot forget its reverb and voices when the context goes.
 */
@Mixin(Library.class)
abstract class LibraryMixin {
	@Inject(method = "createAttributes", at = @At("RETURN"), cancellable = true)
	private void earshot$attributes(MemoryStack stack, boolean hrtf, CallbackInfoReturnable<IntBuffer> cir) {
		cir.setReturnValue(ContextAttributes.withEarshotAttributes(stack, cir.getReturnValue()));
	}

	@Inject(method = "cleanup", at = @At("HEAD"))
	private void earshot$forgetContext(CallbackInfo ci) {
		SoundShaper shaper = EarshotClient.shaper();
		if (shaper != null) shaper.contextClosing();
	}

	@Inject(method = "init", at = @At("TAIL"))
	private void earshot$probe(CallbackInfo ci) {
		OpenAlProbe.run("Library.init");
		SoundShaper shaper = EarshotClient.shaper();
		if (shaper != null) shaper.voice().openAlOpened(OpenAlProbe.last());
	}
}
