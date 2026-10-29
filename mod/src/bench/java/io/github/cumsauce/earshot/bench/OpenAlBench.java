package io.github.cumsauce.earshot.bench;

import static org.lwjgl.system.MemoryUtil.NULL;

import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.FloatBuffer;
import java.nio.IntBuffer;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.SplittableRandom;
import org.lwjgl.BufferUtils;
import org.lwjgl.openal.AL;
import org.lwjgl.openal.AL10;
import org.lwjgl.openal.AL11;
import org.lwjgl.openal.ALC;
import org.lwjgl.openal.ALC10;
import org.lwjgl.openal.ALCCapabilities;
import org.lwjgl.openal.EXTEfx;
import org.lwjgl.openal.SOFTHRTF;
import org.lwjgl.openal.SOFTLoopback;

/**
 * M0 spike (docs/PLAN.md): what OpenAL Soft's rendering costs with the features Earshot plans to use, measured
 * with no audio device by rendering a loopback device as fast as possible on one thread.
 *
 * <p>Each configuration plays looping noise from moving sources around the listener and renders {@value #SECONDS} s
 * of 48 kHz stereo. The share of one core it took is the cost of that mix in the game. It also times swapping a
 * convolution slot's impulse response, which Earshot does whenever the room changes, less the run's per-block render
 * cost (applying the swap takes one block's render, which the share already paid for).
 */
public final class OpenAlBench {
	private static final int RATE = 48_000;
	private static final int BLOCK = 1024;
	private static final int SECONDS = 20;
	/** AL_SOFT_convolution_effect, still in-progress, so not in LWJGL. */
	private static final int AL_EFFECT_CONVOLUTION_SOFT = 0xA000;
	private static final String CONVOLUTION_EXTENSION = "AL_SOFTX_convolution_effect";
	private static final int AL_FORMAT_MONO_FLOAT32 = 0x10010;
	private static final int AL_FORMAT_BFORMAT3D_FLOAT32 = 0x20033;

	private record Config(String name, int sources, boolean hrtf, boolean filters, int eaxSlots, int convolutionSlots,
			double irSeconds) {}

	private record Result(Config config, double coreShare, double swapMillis, String note) {}

	public static void main(String[] args) throws IOException {
		long device = SOFTLoopback.alcLoopbackOpenDeviceSOFT((ByteBuffer) null);
		if (device == NULL) throw new IllegalStateException("no OpenAL loopback device");
		ALCCapabilities alcCaps = ALC.createCapabilities(device);
		if (!alcCaps.ALC_SOFT_loopback || !alcCaps.ALC_EXT_EFX) {
			throw new IllegalStateException("OpenAL lacks ALC_SOFT_loopback or ALC_EXT_EFX");
		}

		List<Config> configs = List.of(
				new Config("dry, stereo panning", 32, false, false, 0, 0, 0),
				new Config("dry, HRTF", 32, true, false, 0, 0, 0),
				new Config("HRTF + occlusion filters", 32, true, true, 0, 0, 0),
				new Config("HRTF + filters + 1 EAX reverb", 32, true, true, 1, 0, 0),
				new Config("HRTF + filters + 2 EAX reverbs", 32, true, true, 2, 0, 0),
				new Config("HRTF + filters + 1 convolution (1.5 s IR)", 32, true, true, 0, 1, 1.5),
				new Config("HRTF + filters + 2 convolutions (1.5 s IR)", 32, true, true, 0, 2, 1.5),
				new Config("HRTF + filters + 2 convolutions (3 s IR)", 32, true, true, 0, 2, 3.0),
				new Config("HRTF + filters + 2 convolutions (1.5 s IR)", 64, true, true, 0, 2, 1.5));

		String version = null;
		List<Result> results = new ArrayList<>();
		for (Config config : configs) {
			long context = createContext(device, config.hrtf);
			AL.createCapabilities(alcCaps);
			if (version == null) {
				version = AL10.alGetString(AL10.AL_VERSION) + " (" + AL10.alGetString(AL10.AL_RENDERER) + ")";
			}
			try {
				results.add(run(device, config));
			} finally {
				ALC10.alcMakeContextCurrent(NULL);
				ALC10.alcDestroyContext(context);
			}
		}
		ALC10.alcCloseDevice(device);
		report(version, results);
	}

	private static long createContext(long device, boolean hrtf) {
		IntBuffer attrs = BufferUtils.createIntBuffer(11).put(new int[] {
				SOFTLoopback.ALC_FORMAT_CHANNELS_SOFT, SOFTLoopback.ALC_STEREO_SOFT,
				SOFTLoopback.ALC_FORMAT_TYPE_SOFT, SOFTLoopback.ALC_FLOAT_SOFT,
				ALC10.ALC_FREQUENCY, RATE,
				SOFTHRTF.ALC_HRTF_SOFT, hrtf ? ALC10.ALC_TRUE : ALC10.ALC_FALSE,
				EXTEfx.ALC_MAX_AUXILIARY_SENDS, 4,
				0}).flip();
		long context = ALC10.alcCreateContext(device, attrs);
		if (context == NULL || !ALC10.alcMakeContextCurrent(context)) {
			throw new IllegalStateException("cannot create a loopback context");
		}
		return context;
	}

	private static Result run(long device, Config config) {
		SplittableRandom random = new SplittableRandom(42);
		String note = "";
		if (config.hrtf && ALC10.alcGetInteger(device, SOFTHRTF.ALC_HRTF_STATUS_SOFT) != SOFTHRTF.ALC_HRTF_ENABLED_SOFT) {
			note = "HRTF did not turn on";
		}
		boolean convolution = AL10.alIsExtensionPresent(CONVOLUTION_EXTENSION);
		if (config.convolutionSlots > 0 && !convolution) {
			return new Result(config, Double.NaN, Double.NaN, CONVOLUTION_EXTENSION + " missing");
		}

		int noise = AL10.alGenBuffers();
		FloatBuffer samples = BufferUtils.createFloatBuffer(RATE);
		for (int i = 0; i < RATE; i++) samples.put((float) (random.nextDouble() * 0.2 - 0.1));
		AL10.alBufferData(noise, AL_FORMAT_MONO_FLOAT32, samples.flip(), RATE);

		List<Integer> slots = new ArrayList<>();
		List<Integer> effects = new ArrayList<>();
		List<Integer> irs = new ArrayList<>();
		for (int i = 0; i < config.eaxSlots; i++) {
			int effect = EXTEfx.alGenEffects();
			EXTEfx.alEffecti(effect, EXTEfx.AL_EFFECT_TYPE, EXTEfx.AL_EFFECT_EAXREVERB);
			EXTEfx.alEffectf(effect, EXTEfx.AL_EAXREVERB_DECAY_TIME, 2.5f);
			int slot = EXTEfx.alGenAuxiliaryEffectSlots();
			EXTEfx.alAuxiliaryEffectSloti(slot, EXTEfx.AL_EFFECTSLOT_EFFECT, effect);
			effects.add(effect);
			slots.add(slot);
		}
		for (int i = 0; i < config.convolutionSlots; i++) {
			int ir = impulseResponse(config.irSeconds, random);
			int effect = EXTEfx.alGenEffects();
			EXTEfx.alEffecti(effect, EXTEfx.AL_EFFECT_TYPE, AL_EFFECT_CONVOLUTION_SOFT);
			int slot = EXTEfx.alGenAuxiliaryEffectSlots();
			// The order OpenAL Soft's alconvolve example uses: buffer, then effect.
			EXTEfx.alAuxiliaryEffectSloti(slot, AL10.AL_BUFFER, ir);
			EXTEfx.alAuxiliaryEffectSloti(slot, EXTEfx.AL_EFFECTSLOT_EFFECT, effect);
			irs.add(ir);
			effects.add(effect);
			slots.add(slot);
		}
		checkError("setting up effect slots");

		int filter = 0;
		if (config.filters) {
			filter = EXTEfx.alGenFilters();
			EXTEfx.alFilteri(filter, EXTEfx.AL_FILTER_TYPE, EXTEfx.AL_FILTER_LOWPASS);
			EXTEfx.alFilterf(filter, EXTEfx.AL_LOWPASS_GAIN, 0.7f);
			EXTEfx.alFilterf(filter, EXTEfx.AL_LOWPASS_GAINHF, 0.2f);
		}

		int[] sources = new int[config.sources];
		double[] angles = new double[config.sources];
		double[] radii = new double[config.sources];
		for (int i = 0; i < sources.length; i++) {
			int source = AL10.alGenSources();
			AL10.alSourcei(source, AL10.AL_BUFFER, noise);
			AL10.alSourcei(source, AL10.AL_LOOPING, AL10.AL_TRUE);
			AL10.alSourcef(source, AL10.AL_GAIN, 0.3f);
			if (filter != 0) AL10.alSourcei(source, EXTEfx.AL_DIRECT_FILTER, filter);
			for (int send = 0; send < slots.size(); send++) {
				AL11.alSource3i(source, EXTEfx.AL_AUXILIARY_SEND_FILTER, slots.get(send), send, filter);
			}
			angles[i] = random.nextDouble() * Math.PI * 2;
			radii[i] = 2 + random.nextDouble() * 18;
			sources[i] = source;
		}
		checkError("setting up sources");
		for (int source : sources) AL10.alSourcePlay(source);

		ByteBuffer out = BufferUtils.createByteBuffer(BLOCK * 2 * Float.BYTES).order(ByteOrder.nativeOrder());
		int blocks = SECONDS * RATE / BLOCK;
		render(device, out, sources, angles, radii, RATE / BLOCK);
		long start = System.nanoTime();
		render(device, out, sources, angles, radii, blocks);
		double elapsed = (System.nanoTime() - start) / 1e9;
		double audio = (double) blocks * BLOCK / RATE;

		double swapMillis = Double.NaN;
		if (!irs.isEmpty()) {
			int fresh = impulseResponse(config.irSeconds, random);
			// The error window covers the swap alone, so a render error below can't be blamed on it.
			AL10.alGetError();
			long swapStart = System.nanoTime();
			EXTEfx.alAuxiliaryEffectSloti(slots.get(slots.size() - 1), AL10.AL_BUFFER, fresh);
			if (AL10.alGetError() != AL10.AL_NO_ERROR) note = "swapping an IR on a live slot failed";
			// The render is what applies the swap to the live mix; the run's per-block cost comes
			// back off, so the column reads as the swap and not the block around it.
			render(device, out, sources, angles, radii, 1);
			swapMillis = Math.max(0, (System.nanoTime() - swapStart) / 1e6 - elapsed / blocks * 1e3);
			irs.add(fresh);
		}

		for (int source : sources) AL10.alDeleteSources(source);
		for (int slot : slots) EXTEfx.alDeleteAuxiliaryEffectSlots(slot);
		for (int effect : effects) EXTEfx.alDeleteEffects(effect);
		for (int ir : irs) AL10.alDeleteBuffers(ir);
		if (filter != 0) EXTEfx.alDeleteFilters(filter);
		AL10.alDeleteBuffers(noise);
		AL10.alGetError();
		return new Result(config, elapsed / audio, swapMillis, note);
	}

	/** Renders {@code blocks} blocks, moving every source a little around the listener before each. */
	private static void render(long device, ByteBuffer out, int[] sources, double[] angles, double[] radii,
			int blocks) {
		for (int block = 0; block < blocks; block++) {
			for (int i = 0; i < sources.length; i++) {
				angles[i] += 0.01;
				AL10.alSource3f(sources[i], AL10.AL_POSITION, (float) (Math.cos(angles[i]) * radii[i]),
						(float) Math.sin(angles[i] * 0.3), (float) (Math.sin(angles[i]) * radii[i]));
			}
			SOFTLoopback.alcRenderSamplesSOFT(device, out, BLOCK);
		}
	}

	/** A first-order ambisonic (B-format) IR of decaying noise, like the ones Earshot will synthesize. */
	private static int impulseResponse(double seconds, SplittableRandom random) {
		int frames = (int) (seconds * RATE);
		FloatBuffer data = BufferUtils.createFloatBuffer(frames * 4);
		for (int i = 0; i < frames; i++) {
			// -60 dB at `seconds`.
			float envelope = (float) Math.exp(-6.91 * i / (seconds * RATE));
			for (int channel = 0; channel < 4; channel++) {
				float weight = channel == 0 ? 0.5f : 0.3f;
				data.put((float) (random.nextGaussian() * weight * envelope * 0.05));
			}
		}
		int buffer = AL10.alGenBuffers();
		AL10.alBufferData(buffer, AL_FORMAT_BFORMAT3D_FLOAT32, data.flip(), RATE);
		checkError("loading a B-format IR");
		return buffer;
	}

	private static void checkError(String what) {
		int error = AL10.alGetError();
		if (error != AL10.AL_NO_ERROR) {
			throw new IllegalStateException(what + ": OpenAL error 0x" + Integer.toHexString(error));
		}
	}

	private static void report(String version, List<Result> results) throws IOException {
		StringBuilder md = new StringBuilder();
		md.append("### OpenAL Soft render cost\n\n");
		md.append(String.format(Locale.ROOT, "%s, %s %s, Java %s. %d s of 48 kHz stereo per row, rendered on one "
				+ "thread.%n%n", version, System.getProperty("os.name"), System.getProperty("os.arch"),
				System.getProperty("java.version"), SECONDS));
		md.append("| Configuration | Sources | Share of one core | IR swap | Note |\n");
		md.append("| --- | --- | --- | --- | --- |\n");
		for (Result r : results) {
			md.append(String.format(Locale.ROOT, "| %s | %d | %s | %s | %s |%n", r.config.name, r.config.sources,
					Double.isNaN(r.coreShare) ? "–" : String.format(Locale.ROOT, "%.1f %%", r.coreShare * 100),
					Double.isNaN(r.swapMillis) ? "–" : String.format(Locale.ROOT, "%.1f ms", r.swapMillis), r.note));
		}
		System.out.print(md);
		String summary = System.getenv("GITHUB_STEP_SUMMARY");
		if (summary != null) {
			Files.writeString(Path.of(summary), md, StandardOpenOption.CREATE, StandardOpenOption.APPEND);
		}
	}
}
