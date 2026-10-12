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
import org.joml.Vector3fc;
import org.jspecify.annotations.Nullable;
import org.lwjgl.openal.AL10;
import org.lwjgl.openal.AL11;
import org.lwjgl.openal.EXTEfx;

/**
 * Shapes every positional sound the game plays by how it reaches the listener (M2): muffled by what is in the way,
 * moved to the opening it comes round through, damped by the air over distance, and muffled hard underwater. Each also
 * sends to a reverb made from the room measured around the listener (M3): a convolution with a synthesized impulse
 * response where OpenAL Soft has the effect, else EAX reverb set from the room's measurements.
 *
 * <p>Once a tick, after the sound engine's own tick, {@link #tick} tells the native engine where the listener is and
 * queues {@link #update} on the sound thread, which asks the native engine how each playing sound is heard (one call
 * for all of them) and sets each OpenAL source's direct filter, position and reverb sends.
 */
public final class SoundShaper {
	/** How the reverb is made. */
	public enum ReverbMode {
		OFF,
		/** EFX's EAX reverb, set from the room's measurements. */
		EAX,
		/** Convolution with impulse responses synthesized from the room. */
		CONVOLUTION
	}

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
	private final ConvolutionReverb convolution;
	private volatile List<Snapshot> snapshot = List.of();
	/** The EAX reverb's effect and slot in the game's context; 0 until made, and again when the context goes. */
	private final Object reverbLock = new Object();
	private int effect;
	private int slot;
	private final double[] room = new double[NativeBridge.ROOM_DOUBLES];
	private volatile double[] lastRoom = new double[0];
	/** The reverb asked for, or null for the best the game's OpenAL has. */
	private volatile @Nullable ReverbMode wantedMode;
	private volatile ReverbMode mode = ReverbMode.OFF;
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
		this.convolution = new ConvolutionReverb(bridge, engine);
	}

	/** The sounds shaped in the last update. */
	public List<Snapshot> snapshot() {
		return snapshot;
	}

	/** The last room and EAX reverb applied ({@link NativeBridge#room}'s doubles), or empty before the first. */
	public double[] room() {
		return lastRoom.clone();
	}

	public ConvolutionReverb convolution() {
		return convolution;
	}

	/** The reverb in use. */
	public ReverbMode mode() {
		return mode;
	}

	/** Asks for a reverb; null for the best the game's OpenAL has. */
	public void wantMode(@Nullable ReverbMode wanted) {
		wantedMode = wanted;
	}

	/**
	 * The game is closing its OpenAL context, which frees every object in it: forget the reverbs, so the next context
	 * gets its own.
	 */
	public void contextClosing() {
		synchronized (reverbLock) {
			effect = 0;
			slot = 0;
		}
		convolution.contextClosing();
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
		float[] orientation = worldAxesFor(camera);
		OpenAlProbe.Report report = OpenAlProbe.last();
		boolean efx = report != null && report.efx();
		ReverbMode mode = chooseMode(report, underwater);
		this.mode = mode;
		if (!call(() -> bridge.setListener(engine, listener))) return;
		channels.executeOnChannels(stream -> update(stream, listener, underwater, efx, mode, orientation));
	}

	/** The wanted reverb, if the game's OpenAL can do it, else the best it can; underwater, EAX's preset. */
	private ReverbMode chooseMode(OpenAlProbe.@Nullable Report report, boolean underwater) {
		boolean eax = report != null && report.efx() && report.eaxReverb();
		boolean convolution = report != null && report.efx() && report.convolutionEffect();
		ReverbMode wanted = wantedMode;
		if (wanted == ReverbMode.OFF) return ReverbMode.OFF;
		if (underwater || wanted == ReverbMode.EAX) return eax ? ReverbMode.EAX : ReverbMode.OFF;
		if (convolution) return ReverbMode.CONVOLUTION;
		return eax ? ReverbMode.EAX : ReverbMode.OFF;
	}

	/**
	 * The world's -z and +y axes in the listener's frame (x right, y up, -z ahead), which is the orientation that
	 * turns a world-axes impulse response with the listener.
	 */
	private static float[] worldAxesFor(Camera camera) {
		Vector3fc f = camera.forwardVector();
		Vector3fc u = camera.upVector();
		Vector3fc l = camera.leftVector();
		float ry = -l.y(), rz = -l.z();
		return new float[] {-rz, -u.z(), f.z(), ry, u.y(), -f.y()};
	}

	/** On the sound thread: shapes every positional source. */
	private void update(Stream<Channel> channels, double[] listener, boolean underwater, boolean efx,
			ReverbMode mode, float[] orientation) {
		int[] sends = reverb(mode, underwater, orientation);
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
			if (efx) {
				filter(channel, state, gains);
				send(channel, state, gains, sends);
			}
			Vec3 heardFrom = state.heardFrom(position);
			AL10.alSource3f(channel.earshot$source(), AL10.AL_POSITION, (float) heardFrom.x, (float) heardFrom.y,
					(float) heardFrom.z);
			shots.add(new Snapshot(position, heardFrom, gains));
		}
		snapshot = List.copyOf(shots);
	}

	/**
	 * Brings the reverb up to date and returns the slots sources send to, by send index (0 for none). On the sound
	 * thread.
	 */
	private int[] reverb(ReverbMode mode, boolean underwater, float[] orientation) {
		if (mode == ReverbMode.OFF || !call(() -> bridge.room(engine, room))) return new int[2];
		lastRoom = room.clone();
		if (mode == ReverbMode.CONVOLUTION) {
			convolution.tick(room, orientation);
			return convolution.slots();
		}
		return new int[] {updateEax(underwater), 0};
	}

	/**
	 * Sets the EAX reverb from the room (or EFX's underwater preset), making the effect and slot first if this
	 * context has none yet. Returns the slot.
	 */
	private int updateEax(boolean underwater) {
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
	 * Sends a source to the reverb slots. What reaches the listener's space excites its reverb, so the send follows
	 * the direct gains, but softened (square root): a muffled sound behind a wall still fills the room a little.
	 */
	private static void send(ShapedChannel channel, SourceState state, float[] gains, int[] slots) {
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
		for (int send = 0; send < slots.length; send++) {
			AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, slots[send], send,
					slots[send] == 0 ? EXTEfx.AL_FILTER_NULL : state.sendFilter);
		}
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

	/** Frees a source's filters when the game destroys the source. Called on the sound thread. */
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
