package io.github.cumsauce.earshot.audio;

import io.github.cumsauce.earshot.EarshotClient;
import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.openal.OpenAlProbe;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import net.fabricmc.loader.api.FabricLoader;
import net.minecraft.world.phys.Vec3;
import org.jspecify.annotations.Nullable;
import org.lwjgl.openal.AL10;
import org.lwjgl.openal.AL11;
import org.lwjgl.openal.EXTEfx;
import org.lwjgl.openal.EXTFloat32;
import org.lwjgl.openal.SOFTCallbackBuffer;

/**
 * Plays Yak VC's voices through the game's OpenAL, so they get the same acoustics as every other sound (M4,
 * desocketed/yakvc#174).
 *
 * <p>Earshot's native library is a {@code YakVcVoiceSink}, offered to Yak VC in Fabric's object share: Yak VC hands it
 * each speaker's decoded frames, which it queues per speaker. Here each speaker heard lately gets an OpenAL source
 * playing a callback buffer, which OpenAL's mixer fills straight from that queue, natively. Once a tick the sources are
 * put where the speakers are, and {@link SoundShaper} shapes the positioned ones like any other sound. Group voice and
 * calls play centred and dry, as Yak VC plays them.
 */
public final class VoicePlayer {
	/** The object share key Yak VC looks for a voice sink under. */
	public static final String SINK_KEY = "yakvc:voice_sink";
	/** Where Yak VC publishes the newest sink version it takes (an {@link Integer}); a Yak VC without it takes 1. */
	public static final String SINK_VERSION_KEY = "yakvc:voice_sink_version";
	/** Yak VC's {@code YAKVC_VOICE_UNPOSITIONED}: group voice and calls, which play centred. */
	private static final int UNPOSITIONED = 1 << 1;
	/** Yak VC's {@code YAKVC_VOICE_SELF}: the player's own voice, which only a version 2 sink gets. */
	private static final int SELF = 1 << 2;
	/** How much of the player's own voice goes to the room's reverb; none of it is played dry. */
	private static final float OWN_VOICE_SEND = 0.5f;
	/**
	 * Speech carries best ahead of the mouth: behind and to the sides a voice is a little quieter and much duller.
	 * OpenAL's cone angles are the full width of the cone, not a half-angle from the facing: the inner
	 * {@value #CONE_INNER} keeps the voice full within 45 degrees of where the speaker faces, and the outer
	 * {@value #CONE_OUTER} ends the fade at 135 degrees from the facing — 45 short of directly behind — where the
	 * gain reaches {@value #BEHIND_GAIN} and stays, its highs at {@value #BEHIND_HIGHS} (about -4 dB and -12 dB,
	 * as measured for human speech around 2 to 4 kHz).
	 */
	private static final float CONE_INNER = 90;
	private static final float CONE_OUTER = 270;
	private static final float BEHIND_GAIN = 0.63f;
	private static final float BEHIND_HIGHS = 0.25f;

	/** Where a player's eyes are and which way their head faces, for placing and pointing their voice. */
	public record Head(Vec3 eye, Vec3 facing) {}

	/** After offering the version 2 sink, Yak VC must have asked it for a frame within this long, or it refused it. */
	private static final long REFUSAL_NANOS = 3_000_000_000L;
	/** A speaker silent this long has its source let go; their next frame makes another. */
	private static final double IDLE_SECONDS = 2;
	/** Voice is at full volume this close, as Yak VC's speakers play it by default. */
	private static final float FULL_VOLUME_DISTANCE = 4;
	/** How far a voice fades out when Yak VC supplies no usable range (the default range elsewhere in Earshot). */
	private static final float DEFAULT_MAX_DISTANCE = 16;
	private static final int SAMPLE_RATE = 48_000;

	private final NativeBridge bridge;
	private final long callback;
	private volatile boolean enabled = true;
	private volatile boolean ownVoice;
	/** The installed Yak VC refused the version 2 sink: it predates own voice. */
	private volatile boolean ownVoiceRefused;
	/** The sink version offered (0 for none), when, and Yak VC's poll count then. */
	private int offeredVersion;
	private long offeredAt;
	private long pollsAtOffer;
	/** Whether the game's OpenAL plays callback buffers; known once it opens. */
	private volatile boolean supported;
	private volatile boolean offered;
	private volatile List<Status> status = List.of();

	// Guarded by `this`; OpenAL calls happen on the sound thread.
	private final Map<UUID, Voice> voices = new HashMap<>();
	private boolean failed;

	/** One speaker being played. */
	static final class Voice {
		final UUID speaker;
		final int source;
		final int buffer;
		final SourceState state = new SourceState();
		/** Where the speaker is, or null while the voice plays centred. */
		@Nullable Vec3 position;
		boolean centred;
		/** The gain set on the source, or -1 before one is. */
		float volume = -1;
		/** The player's own voice, which only feeds the room's reverb. */
		boolean own;
		/** Whether the voice points the way its speaker faces (else it carries the same all round). */
		boolean directional;
		/** As shaped in the last update. */
		@Nullable Vec3 heardFrom;
		float[] gains = {1, 1, 1};
		double queuedSeconds;

		Voice(UUID speaker, int source, int buffer) {
			this.speaker = speaker;
			this.source = source;
			this.buffer = buffer;
		}
	}

	/**
	 * One speaker's voice, for the debug command and tests.
	 *
	 * @param position  where the speaker is, or null when centred
	 * @param heardFrom where the voice seems to come from, or null when centred
	 */
	public record Status(UUID speaker, boolean playing, @Nullable Vec3 position, @Nullable Vec3 heardFrom,
			float[] gains, double queuedSeconds, boolean own, boolean directional) {
		@Override
		public String toString() {
			return "Status[speaker=" + speaker + ", playing=" + playing + ", position=" + position + ", heardFrom="
					+ heardFrom + ", gains=" + java.util.Arrays.toString(gains) + ", queuedSeconds=" + queuedSeconds
					+ ", own=" + own + ", directional=" + directional + "]";
		}
	}

	public VoicePlayer(NativeBridge bridge) {
		this.bridge = bridge;
		this.callback = bridge.voiceCallback();
	}

	/** The voices playing as of the last update. */
	public List<Status> status() {
		return status;
	}

	/** Whether Yak VC's voice is offered to come through Earshot. */
	public boolean offered() {
		return offered;
	}

	/**
	 * Takes Yak VC's voice through Earshot, or gives it back to Yak VC's own speakers; and with {@code ownVoice}, asks
	 * for the player's own voice too, for the room to answer.
	 */
	public void configure(boolean enabled, boolean ownVoice) {
		this.enabled = enabled;
		this.ownVoice = ownVoice;
		offer();
	}

	/** Whether the installed Yak VC refused to send the player's own voice. */
	public boolean ownVoiceRefused() {
		return ownVoiceRefused;
	}

	/** The game opened OpenAL: Yak VC's voice comes through Earshot if this OpenAL can play it. */
	public void openAlOpened(OpenAlProbe.@Nullable Report report) {
		supported = report != null && report.callbackBuffer();
		if (!supported) {
			EarshotClient.LOGGER.warn("This OpenAL has no callback buffers, so Yak VC's voice stays on its own speakers");
		}
		offer();
	}

	/** Sound shaping stopped: give Yak VC's voice back to its own speakers. */
	synchronized void fail() {
		failed = true;
		stopAll();
		offer();
	}

	/** The game is closing its OpenAL context, which frees every source in it: forget them. */
	public synchronized void contextClosing() {
		voices.clear();
		status = List.of();
		// A fresh context gets a fresh chance: what failed against the old one says nothing about the new.
		failed = false;
		// And what was offered to the old context must be offered again: the sink handle from it is dead, so the
		// next openAlOpened() has to republish rather than early-return on the version it already had.
		offeredVersion = -1;
		offered = false;
	}

	/**
	 * Offers the voice sink to Yak VC in the object share, or takes it back. Yak VC looks there every tick and moves
	 * its voice over.
	 */
	private synchronized void offer() {
		boolean want = enabled && supported && !failed;
		int version = !want ? 0 : ownVoice && yakvcTakes(2) ? 2 : 1;
		if (version == offeredVersion) return;
		// Before the share changes: a Yak VC that polls the sink as it is registered must not count towards the baseline.
		pollsAtOffer = bridge.voicePolls();
		bridge.setOwnVoice(version == 2);
		var share = FabricLoader.getInstance().getObjectShare();
		if (want) {
			share.put(SINK_KEY, bridge.voiceSink(version));
		} else {
			share.remove(SINK_KEY);
		}
		offered = want;
		offeredVersion = version;
		offeredAt = System.nanoTime();
		EarshotClient.LOGGER.info(!want ? "Withdrew the voice sink: Yak VC's voice plays through its own speakers"
				: version == 2 ? "Offered Yak VC a voice sink with own voice: voice plays through Earshot"
				: "Offered Yak VC a voice sink: voice plays through Earshot");
	}

	/**
	 * Whether the installed Yak VC takes a sink of {@code version}: as it publishes, or for a Yak VC that doesn't, as
	 * long as it hasn't been seen to refuse one.
	 */
	private boolean yakvcTakes(int version) {
		if (FabricLoader.getInstance().getObjectShare().get(SINK_VERSION_KEY) instanceof Integer newest) {
			return version <= newest;
		}
		return version == 1 || !ownVoiceRefused;
	}

	/**
	 * A Yak VC from before own voice refuses the version 2 sink and keeps voice on its speakers: once it has had time
	 * to take it and hasn't asked it for a single frame, offer version 1 instead.
	 */
	private void checkRefusal() {
		if (offeredVersion != 2 || System.nanoTime() - offeredAt < REFUSAL_NANOS) return;
		// A Yak VC that says which versions it takes was offered one it takes, as long as the version it publishes is
		// at least the one offered: an older one is below it, and the downgrade can still happen.
		if (FabricLoader.getInstance().getObjectShare().get(SINK_VERSION_KEY) instanceof Integer newest
				&& newest >= offeredVersion) return;
		if (!FabricLoader.getInstance().isModLoaded("yakvc") || bridge.voicePolls() != pollsAtOffer) return;
		ownVoiceRefused = true;
		EarshotClient.LOGGER.warn("This Yak VC doesn't send the player's own voice, so Own Voice Echo is off");
		offer();
	}

	/**
	 * On the sound thread: gives each speaker heard lately a playing source, lets go of the rest, and returns the
	 * positioned voices for {@link SoundShaper} to shape, with their positions set.
	 *
	 * @param players the eye of every player in the world, by UUID
	 * @param volume  the game's Voice/Speech volume, with Master: Yak VC's frames come without it
	 */
	synchronized List<Voice> update(Map<UUID, Head> players, float volume, int[] sends) {
		if (!offered) {
			if (!voices.isEmpty()) stopAll();
			return List.of();
		}
		checkRefusal();
		List<Voice> positioned = new ArrayList<>();
		Set<UUID> heard = new HashSet<>();
		for (NativeBridge.VoiceStream stream : bridge.voiceStreams()) {
			if (stream.ended() || stream.secondsSinceFrame() > IDLE_SECONDS) continue;
			boolean centred = (stream.flags() & UNPOSITIONED) != 0;
			Head head = players.get(stream.speaker());
			Vec3 eye = null;
			if (!centred) {
				double[] at = stream.heardFrom();
				eye = head != null ? head.eye() : new Vec3(at[0], at[1], at[2]);
				// A speaker somewhere the native engine cannot use (as Vec3::is_usable: NaN, infinite, or
				// past 1e9) is not heard this tick: the stream is skipped before its source is made or
				// placed, so the bad point never reaches hear(), and any voice it had is let go below, as
				// for a speaker gone quiet.
				if (!SourceState.usable(eye)) continue;
			}
			Voice voice = voices.get(stream.speaker());
			if (voice == null) {
				voice = start(stream.speaker());
				// Out of sources for now: this one waits, the others carry on (stopping early would cut them off).
				if (voice == null) continue;
				voices.put(stream.speaker(), voice);
			}
			heard.add(stream.speaker());
			voice.queuedSeconds = stream.queuedSeconds();
			if (voice.volume != volume) {
				AL10.alSourcef(voice.source, AL10.AL_GAIN, volume);
				voice.volume = volume;
			}
			if (AL10.alGetSourcei(voice.source, AL10.AL_SOURCE_STATE) != AL10.AL_PLAYING) {
				AL10.alSourcePlay(voice.source);
			}
			if (centred) {
				centre(voice);
				if ((stream.flags() & SELF) != 0) reverbOnly(voice, sends);
				continue;
			}
			place(voice, eye, stream.range());
			point(voice, head == null ? null : head.facing());
			positioned.add(voice);
		}
		voices.values().removeIf(voice -> {
			if (heard.contains(voice.speaker)) return false;
			stop(voice);
			return true;
		});
		return positioned;
	}

	/** Publishes the voices as they were shaped in this update. */
	synchronized void report() {
		List<Status> out = new ArrayList<>(voices.size());
		for (Voice voice : voices.values()) {
			boolean playing = AL10.alGetSourcei(voice.source, AL10.AL_SOURCE_STATE) == AL10.AL_PLAYING;
			out.add(new Status(voice.speaker, playing, voice.position, voice.centred ? null : voice.heardFrom,
					voice.gains.clone(), voice.queuedSeconds, voice.own, voice.directional));
		}
		status = List.copyOf(out);
	}

	/**
	 * A source playing {@code speaker}'s stream, or null (and voice withdrawn) if OpenAL won't make one.
	 *
	 * <p>It never latches {@link #failed}: the AL error window is ambiguous (another thread's error in the gap is
	 * indistinguishable from our own), so an error is logged and null is returned instead of failing every voice.
	 */
	private @Nullable Voice start(UUID speaker) {
		// Java-only work stays outside the error window: from the drain to the check below this thread makes
		// OpenAL calls and nothing else, so no call of ours outside the window can be blamed on it.
		long stream = bridge.voiceStream(speaker);
		// OpenAL's error is the context's, shared with the sound thread: clear what it left.
		AL10.alGetError();
		int buffer = AL10.alGenBuffers();
		SOFTCallbackBuffer.nalBufferCallbackSOFT(buffer, EXTFloat32.AL_FORMAT_MONO_FLOAT32, SAMPLE_RATE, callback,
				stream);
		int source = AL10.alGenSources();
		AL10.alSourcei(source, AL10.AL_BUFFER, buffer);
		AL10.alSourcePlay(source);
		int error = AL10.alGetError();
		if (error == AL10.AL_NO_ERROR) return new Voice(speaker, source, buffer);
		if (AL10.alIsSource(source)) AL10.alDeleteSources(source);
		if (AL10.alIsBuffer(buffer)) AL10.alDeleteBuffers(buffer);
		AL10.alGetError();
		if (error == AL10.AL_OUT_OF_MEMORY) {
			// Out of sources: the game has them all for now. Try again next tick.
			return null;
		}
		// The window holds only this thread's own calls, but OpenAL's error state is the context's: an error
		// another thread raised in the gap is indistinguishable from our own. One such ambiguous error must
		// not stop every voice for the rest of the session — it is logged, and this speaker tries again next tick.
		EarshotClient.LOGGER.warn("Yak VC's voice goes back to its own speakers: OpenAL won't play it (error 0x{})",
				Integer.toHexString(error));
		return null;
	}

	/** Puts a voice at {@code eye}, fading out towards {@code range} as Yak VC's speakers do. */
	private static void place(Voice voice, Vec3 eye, float range) {
		unOwnVoice(voice);
		if (voice.position == null || voice.centred) {
			AL10.alSourcei(voice.source, AL10.AL_SOURCE_RELATIVE, AL10.AL_FALSE);
			AL10.alSourcei(voice.source, AL10.AL_DISTANCE_MODEL, AL11.AL_LINEAR_DISTANCE_CLAMPED);
			AL10.alSourcef(voice.source, AL10.AL_REFERENCE_DISTANCE, FULL_VOLUME_DISTANCE);
			AL10.alSourcef(voice.source, AL10.AL_ROLLOFF_FACTOR, 1);
			voice.centred = false;
		}
		AL10.alSourcef(voice.source, AL10.AL_MAX_DISTANCE, maxDistance(range));
		voice.position = eye;
	}

	/**
	 * Takes a voice off the player's-own-voice setup, once its stream stops being unpositioned and it is placed:
	 * {@link #reverbOnly} latched {@code own} and gave the source a direct filter whose gain is 0, so none of it
	 * played dry, and SoundShaper leaves such a voice's filters and sends alone ({@code ownVoice}). Left latched,
	 * that would silence the placed voice's dry sound for the rest of its stream's life and leave it sounding only
	 * into the reverb, unshaped, so here the latch goes and the
	 * filters {@link #reverbOnly} set come off and go; SoundShaper makes its own again and shapes the voice like
	 * any other from this same update on.
	 */
	private static void unOwnVoice(Voice voice) {
		if (!voice.own) return;
		voice.own = false;
		// The filters reverbOnly set go entirely, as stop() frees them: SoundShaper.releaseFilters detaches the
		// direct and send filters from the source (a filter OpenAL still holds cannot be deleted) and deletes
		// them, zeroing the state's names. SoundShaper makes them again this same update — send() setting the
		// room rolloff it only sets at creation, filter() the gains — so the voice comes back shaped like any
		// other, with neither its own-voice silence nor a rolloff left unset behind it.
		SoundShaper.releaseFilters(voice.source, voice.state);
	}

	/**
	 * How far {@code range} fades the voice out, or 16 when Yak VC sends no usable range: a NaN would raise
	 * AL_INVALID_VALUE into the context's shared error state, where a later bracketed caller could blame its own
	 * window for it.
	 */
	static float maxDistance(float range) {
		return Float.isFinite(range) && range > 0 ? Math.max(range, FULL_VOLUME_DISTANCE + 1) : DEFAULT_MAX_DISTANCE;
	}

	/**
	 * The direction a voice may point, or null when {@code facing} is not usable as {@link SourceState#usable}
	 * judges: a NaN or infinite coordinate raises AL_INVALID_VALUE into the context's shared error state, where
	 * a later bracketed caller could blame its own window for it.
	 */
	static @Nullable Vec3 direction(@Nullable Vec3 facing) {
		return SourceState.usable(facing) ? facing : null;
	}

	/** Points a voice the way its speaker faces, or all round when that isn't known or usable. */
	private static void point(Voice voice, @Nullable Vec3 facing) {
		Vec3 direction = direction(facing);
		if (direction == null) {
			if (voice.directional) AL10.alSource3f(voice.source, AL10.AL_DIRECTION, 0, 0, 0);
			voice.directional = false;
			return;
		}
		if (!voice.directional) {
			AL10.alSourcef(voice.source, AL10.AL_CONE_INNER_ANGLE, CONE_INNER);
			AL10.alSourcef(voice.source, AL10.AL_CONE_OUTER_ANGLE, CONE_OUTER);
			AL10.alSourcef(voice.source, AL10.AL_CONE_OUTER_GAIN, BEHIND_GAIN);
			AL10.alSourcef(voice.source, EXTEfx.AL_CONE_OUTER_GAINHF, BEHIND_HIGHS);
			voice.directional = true;
		}
		AL10.alSource3f(voice.source, AL10.AL_DIRECTION, (float) direction.x, (float) direction.y,
				(float) direction.z);
	}

	/**
	 * Sends the player's own voice to the room's reverb ({@code sends}, by send index) and plays none of it dry: the
	 * player hears their voice already, and the room answering it.
	 */
	private static void reverbOnly(Voice voice, int[] sends) {
		SourceState state = voice.state;
		if (!voice.own) {
			voice.own = true;
			// Made only when the state has none: a voice shaped while placed keeps its filters when it is
			// centred again, and regenerating here would leak the old names. Both gains are set either way,
			// since a reused filter carries what SoundShaper last shaped on it (a fresh low-pass rests at
			// gain and high-frequency gain 1).
			if (state.filter == 0) {
				state.filter = EXTEfx.alGenFilters();
				EXTEfx.alFilteri(state.filter, EXTEfx.AL_FILTER_TYPE, EXTEfx.AL_FILTER_LOWPASS);
			}
			EXTEfx.alFilterf(state.filter, EXTEfx.AL_LOWPASS_GAIN, 0f);
			EXTEfx.alFilterf(state.filter, EXTEfx.AL_LOWPASS_GAINHF, 1f);
			if (state.sendFilter == 0) {
				state.sendFilter = EXTEfx.alGenFilters();
				EXTEfx.alFilteri(state.sendFilter, EXTEfx.AL_FILTER_TYPE, EXTEfx.AL_FILTER_LOWPASS);
			}
			EXTEfx.alFilterf(state.sendFilter, EXTEfx.AL_LOWPASS_GAIN, OWN_VOICE_SEND);
			EXTEfx.alFilterf(state.sendFilter, EXTEfx.AL_LOWPASS_GAINHF, 1f);
		}
		AL10.alSourcei(voice.source, EXTEfx.AL_DIRECT_FILTER, state.filter);
		for (int send = 0; send < sends.length; send++) {
			AL11.alSource3i(voice.source, EXTEfx.AL_AUXILIARY_SEND_FILTER, sends[send], send,
					sends[send] == 0 ? EXTEfx.AL_FILTER_NULL : state.sendFilter);
		}
		voice.gains = new float[] {0, 0, 0};
	}

	/** Plays a voice centred and dry, as Yak VC plays group voice and calls. */
	private static void centre(Voice voice) {
		voice.position = null;
		if (voice.centred) return;
		voice.centred = true;
		point(voice, null);
		AL10.alSourcei(voice.source, AL10.AL_SOURCE_RELATIVE, AL10.AL_TRUE);
		AL10.alSource3f(voice.source, AL10.AL_POSITION, 0, 0, 0);
		AL10.alSourcei(voice.source, AL10.AL_DISTANCE_MODEL, AL10.AL_NONE);
		AL10.alSourcei(voice.source, EXTEfx.AL_DIRECT_FILTER, EXTEfx.AL_FILTER_NULL);
		// Every send goes, sends 0 and 1 like before and the space (SpaceReverb.SEND) and early
		// reflections (EarlyReflections.SEND) sends too: a centred voice leaves SoundShaper's target
		// list, so nothing later clears those, and the dry voice would keep exciting them for its lifetime.
		for (int send = 0; send <= Math.max(SpaceReverb.SEND, EarlyReflections.SEND); send++) {
			AL11.alSource3i(voice.source, EXTEfx.AL_AUXILIARY_SEND_FILTER, EXTEfx.AL_EFFECTSLOT_NULL, send,
					EXTEfx.AL_FILTER_NULL);
		}
		voice.state.spaceSent = false;
		voice.state.earlySent = false;
		voice.gains = new float[] {1, 1, 1};
	}

	private void stopAll() {
		voices.values().forEach(VoicePlayer::stop);
		voices.clear();
		status = List.of();
	}

	/** Stops a voice and frees its source, buffer and filters. */
	private static void stop(Voice voice) {
		AL10.alSourceStop(voice.source);
		AL10.alSourcei(voice.source, AL10.AL_BUFFER, 0);
		// Filters must come off before the source goes: the int+state overload detaches the direct, send
		// and space filters from the source, which a deleted source cannot be detached from.
		SoundShaper.releaseFilters(voice.source, voice.state);
		AL10.alDeleteSources(voice.source);
		AL10.alDeleteBuffers(voice.buffer);
	}
}
