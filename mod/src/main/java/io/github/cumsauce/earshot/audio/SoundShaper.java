package io.github.cumsauce.earshot.audio;

import com.mojang.blaze3d.audio.Channel;
import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.natives.EarshotException;
import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import java.lang.foreign.MemorySegment;
import java.util.ArrayList;
import java.util.List;
import java.util.stream.Stream;
import net.minecraft.client.Camera;
import net.minecraft.client.Minecraft;
import net.minecraft.client.sounds.ChannelAccess;
import net.minecraft.world.level.material.FogType;
import net.minecraft.world.phys.Vec3;
import org.lwjgl.openal.AL10;
import org.lwjgl.openal.EXTEfx;

/**
 * Shapes every positional sound the game plays by how it reaches the listener (M2): muffled by what is in the way,
 * moved to the opening it comes round through, damped by the air over distance, and muffled hard underwater.
 *
 * <p>Once a tick, after the sound engine's own tick, {@link #tick} tells the native engine where the listener is and
 * queues {@link #update} on the sound thread, which asks the native engine how each playing sound is heard (one call
 * for all of them) and sets each OpenAL source's direct filter and position.
 */
public final class SoundShaper {
	/** OpenAL's air absorption: 1 is about 0.05 dB lost per metre at 5 kHz. */
	private static final float AIR_ABSORPTION = 1.0f;
	/** Underwater, everything sounds like this, low to high. */
	private static final float[] UNDERWATER = {0.7f, 0.2f, 0.05f};

	private final NativeBridge bridge;
	private final MemorySegment engine;
	private volatile List<Snapshot> snapshot = List.of();
	private volatile boolean failed;

	/** One shaped sound, for the debug command and tests. */
	public record Snapshot(Vec3 position, Vec3 heardFrom, float[] gains) {
		@Override
		public String toString() {
			return "Snapshot[position=" + position + ", heardFrom=" + heardFrom + ", gains="
					+ java.util.Arrays.toString(gains) + "]";
		}
	}

	public SoundShaper(NativeBridge bridge, MemorySegment engine) {
		this.bridge = bridge;
		this.engine = engine;
	}

	/** The sounds shaped in the last update. */
	public List<Snapshot> snapshot() {
		return snapshot;
	}

	/** Called on the client thread at the end of the sound engine's tick. */
	public void tick(ChannelAccess channels) {
		if (failed) return;
		Minecraft minecraft = Minecraft.getInstance();
		Camera camera = minecraft.gameRenderer.mainCamera();
		if (minecraft.level == null || !camera.isInitialized()) return;
		Vec3 eye = camera.position();
		double[] listener = {eye.x, eye.y, eye.z};
		boolean underwater = camera.getFluidInCamera() == FogType.WATER;
		OpenAlProbe.Report report = OpenAlProbe.last();
		boolean efx = report != null && report.efx();
		if (!call(() -> bridge.setListener(engine, listener))) return;
		channels.executeOnChannels(stream -> update(stream, listener, underwater, efx));
	}

	/** On the sound thread: shapes every positional source. */
	private void update(Stream<Channel> channels, double[] listener, boolean underwater, boolean efx) {
		List<ShapedChannel> shaped = new ArrayList<>();
		channels.forEach(channel -> {
			ShapedChannel c = (ShapedChannel) channel;
			if (c.earshot$positional() && c.earshot$position() != null) shaped.add(c);
		});
		int count = shaped.size();
		if (count == 0) {
			snapshot = List.of();
			return;
		}
		double[] sources = new double[3 * count];
		for (int i = 0; i < count; i++) {
			Vec3 position = shaped.get(i).earshot$position();
			sources[3 * i] = position.x;
			sources[3 * i + 1] = position.y;
			sources[3 * i + 2] = position.z;
		}
		double[] heard = new double[NativeBridge.HEARD_DOUBLES * count];
		if (!call(() -> bridge.hear(engine, listener, sources, count, heard))) return;

		List<Snapshot> shots = new ArrayList<>(count);
		for (int i = 0; i < count; i++) {
			ShapedChannel channel = shaped.get(i);
			SourceState state = channel.earshot$state();
			int at = NativeBridge.HEARD_DOUBLES * i;
			Vec3 position = channel.earshot$position();
			state.approach(new float[] {(float) heard[at], (float) heard[at + 1], (float) heard[at + 2]},
					new Vec3(heard[at + 3], heard[at + 4], heard[at + 5]), position);
			float[] gains = state.gains();
			if (underwater) {
				for (int band = 0; band < gains.length; band++) gains[band] *= UNDERWATER[band];
			}
			if (efx) filter(channel, state, gains);
			Vec3 heardFrom = state.heardFrom(position);
			AL10.alSource3f(channel.earshot$source(), AL10.AL_POSITION, (float) heardFrom.x, (float) heardFrom.y,
					(float) heardFrom.z);
			shots.add(new Snapshot(position, heardFrom, gains));
		}
		snapshot = List.copyOf(shots);
	}

	/**
	 * Sets the source's direct filter: a low-pass whose gain is the low band and whose high-frequency gain brings the
	 * high band down to its own value. OpenAL copies a filter when it is attached, so it is attached again each time.
	 */
	private static void filter(ShapedChannel channel, SourceState state, float[] gains) {
		int source = channel.earshot$source();
		if (state.filter == 0) {
			state.filter = EXTEfx.alGenFilters();
			EXTEfx.alFilteri(state.filter, EXTEfx.AL_FILTER_TYPE, EXTEfx.AL_FILTER_LOWPASS);
			AL10.alSourcef(source, EXTEfx.AL_AIR_ABSORPTION_FACTOR, AIR_ABSORPTION);
		}
		float low = Math.clamp(gains[0], 0f, 1f);
		float highRatio = low > 0 ? Math.clamp(gains[2] / low, 0f, 1f) : 0f;
		EXTEfx.alFilterf(state.filter, EXTEfx.AL_LOWPASS_GAIN, low);
		EXTEfx.alFilterf(state.filter, EXTEfx.AL_LOWPASS_GAINHF, highRatio);
		AL10.alSourcei(source, EXTEfx.AL_DIRECT_FILTER, state.filter);
	}

	/**
	 * Puts a source back where it is heard from after the game moved it, so a sound coming round a corner doesn't
	 * flick back to where it really is until the next update. Called on the sound thread.
	 */
	public static void reapplyPosition(ShapedChannel channel) {
		SourceState state = channel.earshot$state();
		Vec3 position = channel.earshot$position();
		if (!state.started() || position == null || !channel.earshot$positional()) return;
		Vec3 heardFrom = state.heardFrom(position);
		AL10.alSource3f(channel.earshot$source(), AL10.AL_POSITION, (float) heardFrom.x, (float) heardFrom.y,
				(float) heardFrom.z);
	}

	/** Frees a source's filter when the game destroys the source. Called on the sound thread. */
	public static void release(ShapedChannel channel) {
		SourceState state = channel.earshot$state();
		if (state.filter != 0) {
			EXTEfx.alDeleteFilters(state.filter);
			state.filter = 0;
		}
	}

	private boolean call(Runnable call) {
		try {
			call.run();
			return true;
		} catch (EarshotException e) {
			failed = true;
			EarshotClient.LOGGER.error("Sound shaping stopped: a native call failed", e);
			return false;
		}
	}
}
