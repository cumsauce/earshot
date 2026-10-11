package io.github.cumsauce.earshot.mixin;

import com.mojang.blaze3d.audio.Library;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Probes OpenAL right after Minecraft opens its sound device and context, on the thread that opened them. */
@Mixin(Library.class)
abstract class LibraryMixin {
	@Inject(method = "init", at = @At("TAIL"))
	private void earshot$probe(CallbackInfo ci) {
		OpenAlProbe.run("Library.init");
	}
}
