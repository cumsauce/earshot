package io.github.cumsauce.earshot.audio;

import com.mojang.blaze3d.audio.Channel;
import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.natives.EarshotException;
import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import java.lang.foreign.MemorySegment;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.function.IntConsumer;
import java.util.function.IntSupplier;
import java.util.stream.Stream;
import net.minecraft.client.Camera;
import net.minecraft.client.Minecraft;
import net.minecraft.client.player.AbstractClientPlayer;
import net.minecraft.client.sounds.ChannelAccess;
import net.minecraft.sounds.SoundSource;
import net.minecraft.world.level.material.FogType;
import net.minecraft.world.phys.Vec3;
import org.joml.Vector3fc;
import org.jspecify.annotations.Nullable;
import org.lwjgl.openal.AL10;
import org.lwjgl.openal.AL11;
import org.lwjgl.openal.EXTEfx;

/**
 * Shapes every positional sound the game plays by how it reaches the listener (M2): muffled by what is in the way,
 * moved to the opening it comes round through, damped by the air over distance, and muffled hard underwater. Moving
 * sounds, and the listener, get velocities for OpenAL's Doppler shift. Each also
 * sends to a reverb made from the room measured around the listener (M3): a convolution with a synthesized impulse
 * response where OpenAL Soft has the effect, else EAX reverb set from the room's measurements. Yak VC's voices, played
 * by {@link VoicePlayer}, are shaped the same way (M4).
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
	private static final float[] NO_VELOCITY = {0, 0, 0};
	/** Underwater, everything sounds like this, low to high. */
	private static final float[] UNDERWATER = {0.7f, 0.2f, 0.05f};
	/**
	 * How fast the reverb send falls with distance, against the direct sound's 1: far sounds are more reverberant
	 * than near ones, which is how the ear judges distance.
	 */
	private static final float ROOM_ROLLOFF = 0.5f;
	/** The EAX reverb parameters in the order the native room gives them. */
	static final int[] EAX_PARAMETERS = {EXTEfx.AL_EAXREVERB_DENSITY, EXTEfx.AL_EAXREVERB_DIFFUSION,
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
	private final SpaceReverb spaces;
	private final EarlyReflections early;
	/**
	 * On Ultra with convolution, the loudest sounds' own reverbs: each made from the room round the sound and its
	 * reflections off its own walls, in place of the listener's.
	 */
	private final ConvolutionReverb[] ownReverbs = new ConvolutionReverb[OWN_REVERBS];
	private static final int OWN_REVERBS = 2;
	/**
	 * Own reverbs no sound is given this update, let go only at its end, once every source's sends have moved off
	 * them: OpenAL won't delete a slot a source still sends to.
	 */
	private final boolean[] unusedOwn = new boolean[OWN_REVERBS];
	/**
	 * The OpenAL source each own reverb and each early-reflections slot was last given (0 for none), so a sound keeps
	 * its slot while it stays among the loudest: given by rank, two sounds trading places swapped slots every tick,
	 * jumping the reflections and remaking the convolutions.
	 */
	private final int[] ownOwners = new int[OWN_REVERBS];
	private final int[] earlyOwners = new int[EarlyReflections.SOURCES];
	/** A sound's own reverb is made again once it has moved this far. */
	private static final double OWN_REVERB_MOVE = 2;
	private final VoicePlayer voice;
	private volatile List<Snapshot> snapshot = List.of();
	/** The EAX reverb's effect and slot in the game's context; 0 until made, and again when the context goes. */
	private final Object reverbLock = new Object();
	private int effect;
	private int slot;
	/**
	 * The device refused the EAX reverb a slot (its slot cap was reached): off for the rest of this context, so no
	 * tick spends an effect on a slot that will not come. The close clears it, as it clears the names.
	 */
	private boolean eaxOff;
	private final double[] room = new double[NativeBridge.ROOM_DOUBLES];
	/**
	 * The reverb as heard, gliding each tick towards the room as last measured: measured every fifth of a second or
	 * so (slower on Ultra), it stepped with each. Null until there's one; jumps when going under or out of water.
	 */
	private float @Nullable [] heardEax;
	private boolean heardUnderwater;
	/** How far the heard reverb moves towards the measured each tick: about a third of a second. */
	private static final float EAX_GLIDE = 0.15f;
	private volatile double[] lastRoom = new double[0];
	/** The reverb asked for, or null for the best the game's OpenAL has. */
	private volatile @Nullable ReverbMode wantedMode;
	private volatile ReverbMode mode = ReverbMode.OFF;
	private volatile boolean failed;
	/** OpenAL's air absorption factor now: from the climate where the listener is, followed smoothly. */
	private volatile float airAbsorption = AIR_ABSORPTION;
	private volatile boolean airAbsorptionOn = true;
	private volatile boolean underwaterMuffling = true;
	private volatile boolean doppler = true;
	/** Whether sounds in another space get that space's reverb (High and Ultra). */
	private volatile boolean spaceReverb;
	/** Whether the loudest sounds get early reflections of their own (Ultra). */
	private volatile boolean earlyReflections;
	/** Whether the game's sounds fall off as 1/distance rather than linearly. */
	private volatile boolean realisticFalloff;
	/**
	 * Realistic falloff: full volume within this many blocks, then 1/distance, 6 dB quieter each doubling, as in open
	 * air (air absorption and occlusion take more on top).
	 */
	private static final float REFERENCE_DISTANCE = 3;
	/**
	 * Sounds carry this many times the game's range (16 blocks for most), as real ones do; in a world this game hosts,
	 * the server sends them that far too ({@code SoundEventMixin}).
	 */
	public static final float REACH = 4;
	/** And fade to silence over the last 15 % of that, rather than stopping dead. */
	private static final float FADE_SHARE = 0.15f;
	/** The listener's velocity, estimated on the client thread. */
	private final SourceState.Motion listenerMotion = new SourceState.Motion();

	/**
	 * One shaped sound, for the debug command and tests: {@code voice} for a Yak VC voice, {@code elsewhere} when it
	 * gets the reverb of another space than the listener's.
	 */
	public record Snapshot(Vec3 position, Vec3 heardFrom, float[] gains, boolean voice, boolean elsewhere,
			boolean early, boolean ownReverb, float aperture) {
		@Override
		public String toString() {
			return "Snapshot[position=" + position + ", heardFrom=" + heardFrom + ", gains="
					+ java.util.Arrays.toString(gains) + ", voice=" + voice + ", elsewhere=" + elsewhere + ", early="
					+ early + ", ownReverb=" + ownReverb + ", aperture=" + aperture + "]";
		}
	}

	/** The listener's right, up and forward, in world axes. */
	private record Axes(Vec3 right, Vec3 up, Vec3 forward) {}

	/** A source to shape: the OpenAL source, its smoothed shaping, where it is, and the voice it plays if any. */
	record Target(int source, SourceState state, Vec3 position, VoicePlayer.@Nullable Voice voice,
			float range) {}

	public SoundShaper(NativeBridge bridge, MemorySegment engine) {
		this.bridge = bridge;
		this.engine = engine;
		this.convolution = new ConvolutionReverb(bridge, engine);
		this.spaces = new SpaceReverb(bridge, engine);
		this.early = new EarlyReflections(bridge, engine);
		for (int i = 0; i < OWN_REVERBS; i++) {
			ownReverbs[i] = new ConvolutionReverb(
					(at, seed) -> bridge.impulseResponseFor(engine, at, 1024, 8, 48_000, 1.5f, seed),
					(madeFrom, at) -> Math.hypot(Math.hypot(at[0] - madeFrom[0], at[1] - madeFrom[1]),
							at[2] - madeFrom[2]) > OWN_REVERB_MOVE);
		}
		this.voice = new VoicePlayer(bridge);
	}

	/** The rooms measured for sounds in other spaces, as {@link NativeBridge#room}'s doubles. For tests. */
	public List<double[]> spaceRooms() {
		return spaces.rooms();
	}

	/** What plays Yak VC's voices. */
	public VoicePlayer voice() {
		return voice;
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

	/** Takes up the settings (the engine's quality is set separately). */
	public void configure(io.github.cumsauce.earshot.config.EarshotConfig config) {
		boolean active = config.active();
		airAbsorptionOn = active && config.airAbsorption();
		if (!airAbsorptionOn) airAbsorption = 0;
		underwaterMuffling = active && config.underwater();
		doppler = active && config.doppler();
		var preset = config.preset().resolve();
		spaceReverb = active && config.reverb() != io.github.cumsauce.earshot.config.EarshotConfig.Reverb.OFF
				&& (preset == io.github.cumsauce.earshot.config.EarshotConfig.Preset.HIGH
						|| preset == io.github.cumsauce.earshot.config.EarshotConfig.Preset.ULTRA);
		realisticFalloff = active && config.realisticFalloff();
		earlyReflections = active && config.reverb() != io.github.cumsauce.earshot.config.EarshotConfig.Reverb.OFF
				&& preset == io.github.cumsauce.earshot.config.EarshotConfig.Preset.ULTRA;
		wantMode(!active ? ReverbMode.OFF : switch (config.reverb()) {
			case AUTO -> null;
			case CONVOLUTION -> ReverbMode.CONVOLUTION;
			case EAX -> ReverbMode.EAX;
			case OFF -> ReverbMode.OFF;
		});
	}

	/**
	 * The game is closing its OpenAL context, which frees every object in it: forget the reverbs, so the next context
	 * gets its own.
	 */
	public void contextClosing() {
		synchronized (reverbLock) {
			effect = 0;
			slot = 0;
			heardEax = null;
			// A fresh context gets a fresh chance at a slot, as it does at every other name.
			eaxOff = false;
		}
		// A fresh context gets a fresh chance: what failed against the old one says nothing about the new.
		failed = false;
		convolution.contextClosing();
		spaces.contextClosing();
		early.contextClosing();
		for (ConvolutionReverb own : ownReverbs) own.contextClosing();
		forgetOwners(ownOwners, earlyOwners, unusedOwn);
		voice.contextClosing();
	}

	/** Called on the client thread at the end of the sound engine's tick. */
	public void tick(ChannelAccess channels) {
		if (failed) return;
		Minecraft minecraft = Minecraft.getInstance();
		Camera camera = minecraft.gameRenderer.mainCamera();
		if (minecraft.level == null || !camera.isInitialized()) return;
		Vec3 eye = camera.position();
		double[] listener = {eye.x, eye.y, eye.z};
		boolean underwater = underwaterMuffling && camera.getFluidInCamera() == FogType.WATER;
		followClimate(minecraft, eye);
		Vec3 listenerVelocity = doppler ? listenerMotion.update(eye, System.nanoTime()) : Vec3.ZERO;
		float[] orientation = worldAxesFor(camera);
		Vector3fc left = camera.leftVector();
		Axes axes = new Axes(new Vec3(-left.x(), -left.y(), -left.z()), new Vec3(camera.upVector()),
				new Vec3(camera.forwardVector()));
		OpenAlProbe.Report report = OpenAlProbe.last();
		boolean efx = report != null && report.efx();
		ReverbMode mode = chooseMode(report, underwater);
		this.mode = mode;
		if (!call(() -> bridge.setListener(engine, listener))) return;
		Map<UUID, VoicePlayer.Head> players = new HashMap<>();
		float voiceVolume = Math.clamp(minecraft.options.getFinalSoundSourceVolume(SoundSource.VOICE), 0f, 1f);
		if (voice.offered()) {
			for (AbstractClientPlayer player : minecraft.level.players()) {
				players.put(player.getUUID(), new VoicePlayer.Head(player.getEyePosition(),
						net.minecraft.world.entity.Entity.calculateViewVector(player.getXRot(), player.getYHeadRot())));
			}
		}
		channels.executeOnChannels(
				stream -> update(stream, listener, underwater, efx, mode, orientation, players, voiceVolume,
						listenerVelocity, axes));
	}

	/** OpenAL's air absorption factor in use now. */
	public float airAbsorption() {
		return airAbsorption;
	}

	/**
	 * Moves the air absorption towards what the climate where the listener is gives (ISO 9613-1, {@link Air}), over
	 * about a second, so walking out of a desert doesn't click.
	 */
	private void followClimate(Minecraft minecraft, Vec3 eye) {
		if (!airAbsorptionOn || minecraft.level == null) return;
		var pos = net.minecraft.core.BlockPos.containing(eye);
		var biome = minecraft.level.getBiome(pos).value();
		boolean rains = biome.hasPrecipitation();
		float target = Air.factorForBiome(biome.getBaseTemperature(), rains, rains && minecraft.level.isRaining());
		airAbsorption += (target - airAbsorption) * 0.05f;
	}

	/** The wanted reverb, if the game's OpenAL can do it, else the best it can; underwater, EAX's preset. */
	private ReverbMode chooseMode(OpenAlProbe.@Nullable Report report, boolean underwater) {
		boolean eax = report != null && report.efx() && report.eaxReverb();
		boolean convolution = report != null && report.efx() && report.convolutionEffect();
		ReverbMode wanted = wantedMode;
		if (wanted == ReverbMode.OFF) return ReverbMode.OFF;
		// Convolution only when asked for: in a busy scene it came out some 30 dB too loud, as a wash of noise, for
		// reasons inside OpenAL Soft's convolution still being found. Auto takes the EAX reverb.
		if (!underwater && wanted == ReverbMode.CONVOLUTION && convolution) return ReverbMode.CONVOLUTION;
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

	/** On the sound thread: shapes every positional source, the game's and Yak VC's voices. */
	private void update(Stream<Channel> channels, double[] listener, boolean underwater, boolean efx,
			ReverbMode mode, float[] orientation, Map<UUID, VoicePlayer.Head> players, float voiceVolume, Vec3 listenerVelocity,
			Axes axes) {
		Vec3 ear = new Vec3(listener[0], listener[1], listener[2]);
		long now = System.nanoTime();
		// Filled by the pass as it finds targets, so a pass that dies part way can still release what it had:
		// its sends taken off and the slots freed. Empty (and the release a no-op on sources) if it dies before
		// any target was known.
		List<Target> targets = new ArrayList<>();
		shapeSafely(() -> shape(channels, listener, underwater, efx, mode, orientation, players, voiceVolume,
				listenerVelocity, axes, ear, now, targets), () -> {
			// A pass that dies part way must not repeat every tick: half of the sources would keep the filters
			// and sends set on them, spaces.begin() would have no matching end(), and unusedOwn would never be
			// released — and with failed unset, tick() would queue the same broken pass forever. Turn shaping
			// off cleanly instead: failed stops tick(), voice.fail() frees the voices, and releaseAll takes
			// every send off the tracked sources and frees the slots, whatever the pass had time to do.
			failed = true;
			voice.fail();
			releaseAll(targets, now, ear, axes);
		});
	}

	/**
	 * One shaping pass: the reverb up to date, then every playing source's filter, position, velocity and sends.
	 * On the sound thread, run only through {@link #update}, which guards it with {@link #shapeSafely}.
	 */
	private void shape(Stream<Channel> channels, double[] listener, boolean underwater, boolean efx,
			ReverbMode mode, float[] orientation, Map<UUID, VoicePlayer.Head> players, float voiceVolume, Vec3 listenerVelocity,
			Axes axes, Vec3 ear, long now, List<Target> targets) {
		// Before reverb(): a pass that dies in reverb() must still leave a begin() for releaseAll's
		// spaces.end() to close. reverb() makes no space slot, so the order costs nothing.
		spaces.begin();
		int[] sends = reverb(mode, underwater, orientation);
		boolean spacesOn = spaceReverb && efx && mode != ReverbMode.OFF && !underwater;
		AL10.alListener3f(AL10.AL_VELOCITY, (float) listenerVelocity.x, (float) listenerVelocity.y,
				(float) listenerVelocity.z);
		boolean doppler = this.doppler;
		channels.forEach(channel -> {
			ShapedChannel c = (ShapedChannel) channel;
			Vec3 position = c.earshot$position();
			if (c.earshot$positional() && position != null) {
				targets.add(new Target(c.earshot$source(), c.earshot$state(), position, null, c.earshot$range()));
			}
		});
		for (VoicePlayer.Voice v : voice.update(players, voiceVolume, sends)) targets.add(new Target(v.source, v.state, v.position, v, 0));
		// A source at a point the native engine cannot use (as Vec3::is_usable: NaN, infinite, or past 1e9)
		// is not heard: one such point would fail hear() for every source at once. The bad ones are dropped
		// before the sources array is built, and the shaping earlier updates set on them is taken off, so a
		// source gone bad does not keep a stale filter or hold a reverb send open.
		List<Target> dropped = new ArrayList<>();
		List<Target> kept = heardTargets(targets, dropped);
		targets.clear();
		targets.addAll(kept);
		for (Target target : dropped) releaseStale(target);
		int count = targets.size();
		if (count == 0) {
			snapshot = List.of();
			voice.report();
			spaces.end(System.nanoTime(), !spacesOn, ear, axes.right(), axes.up(), axes.forward());
			if (!earlyOn(efx, underwater)) early.free();
			for (ConvolutionReverb own : ownReverbs) own.release();
			releaseConvolutionWhenOff();
			return;
		}
		double[] sources = new double[3 * count];
		for (int i = 0; i < count; i++) {
			Vec3 position = targets.get(i).position();
			sources[3 * i] = position.x;
			sources[3 * i + 1] = position.y;
			sources[3 * i + 2] = position.z;
		}
		double[] heard = new double[NativeBridge.HEARD_DOUBLES * count];
		if (!call(() -> bridge.hear(engine, listener, sources, count, heard))) {
			releaseAll(targets, now, ear, axes);
			return;
		}
		// Each source's block is read straight out of heard below (heard[at] … heard[at + 7]), so a count
		// mismatch stops shaping here, as a failed native call does, instead of throwing part way through.
		if (!heardHolds(heard, count)) {
			failed = true;
			EarshotClient.LOGGER.error("Sound shaping stopped: hear broke its contract, {} for {} sources, needing {}",
					heard.length, count, (long) NativeBridge.HEARD_DOUBLES * count);
			voice.fail();
			releaseAll(targets, now, ear, axes);
			return;
		}

		int[][] ownSends = new int[count][];
		int[] earlySlots = earlySlots(targets, heard, listener, ear, axes, efx, underwater,
				mode == ReverbMode.CONVOLUTION, orientation, ownSends);
		List<Snapshot> shots = new ArrayList<>(count);
		for (int i = 0; i < count; i++) {
			Target target = targets.get(i);
			SourceState state = target.state();
			int at = NativeBridge.HEARD_DOUBLES * i;
			Vec3 position = target.position();
			state.approach(new float[] {(float) heard[at], (float) heard[at + 1], (float) heard[at + 2]},
					new Vec3(heard[at + 3], heard[at + 4], heard[at + 5]), position);
			float[] gains = state.gains();
			if (underwater) {
				for (int band = 0; band < gains.length; band++) gains[band] *= UNDERWATER[band];
			}
			Vec3 heardFrom = state.heardFrom(position);
			double straight = position.distanceTo(ear);
			boolean elsewhere = spacesOn && straight > 3
					&& (heard[at + 6] > straight + SpaceReverb.ROUND || heard[at + 1] < SpaceReverb.WALLED);
			int spaceSlot = elsewhere ? spaces.slotFor(position, heardFrom, now) : 0;
			// A sound that doesn't fade with distance (a placed bed) sets its own level: only walls and rooms shape it.
			if (target.voice() == null && target.range() > 0) {
				float fade = falloff(target.source(), state, target.range(), (float) straight);
				for (int band = 0; band < gains.length; band++) gains[band] *= fade;
			}
			// The ground's reflection can bring the direct sound above free field, and the mixer's low-pass cannot
			// pass one: the lift rides the source's own gain (times the game's volume) and the filter takes the rest.
			// A voice's gain is VoicePlayer's own, so a voice takes no lift here.
			float boost = target.voice() == null ? lift(gains) : 1f;
			if (target.voice() == null) {
				AL10.alSourcef(target.source(), AL10.AL_GAIN, state.volume() * boost);
			}
			if (efx) {
				filter(target.source(), state, gains, boost);
				send(target.source(), state, gains, ownSends[i] != null ? ownSends[i] : sends, spaceSlot,
						portalShare((float) heard[at + 7]));
				sendEarly(target.source(), state, earlySlots[i]);
			}
			AL10.alSource3f(target.source(), AL10.AL_POSITION, (float) heardFrom.x, (float) heardFrom.y,
					(float) heardFrom.z);
			Vec3 velocity = state.velocity(position, now);
			if (doppler) {
				AL10.alSource3f(target.source(), AL10.AL_VELOCITY, (float) velocity.x, (float) velocity.y,
						(float) velocity.z);
			} else {
				AL10.alSourcefv(target.source(), AL10.AL_VELOCITY, NO_VELOCITY);
			}
			if (target.voice() != null) {
				target.voice().heardFrom = heardFrom;
				target.voice().gains = gains;
			}
			shots.add(new Snapshot(position, heardFrom, gains, target.voice() != null, spaceSlot != 0,
					earlySlots[i] != 0, ownSends[i] != null, (float) heard[at + 7]));
		}
		snapshot = List.copyOf(shots);
		voice.report();
		spaces.end(now, !spacesOn, ear, axes.right(), axes.up(), axes.forward());
		if (!earlyOn(efx, underwater)) early.free();
		for (int k = 0; k < OWN_REVERBS; k++) {
			if (unusedOwn[k]) ownReverbs[k].release();
		}
		releaseConvolutionWhenOff();
	}

	/**
	 * The convolution reverb's slots keep convolving (about a tenth of a core) whether or not a source sends to
	 * them, so once the mode has left convolution they are let go. Called at the end of a pass, after every
	 * source's sends have been rewritten, so nothing still sends to a slot being deleted. Idempotent: releasing
	 * with nothing named does nothing.
	 */
	private void releaseConvolutionWhenOff() {
		if (!freesConvolution(mode)) return;
		convolution.release();
	}

	/** Whether the mode has left convolution, so its slots should be let go rather than keep convolving. */
	static boolean freesConvolution(ReverbMode mode) {
		return mode != ReverbMode.CONVOLUTION;
	}

	/**
	 * Lets go of the OpenAL resources an update made when no update will follow to let go of them: every space's
	 * slot ({@code off}), the early slots, and both own reverbs, kept or not — unlike the end of a normal update,
	 * where spaces stay while they have sounds, the early slots stay while reflections are on, and only the own
	 * reverbs no sound kept are released. {@link #failed} has stopped {@link #tick} for now, so anything left
	 * live here would stay so until the context closes. On the sound thread.
	 *
	 * <p>Every tracked source's sends come off first: OpenAL won't free a slot a source still sends to, so the
	 * frees below would fail and leave the slots mixing until the context closes.
	 */
	private void releaseAll(List<Target> targets, long now, Vec3 ear, Axes axes) {
		for (Target target : targets) {
			int source = target.source();
			SourceState state = target.state();
			if (state.spaceSent) {
				AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, EXTEfx.AL_EFFECTSLOT_NULL,
						SpaceReverb.SEND, EXTEfx.AL_FILTER_NULL);
				state.spaceSent = false;
			}
			if (state.earlySent) {
				AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, EXTEfx.AL_EFFECTSLOT_NULL,
						EarlyReflections.SEND, EXTEfx.AL_FILTER_NULL);
				state.earlySent = false;
			}
		}
		for (int k = 0; k < OWN_REVERBS; k++) {
			// An own reverb's slots sit on sends 0 and 1 of the one sound given it; one that has gone
			// since took its sends away with its source.
			int source = ownOwners[k];
			if (source != 0 && plays(targets, source)) {
				AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, EXTEfx.AL_EFFECTSLOT_NULL, 0,
						EXTEfx.AL_FILTER_NULL);
				AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, EXTEfx.AL_EFFECTSLOT_NULL, 1,
						EXTEfx.AL_FILTER_NULL);
			}
			ownOwners[k] = 0;
		}
		java.util.Arrays.fill(earlyOwners, 0);
		spaces.end(now, true, ear, axes.right(), axes.up(), axes.forward());
		early.free();
		for (ConvolutionReverb own : ownReverbs) own.release();
		releaseConvolutionWhenOff();
	}

	/**
	 * Forgets which sources held the own-reverb and early-reflections slots (and which own reverbs went unused),
	 * as {@link #contextClosing} does: the close frees every slot, and the source ids a fresh context hands out
	 * start over, so an id that comes again would otherwise be given the old owner's slot by id alone.
	 */
	static void forgetOwners(int[] ownOwners, int[] earlyOwners, boolean[] unusedOwn) {
		java.util.Arrays.fill(ownOwners, 0);
		java.util.Arrays.fill(earlyOwners, 0);
		java.util.Arrays.fill(unusedOwn, false);
	}

	/** Whether {@code source} is among {@code targets}, still being shaped. */
	private static boolean plays(List<Target> targets, int source) {
		for (Target target : targets) {
			if (target.source() == source) return true;
		}
		return false;
	}

	/**
	 * The targets at points the native engine can use ({@link SourceState#usable}), which is every one it will
	 * hear; the rest — a NaN or absurd position from some other mod — go to {@code dropped} rather than into the
	 * sources array, where one would fail the whole hear() call.
	 */
	static List<Target> heardTargets(List<Target> targets, List<Target> dropped) {
		List<Target> heard = new ArrayList<>(targets.size());
		for (Target target : targets) {
			if (SourceState.usable(target.position())) heard.add(target);
			else dropped.add(target);
		}
		return heard;
	}

	/**
	 * Takes off the shaping a source keeps from earlier updates when this update cannot hear it: its sends, which
	 * would hold reverb slots open, and its filters, as {@link #release} does when the game destroys the source.
	 * The early-reflections and own-reverb slots it held are let go by {@link #keep}, which hands slots out only
	 * to the targets still heard.
	 */
	private static void releaseStale(Target target) {
		int source = target.source();
		SourceState state = target.state();
		if (state.spaceSent) {
			AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, EXTEfx.AL_EFFECTSLOT_NULL,
					SpaceReverb.SEND, EXTEfx.AL_FILTER_NULL);
			state.spaceSent = false;
		}
		if (state.earlySent) {
			AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, EXTEfx.AL_EFFECTSLOT_NULL,
					EarlyReflections.SEND, EXTEfx.AL_FILTER_NULL);
			state.earlySent = false;
		}
		releaseFilters(source, state);
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
	 * An EFX effect and the auxiliary effect slot that plays it. Either name is 0 where OpenAL would not give one,
	 * which leaves nothing a source can send to.
	 */
	record EffectSlot(int effect, int slot) {
	}

	/**
	 * Makes an EFX pair the only way the two can be kept together: the effect first, then the slot that plays it. A
	 * device whose slots are all used gives 0 back for the slot, and an effect with no slot to play in is nothing but
	 * a leak, regenerated on every later tick by a caller that tries again. So a half-made pair is deleted at once,
	 * each name by its own, and the caller is told 0, no send at all, instead of a slot name OpenAL never handed out.
	 * The OpenAL calls are taken as parameters so the pairing itself can be tested without a live context.
	 *
	 * @return the pair made, both names 0 where either would not be
	 */
	static EffectSlot makeEffectSlot(IntSupplier genEffect, IntSupplier genSlot, IntConsumer deleteEffect,
			IntConsumer deleteSlot) {
		int effect = genEffect.getAsInt();
		int slot = genSlot.getAsInt();
		if (effect == 0 || slot == 0) {
			deleteEffectSlot(effect, slot, deleteEffect, deleteSlot);
			return new EffectSlot(0, 0);
		}
		return new EffectSlot(effect, slot);
	}

	/**
	 * Lets an effect and its slot go, each name deleted by its own: an effect whose slot never came (or has already
	 * gone) must still be deleted, or it stays live until the context closes.
	 */
	static void deleteEffectSlot(int effect, int slot, IntConsumer deleteEffect, IntConsumer deleteSlot) {
		if (slot != 0) deleteSlot.accept(slot);
		if (effect != 0) deleteEffect.accept(effect);
	}

	/**
	 * Sets the EAX reverb from the room (or EFX's underwater preset), making the effect and slot first if this
	 * context has none yet. Returns the slot, or 0 where the device would not give one, which leaves the send empty.
	 */
	private int updateEax(boolean underwater) {
		synchronized (reverbLock) {
			if (slot == 0 && !eaxOff) {
				EffectSlot made = makeEffectSlot(() -> {
					int effect = EXTEfx.alGenEffects();
					EXTEfx.alEffecti(effect, EXTEfx.AL_EFFECT_TYPE, EXTEfx.AL_EFFECT_EAXREVERB);
					return effect;
				}, EXTEfx::alGenAuxiliaryEffectSlots, EXTEfx::alDeleteEffects, EXTEfx::alDeleteAuxiliaryEffectSlots);
				effect = made.effect();
				slot = made.slot();
				if (slot == 0) {
					// The device is out of slots: every later tick would fail the same way and spend one effect
					// each, so the reverb stays off until the context closes and hands out a fresh chance.
					eaxOff = true;
					EarshotClient.LOGGER.warn("EAX reverb is off: OpenAL would not give it a slot");
				}
			}
			if (slot == 0) return 0;
			boolean jump = heardEax == null || underwater != heardUnderwater;
			if (heardEax == null) heardEax = new float[EAX_PARAMETERS.length];
			heardUnderwater = underwater;
			requireRoom(room);
			for (int i = 0; i < EAX_PARAMETERS.length; i++) {
				float value = underwater ? UNDERWATER_REVERB[i] : (float) room[NativeBridge.EAX_FIRST + i];
				heardEax[i] = jump ? value : heardEax[i] + (value - heardEax[i]) * EAX_GLIDE;
				EXTEfx.alEffectf(effect, EAX_PARAMETERS[i], heardEax[i]);
			}
			// A slot copies its effect when given it, so it is given it again after every change.
			EXTEfx.alAuxiliaryEffectSloti(slot, EXTEfx.AL_EFFECTSLOT_EFFECT, effect);
			return slot;
		}
	}

	/** The most the mixer will lift a source above the game's own volume: the hard ground's +6 dB. */
	private static final float MAX_LIFT = 2f;

	/**
	 * How much of a direct gain above one must ride the source's own gain rather than the filter: an EFX low-pass
	 * cannot pass more than one, so the ground's reflection (which can bring the direct sound above free field) has
	 * nowhere else to go. One for an ordinary gain, so nothing indoors changes.
	 */
	static float lift(float[] gains) {
		float most = Math.max(1f, Math.max(gains[0], Math.max(gains[1], gains[2])));
		return Math.min(most, MAX_LIFT);
	}

	/**
	 * Sets the source's direct filter: a low-pass whose gain is the low band and whose high-frequency gain brings the
	 * high band down to its own value, with any `lift` already taken out of the source's own gain. OpenAL copies a
	 * filter when it is attached, so it is attached again each time.
	 */
	private void filter(int source, SourceState state, float[] gains, float lift) {
		if (state.filter == 0) {
			state.filter = EXTEfx.alGenFilters();
			EXTEfx.alFilteri(state.filter, EXTEfx.AL_FILTER_TYPE, EXTEfx.AL_FILTER_LOWPASS);
		}
		AL10.alSourcef(source, EXTEfx.AL_AIR_ABSORPTION_FACTOR, airAbsorption);
		float low = Math.clamp(gains[0] / lift, 0f, 1f);
		float highRatio = low > 0 ? Math.clamp(gains[2] / gains[0], 0f, 1f) : 0f;
		EXTEfx.alFilterf(state.filter, EXTEfx.AL_LOWPASS_GAIN, low);
		EXTEfx.alFilterf(state.filter, EXTEfx.AL_LOWPASS_GAINHF, highRatio);
		AL10.alSourcei(source, EXTEfx.AL_DIRECT_FILTER, state.filter);
	}

	/**
	 * Puts a game sound on realistic falloff (1/distance past {@value #REFERENCE_DISTANCE} blocks) or back on the
	 * game's linear one, and returns the extra gain realistic falloff needs: a fade to silence over the last quarter
	 * of the sound's range, so it doesn't stop dead where the game stops it.
	 */
	private float falloff(int source, SourceState state, float range, float distance) {
		boolean realistic = realisticFalloff;
		if (realistic != state.realistic) {
			state.realistic = realistic;
			AL10.alSourcei(source, AL10.AL_DISTANCE_MODEL,
					realistic ? AL11.AL_INVERSE_DISTANCE_CLAMPED : AL11.AL_LINEAR_DISTANCE);
			AL10.alSourcef(source, AL10.AL_REFERENCE_DISTANCE, realistic ? REFERENCE_DISTANCE : 0);
			AL10.alSourcef(source, AL10.AL_ROLLOFF_FACTOR, 1);
			AL10.alSourcef(source, AL10.AL_MAX_DISTANCE, realistic ? range * REACH : range);
		}
		return realistic ? fade(range * REACH, distance) : 1;
	}

	/** 1 until the last {@value #FADE_SHARE} of {@code range}, then down to 0 at it. */
	static float fade(float range, float distance) {
		return Math.clamp((range - distance) / (FADE_SHARE * range), 0f, 1f);
	}

	/**
	 * Whether {@code heard} holds a block of {@link NativeBridge#HEARD_DOUBLES} doubles for each of {@code count}
	 * sources: the update reads {@code heard[at]} through {@code heard[at + 7]} per source with no bounds checks,
	 * so a short array is caught once, here, rather than an
	 * {@link ArrayIndexOutOfBoundsException} part way through a source.
	 */
	static boolean heardHolds(double[] heard, int count) {
		return count >= 0 && (long) heard.length >= (long) NativeBridge.HEARD_DOUBLES * count;
	}

	/**
	 * Fails fast unless {@code room} is laid out as {@link NativeBridge#room} writes it: no more than
	 * {@link NativeBridge#ROOM_DOUBLES} doubles, so nothing reads or copies past its end, and enough to reach the
	 * last EAX parameter after {@link NativeBridge#EAX_FIRST}, which the reverb setters index straight out of it.
	 */
	static void requireRoom(double[] room) {
		int reach = NativeBridge.EAX_FIRST + EAX_PARAMETERS.length;
		if (room.length > NativeBridge.ROOM_DOUBLES || room.length < reach) {
			throw new IllegalStateException("room has " + room.length + " doubles: the layout takes "
					+ NativeBridge.ROOM_DOUBLES + ", its EAX parameters reaching index " + (reach - 1));
		}
	}

	private boolean earlyOn(boolean efx, boolean underwater) {
		return earlyReflections && efx && !underwater;
	}

	/**
	 * The early-reflections slot for each target (0 for none): the {@value EarlyReflections#SOURCES} loudest, by how
	 * much of their mid band gets through over how far they are.
	 */
	private int[] earlySlots(List<Target> targets, double[] heard, double[] listener, Vec3 ear, Axes axes,
			boolean efx, boolean underwater, boolean convolution, float[] orientation, int[][] ownSends) {
		int[] slots = new int[targets.size()];
		if (!earlyOn(efx, underwater)) {
			java.util.Arrays.fill(unusedOwn, true);
			return slots;
		}
		Integer[] order = new Integer[targets.size()];
		for (int i = 0; i < order.length; i++) order[i] = i;
		java.util.Arrays.sort(order, java.util.Comparator.comparingDouble(i -> -heard[NativeBridge.HEARD_DOUBLES * i + 1]
				/ Math.max(1, targets.get(i).position().distanceTo(ear))));
		// The loudest get reverbs of their own (with their early reflections in them); the next their early
		// reflections over the listener's reverb.
		int owned = convolution ? Math.min(order.length, OWN_REVERBS) : 0;
		int[] ownSlot = keep(ownOwners, order, 0, owned, targets);
		java.util.Arrays.fill(unusedOwn, true);
		for (int k = 0; k < owned; k++) {
			int s = ownSlot[k];
			unusedOwn[s] = false;
			Vec3 at = targets.get(order[k]).position();
			ownReverbs[s].tick(new double[] {at.x, at.y, at.z}, orientation);
			if (ownReverbs[s].playing()) ownSends[order[k]] = ownReverbs[s].slots();
		}
		int chosen = Math.min(order.length - owned, EarlyReflections.SOURCES);
		int[] earlySlot = keep(earlyOwners, order, owned, chosen, targets);
		Vec3[] positions = new Vec3[EarlyReflections.SOURCES];
		for (int k = 0; k < chosen; k++) positions[earlySlot[k]] = targets.get(order[owned + k]).position();
		int[] assigned = early.update(listener, positions, axes.right(), axes.up(), axes.forward());
		for (int k = 0; k < chosen; k++) slots[order[owned + k]] = assigned[earlySlot[k]];
		return slots;
	}

	/**
	 * Gives each of the {@code n} targets at {@code order[from…]} one of {@code owners}' slots: the one it had last
	 * update if it had one, else one freed. Records the new owners; returns each target's slot, by rank.
	 */
	private static int[] keep(int[] owners, Integer[] order, int from, int n, List<Target> targets) {
		int[] slotOf = new int[n];
		java.util.Arrays.fill(slotOf, -1);
		boolean[] taken = new boolean[owners.length];
		for (int k = 0; k < n; k++) {
			int source = targets.get(order[from + k]).source();
			for (int s = 0; s < owners.length; s++) {
				if (owners[s] == source && !taken[s]) {
					slotOf[k] = s;
					taken[s] = true;
					break;
				}
			}
		}
		for (int k = 0; k < n; k++) {
			if (slotOf[k] >= 0) continue;
			for (int s = 0; s < owners.length; s++) {
				if (!taken[s]) {
					slotOf[k] = s;
					taken[s] = true;
					break;
				}
			}
		}
		java.util.Arrays.fill(owners, 0);
		for (int k = 0; k < n; k++) {
			if (slotOf[k] < 0) continue;
			owners[slotOf[k]] = targets.get(order[from + k]).source();
		}
		return slotOf;
	}

	/** Sends a source to its own early reflections, unfiltered: they come by their own ways, not the direct one. */
	private static void sendEarly(int source, SourceState state, int slot) {
		if (slot != 0) {
			AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, slot, EarlyReflections.SEND, EXTEfx.AL_FILTER_NULL);
		} else if (state.earlySent) {
			AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, EXTEfx.AL_EFFECTSLOT_NULL, EarlyReflections.SEND,
					EXTEfx.AL_FILTER_NULL);
		}
		state.earlySent = slot != 0;
	}

	/**
	 * Sends a source to the reverb slots. What reaches the listener's space excites its reverb, so the send follows
	 * the direct gains, but softened (square root): a muffled sound behind a wall still fills the room a little.
	 */
	/**
	 * How much of another space's reverb gets through an opening of {@code aperture} square blocks: in proportion to
	 * its area, all of it from {@value #OPEN_PORTAL} up (a 4 × 4 arch), a door's 2 about -9 dB.
	 */
	static float portalShare(float aperture) {
		return Math.clamp(aperture / OPEN_PORTAL, 0.02f, 1f);
	}

	private static final float OPEN_PORTAL = 16;

	private static void send(int source, SourceState state, float[] gains, int[] slots, int spaceSlot,
			float portalShare) {
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
		// Another space's reverb reaches the listener the way the sound does, so it is muffled the same.
		if (spaceSlot != 0) {
			if (state.spaceFilter == 0) {
				state.spaceFilter = EXTEfx.alGenFilters();
				EXTEfx.alFilteri(state.spaceFilter, EXTEfx.AL_FILTER_TYPE, EXTEfx.AL_FILTER_LOWPASS);
			}
			// Through the opening the sound comes by, as much of that space's reverb as the opening's size lets out.
			float direct = Math.clamp(gains[0], 0f, 1f);
			EXTEfx.alFilterf(state.spaceFilter, EXTEfx.AL_LOWPASS_GAIN, direct * portalShare);
			EXTEfx.alFilterf(state.spaceFilter, EXTEfx.AL_LOWPASS_GAINHF,
					direct > 0 ? Math.clamp(gains[2] / direct, 0f, 1f) : 0f);
			AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, spaceSlot, SpaceReverb.SEND, state.spaceFilter);
		} else if (state.spaceSent) {
			AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, EXTEfx.AL_EFFECTSLOT_NULL, SpaceReverb.SEND,
					EXTEfx.AL_FILTER_NULL);
		}
		state.spaceSent = spaceSlot != 0;
	}

	/**
	 * Puts a source back where it is heard from after the game moved it, so a sound coming round a corner doesn't
	 * flick back to where it really is until the next update. Called on the sound thread.
	 */
	public static void reapplyPosition(ShapedChannel channel) {
		SourceState state = channel.earshot$state();
		Vec3 position = channel.earshot$position();
		// usable(null) covers the not-positional case: a point the native engine cannot use must not reach
		// OpenAL as a NaN position either (AL_INVALID_VALUE into the context's shared error state).
		if (!state.started() || !SourceState.usable(position) || !channel.earshot$positional()) return;
		Vec3 heardFrom = state.heardFrom(position);
		AL10.alSource3f(channel.earshot$source(), AL10.AL_POSITION, (float) heardFrom.x, (float) heardFrom.y,
				(float) heardFrom.z);
	}

	/** Frees a source's filters when the game destroys the source. Called on the sound thread. */
	public static void release(ShapedChannel channel) {
		releaseFilters(channel.earshot$source(), channel.earshot$state());
	}

	/**
	 * Takes a source's filters off it, then frees them: OpenAL won't delete a filter its source still holds, so
	 * the attachments are cleared while the source is still alive. Called on the sound thread.
	 */
	static void releaseFilters(int source, SourceState state) {
		if (state.filter != 0) {
			AL10.alSourcei(source, EXTEfx.AL_DIRECT_FILTER, EXTEfx.AL_FILTER_NULL);
		}
		if (state.sendFilter != 0) {
			// The send filter is on the listener's reverb sends, 0 and 1.
			AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, EXTEfx.AL_EFFECTSLOT_NULL, 0,
					EXTEfx.AL_FILTER_NULL);
			AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, EXTEfx.AL_EFFECTSLOT_NULL, 1,
					EXTEfx.AL_FILTER_NULL);
		}
		if (state.spaceFilter != 0) {
			AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, EXTEfx.AL_EFFECTSLOT_NULL, SpaceReverb.SEND,
					EXTEfx.AL_FILTER_NULL);
		}
		releaseFilters(state);
	}

	/** Frees the filters made for a source, once nothing holds them. Called on the sound thread. */
	static void releaseFilters(SourceState state) {
		if (state.filter != 0) {
			EXTEfx.alDeleteFilters(state.filter);
			state.filter = 0;
		}
		if (state.sendFilter != 0) {
			EXTEfx.alDeleteFilters(state.sendFilter);
			state.sendFilter = 0;
		}
		if (state.spaceFilter != 0) {
			EXTEfx.alDeleteFilters(state.spaceFilter);
			state.spaceFilter = 0;
		}
		// A delete that failed leaves its error in the context's shared state, where the next call could
		// misread it as its own: drain it here.
		AL10.alGetError();
	}

	private boolean call(Runnable call) {
		try {
			call.run();
			return true;
		} catch (EarshotException e) {
			failed = true;
			EarshotClient.LOGGER.error("Sound shaping stopped: a native call failed", e);
			voice.fail();
			return false;
		}
	}

	/**
	 * Runs one whole shaping pass ({@link #shape}), catching any {@link RuntimeException} that escapes it. The
	 * typed native failures are handled inside the pass by {@link #call}, but anything else — a broken contract
	 * like {@link #requireRoom}, an unguarded native call, a bad cast — would escape to the sound thread's
	 * executor, which logs "Error executing task" and carries on, leaving the pass half-applied (sends set on
	 * some sources, spaces.begin() with no matching end(), unusedOwn never released) and queueing the same
	 * failure every tick. Nothing is rethrown: on failure {@code onFail} has already turned shaping off. Returns
	 * whether the pass ran.
	 */
	static boolean shapeSafely(Runnable pass, Runnable onFail) {
		try {
			pass.run();
			return true;
		} catch (RuntimeException e) {
			EarshotClient.LOGGER.error("Sound shaping stopped: the shaping pass threw", e);
			onFail.run();
			return false;
		}
	}
}
