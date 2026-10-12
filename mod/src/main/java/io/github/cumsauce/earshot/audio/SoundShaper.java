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
import org.lwjgl.openal.AL11;
import org.lwjgl.openal.EXTEfx;

/**
 * Shapes every positional sound the game plays by how it reaches the listener (M2): muffled by what is in the way,
 * moved to the opening it comes round through, damped by the air over distance, and muffled hard underwater. Each also
 * sends to one reverb whose parameters come from the room measured around the listener (M3, parametric tier).
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
	/**
	 * How fast the reverb send falls with distance, against the direct sound's 1: far sounds are more reverberant
	 * than near ones, which is how the ear judges distance.
	 */
	private static final float ROOM_ROLLOFF = 0.5f;
	/** The EAX reverb parameters in the order the native room gives them. */
	private static final int[] EAX_PARAMETERS = {EXTEfx.AL_EAXREVERB_DENSITY, EXTEfx.AL_EAXREVERB_DIFFUSION,
			EXTEfx.AL_EAXREVERB_GAIN, EXTEfx.AL_EAXREVERB_GAINHF, EXTEfx.AL_EAXREVERB_GAINLF,
			EXTEfx.AL_EAXREVERB_DECAY_TIME, EXTEfx.AL_EAXREVERB_DECAY_HFRATIO, EXTEfx.AL_EAXREVERB_DECAY_LFRATIO,
			EXTEfx.AL_EAXREVERB_REFLECTIONS_GAIN, EXTEfx.AL_EAXREVERB_REFLECTIONS_DELAY,
			EXTEfx.AL_EAXREVERB_LATE_REVERB_GAIN, EXTEfx.AL_EAXREVERB_LATE_REVERB_DELAY, EXTEfx.AL_EAXREVERB_ECHO_TIME,
			EXTEfx.AL_EAXREVERB_ECHO_DEPTH};
	/** EFX's underwater preset, in the same order. */
	private static final float[] UNDERWATER_REVERB = {0.3645f, 1.0f, 0.3162f, 0.01f, 1.0f, 1.49f, 0.1f, 1.0f, 0.5963f,
			0.007f, 7.0795f, 0.011f, 0.25f, 0.0f};

	private final NativeBridge bridge;
	private final MemorySegment engine;
	private volatile List<Snapshot> snapshot = List.of();
	/** The reverb's effect and slot in the game's context; 0 until made, and again when the context goes. */
	private final Object reverbLock = new Object();
	private int effect;
	private int slot;
	private final double[] room = new double[NativeBridge.ROOM_DOUBLES];
	private volatile double[] lastRoom = new double[0];
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

	/** The last room and EAX reverb applied ({@link NativeBridge#room}'s doubles), or empty before the first. */
	public double[] room() {
		return lastRoom.clone();
	}

	/**
	 * The game is closing its OpenAL context, which frees every object in it: forget the reverb, so the next context
	 * gets its own.
	 */
	public void contextClosing() {
		synchronized (reverbLock) {
			effect = 0;
			slot = 0;
		}
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
		boolean eax = efx && report.eaxReverb();
		if (!call(() -> bridge.setListener(engine, listener))) return;
		channels.executeOnChannels(stream -> update(stream, listener, underwater, efx, eax));
	}

	/** On the sound thread: shapes every positional source. */
	private void update(Stream<Channel> channels, double[] listener, boolean underwater, boolean efx, boolean eax) {
		int reverb = eax ? updateReverb(underwater) : 0;
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
			if (reverb != 0) send(channel, state, gains, reverb);
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
	 * Sets the reverb from the room measured around the listener (or EFX's underwater preset), making the effect and
	 * slot first if this context has none yet. Returns the slot. On the sound thread.
	 */
	private int updateReverb(boolean underwater) {
		if (!call(() -> bridge.room(engine, room))) return 0;
		lastRoom = room.clone();
		synchronized (reverbLock) {
			if (slot == 0) {
				effect = EXTEfx.alGenEffects();
				EXTEfx.alEffecti(effect, EXTEfx.AL_EFFECT_TYPE, EXTEfx.AL_EFFECT_EAXREVERB);
				slot = EXTEfx.alGenAuxiliaryEffectSlots();
			}
			for (int i = 0; i < EAX_PARAMETERS.length; i++) {
				float value = underwater ? UNDERWATER_REVERB[i] : (float) room[NativeBridge.EAX_FIRST + i];
				EXTEfx.alEffectf(effect, EAX_PARAMETERS[i], value);
			}
			// A slot copies its effect when given it, so it is given it again after every change.
			EXTEfx.alAuxiliaryEffectSloti(slot, EXTEfx.AL_EFFECTSLOT_EFFECT, effect);
			return slot;
		}
	}

	/**
	 * Sends a source to the reverb. What reaches the listener's space excites its reverb, so the send follows the
	 * direct gains, but softened (square root): a muffled sound behind a wall still fills the room a little.
	 */
	private static void send(ShapedChannel channel, SourceState state, float[] gains, int slot) {
		int source = channel.earshot$source();
		if (state.sendFilter == 0) {
			state.sendFilter = EXTEfx.alGenFilters();
			EXTEfx.alFilteri(state.sendFilter, EXTEfx.AL_FILTER_TYPE, EXTEfx.AL_FILTER_LOWPASS);
			AL10.alSourcef(source, EXTEfx.AL_ROOM_ROLLOFF_FACTOR, ROOM_ROLLOFF);
		}
		float low = (float) Math.sqrt(Math.clamp(gains[0], 0f, 1f));
		float high = (float) Math.sqrt(Math.clamp(gains[2], 0f, 1f));
		EXTEfx.alFilterf(state.sendFilter, EXTEfx.AL_LOWPASS_GAIN, low);
		EXTEfx.alFilterf(state.sendFilter, EXTEfx.AL_LOWPASS_GAINHF, low > 0 ? Math.clamp(high / low, 0f, 1f) : 0f);
		AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, slot, 0, state.sendFilter);
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
		if (state.sendFilter != 0) {
			EXTEfx.alDeleteFilters(state.sendFilter);
			state.sendFilter = 0;
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
