package io.github.cumsauce.earshot.openal;

import io.github.cumsauce.earshot.EarshotClient;
import java.util.ArrayList;
import java.util.List;
import org.jspecify.annotations.Nullable;
import org.lwjgl.openal.AL10;
import org.lwjgl.openal.ALC10;
import org.lwjgl.openal.ALC11;
import org.lwjgl.openal.EXTEfx;
import org.lwjgl.openal.SOFTHRTF;

/**
 * Reports what the game's OpenAL can do for Earshot (M0): the OpenAL Soft version, the effect slots and sends
 * available, HRTF, and the extensions the plan relies on. Read-only: it changes nothing in the game's context, and
 * the one effect it creates to test the convolution type is deleted again.
 */
public final class OpenAlProbe {
	/** AL_SOFT_convolution_effect, still in-progress in OpenAL Soft, so not in LWJGL. */
	private static final int AL_EFFECT_CONVOLUTION_SOFT = 0xA000;
	private static final String CONVOLUTION_EXTENSION = "AL_SOFTX_convolution_effect";

	private static volatile @Nullable Report last;

	private OpenAlProbe() {}

	/** What the game's OpenAL can do. */
	public record Report(
			String where,
			String version,
			String renderer,
			String device,
			int frequency,
			int monoSources,
			int stereoSources,
			boolean efx,
			int maxAuxiliarySends,
			boolean eaxReverb,
			boolean convolutionExtension,
			boolean convolutionEffect,
			boolean callbackBuffer,
			boolean hrtfExtension,
			int hrtfStatus,
			List<String> hrtfs,
			String alExtensions,
			String alcExtensions) {

		public String describe() {
			return String.join("\n",
					"OpenAL probe (" + where + ")",
					"  version: " + version + " (" + renderer + ")",
					"  device: " + device + ", " + frequency + " Hz",
					"  sources: " + monoSources + " mono, " + stereoSources + " stereo",
					"  EFX: " + efx + ", max auxiliary sends per source: " + maxAuxiliarySends,
					"  EAX reverb effect: " + eaxReverb,
					"  convolution: extension " + convolutionExtension + ", effect type " + convolutionEffect,
					"  callback buffers: " + callbackBuffer,
					"  HRTF: extension " + hrtfExtension + ", status " + hrtfStatus(hrtfStatus) + ", available " + hrtfs,
					"  AL extensions: " + alExtensions,
					"  ALC extensions: " + alcExtensions);
		}
	}

	/** The last report, or null if the probe hasn't run or found no context. */
	public static @Nullable Report last() {
		return last;
	}

	/** Probes the current context and logs the result. Never throws: a failed probe must not break the game. */
	public static void run(String where) {
		try {
			Report report = probe(where);
			if (report == null) {
				EarshotClient.LOGGER.warn("OpenAL probe ({}): no current OpenAL context", where);
				return;
			}
			last = report;
			EarshotClient.LOGGER.info(report.describe());
		} catch (RuntimeException | LinkageError e) {
			EarshotClient.LOGGER.warn("OpenAL probe ({}) failed", where, e);
		}
	}

	private static @Nullable Report probe(String where) {
		long context = ALC10.alcGetCurrentContext();
		if (context == 0) return null;
		long device = ALC10.alcGetContextsDevice(context);
		// Leave no error behind for the game's own checks to trip over.
		AL10.alGetError();

		boolean efx = ALC10.alcIsExtensionPresent(device, "ALC_EXT_EFX");
		int sends = efx ? ALC10.alcGetInteger(device, EXTEfx.ALC_MAX_AUXILIARY_SENDS) : 0;
		boolean eaxReverb = efx && effectTypeWorks(EXTEfx.AL_EFFECT_EAXREVERB);
		boolean convolutionExtension = AL10.alIsExtensionPresent(CONVOLUTION_EXTENSION);
		boolean convolutionEffect = efx && effectTypeWorks(AL_EFFECT_CONVOLUTION_SOFT);

		boolean hrtfExtension = ALC10.alcIsExtensionPresent(device, "ALC_SOFT_HRTF");
		int hrtfStatus = -1;
		List<String> hrtfs = new ArrayList<>();
		if (hrtfExtension) {
			hrtfStatus = ALC10.alcGetInteger(device, SOFTHRTF.ALC_HRTF_STATUS_SOFT);
			int count = ALC10.alcGetInteger(device, SOFTHRTF.ALC_NUM_HRTF_SPECIFIERS_SOFT);
			for (int i = 0; i < count; i++) {
				hrtfs.add(SOFTHRTF.alcGetStringiSOFT(device, SOFTHRTF.ALC_HRTF_SPECIFIER_SOFT, i));
			}
		}

		Report report = new Report(where,
				AL10.alGetString(AL10.AL_VERSION),
				AL10.alGetString(AL10.AL_RENDERER),
				ALC10.alcGetString(device, ALC11.ALC_ALL_DEVICES_SPECIFIER),
				ALC10.alcGetInteger(device, ALC10.ALC_FREQUENCY),
				ALC10.alcGetInteger(device, ALC11.ALC_MONO_SOURCES),
				ALC10.alcGetInteger(device, ALC11.ALC_STEREO_SOURCES),
				efx, sends, eaxReverb, convolutionExtension, convolutionEffect,
				AL10.alIsExtensionPresent("AL_SOFT_callback_buffer"),
				hrtfExtension, hrtfStatus, hrtfs,
				AL10.alGetString(AL10.AL_EXTENSIONS),
				ALC10.alcGetString(device, ALC10.ALC_EXTENSIONS));
		AL10.alGetError();
		return report;
	}

	/** Whether an effect of {@code type} can be made, by making one and deleting it. */
	private static boolean effectTypeWorks(int type) {
		int effect = EXTEfx.alGenEffects();
		if (AL10.alGetError() != AL10.AL_NO_ERROR) return false;
		EXTEfx.alEffecti(effect, EXTEfx.AL_EFFECT_TYPE, type);
		boolean works = AL10.alGetError() == AL10.AL_NO_ERROR;
		EXTEfx.alDeleteEffects(effect);
		AL10.alGetError();
		return works;
	}

	private static String hrtfStatus(int status) {
		return switch (status) {
			case SOFTHRTF.ALC_HRTF_DISABLED_SOFT -> "disabled";
			case SOFTHRTF.ALC_HRTF_ENABLED_SOFT -> "enabled";
			case SOFTHRTF.ALC_HRTF_DENIED_SOFT -> "denied";
			case SOFTHRTF.ALC_HRTF_REQUIRED_SOFT -> "required";
			case SOFTHRTF.ALC_HRTF_HEADPHONES_DETECTED_SOFT -> "headphones detected";
			case SOFTHRTF.ALC_HRTF_UNSUPPORTED_FORMAT_SOFT -> "unsupported format";
			default -> "unknown (" + status + ")";
		};
	}
}
