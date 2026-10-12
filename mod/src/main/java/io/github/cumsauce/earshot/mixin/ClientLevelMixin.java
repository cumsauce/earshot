package io.github.cumsauce.earshot.mixin;

import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.world.WorldMirror;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.world.level.block.state.BlockState;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Passes every block change the client sees on to the world mirror. */
@Mixin(ClientLevel.class)
abstract class ClientLevelMixin {
	@Inject(method = "sendBlockUpdated", at = @At("HEAD"))
	private void earshot$blockChanged(BlockPos pos, BlockState oldState, BlockState newState, int flags,
			CallbackInfo ci) {
		WorldMirror mirror = EarshotClient.mirror();
		if (mirror != null) mirror.blockChanged((ClientLevel) (Object) this, pos, newState);
	}
}
