package io.github.cumsauce.earshot.mixin;

import com.mojang.blaze3d.audio.Channel;
import io.github.cumsauce.earshot.audio.ShapedChannel;
import io.github.cumsauce.earshot.audio.SoundShaper;
import io.github.cumsauce.earshot.audio.SourceState;
import net.minecraft.world.phys.Vec3;
import org.jspecify.annotations.Nullable;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.Unique;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Remembers what the game tells each source, so {@link SoundShaper} can shape it. */
@Mixin(Channel.class)
abstract class ChannelMixin implements ShapedChannel {
	@Shadow
	@Final
	private int source;

	@Unique
	private @Nullable Vec3 earshot$position;
	@Unique
	private boolean earshot$relative;
	@Unique
	private boolean earshot$attenuated = true;
	@Unique
	private float earshot$range = 16;
	@Unique
	private final SourceState earshot$state = new SourceState();

	@Inject(method = "setSelfPosition", at = @At("HEAD"))
	private void earshot$rememberPosition(Vec3 position, CallbackInfo ci) {
		earshot$position = position;
	}

	@Inject(method = "setSelfPosition", at = @At("TAIL"))
	private void earshot$keepHeardPosition(Vec3 position, CallbackInfo ci) {
		SoundShaper.reapplyPosition(this);
	}

	@Inject(method = "setRelative", at = @At("HEAD"))
	private void earshot$rememberRelative(boolean relative, CallbackInfo ci) {
		earshot$relative = relative;
	}

	@Inject(method = "disableAttenuation", at = @At("HEAD"))
	private void earshot$rememberNoAttenuation(CallbackInfo ci) {
		earshot$attenuated = false;
	}

	@Inject(method = "linearAttenuation", at = @At("HEAD"))
	private void earshot$rememberAttenuation(float distance, CallbackInfo ci) {
		earshot$attenuated = true;
		earshot$range = distance;
	}

	@Inject(method = "setVolume", at = @At("HEAD"))
	private void earshot$rememberVolume(float volume, CallbackInfo ci) {
		earshot$state.volume(volume);
	}

	@Inject(method = "destroy", at = @At("HEAD"))
	private void earshot$release(CallbackInfo ci) {
		SoundShaper.release(this);
	}

	@Override
	public int earshot$source() {
		return source;
	}

	@Override
	public @Nullable Vec3 earshot$position() {
		return earshot$position;
	}

	@Override
	public boolean earshot$positional() {
		return !earshot$relative;
	}

	@Override
	public float earshot$range() {
		return earshot$attenuated ? earshot$range : 0;
	}

	@Override
	public SourceState earshot$state() {
		return earshot$state;
	}
}
