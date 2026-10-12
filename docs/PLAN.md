# Earshot — Plan

Draft, 2026-10-11. "Earshot" (mod id `earshot`) is a working name; renaming before the first push is cheap.

## Goal

A client-only Fabric mod for Minecraft 26.3 that makes the game's own sounds behave like sound in a real space. Sounds are muffled through walls and bend around corners and through doorways. A cave, a wooden house and a stone hall each sound different, cliffs echo, and everything is binaural on headphones. Yak VC voice goes through the same processing, so a player talking behind a wall sounds like their footsteps do. It ships as one jar for macOS, Linux and Windows. It runs comfortably on an M-series MacBook, and raytracing is at its core.

Non-goals: replacing vanilla's sound assets, server-side anything, changing gameplay-relevant loudness by default (vanilla distance falloff stays unless the player opts in).

## What I checked (2026-10-11)

1. **Minecraft 26.3 ships LWJGL 3.4.3**, including `lwjgl-openal` (from Mojang's 26.3 version manifest). That means OpenAL Soft. LWJGL 3.4.2 moved to OpenAL Soft 1.25.2, and 3.4.3's notes list no OpenAL change. So 26.3 almost certainly runs 1.25.2, which M0 confirms at runtime from `AL_VERSION`.
2. **OpenAL Soft already does most of the rendering we want:**
   - HRTF. Vanilla's "Directional Audio" option turns it on.
   - Per-source EFX filters: low-pass, high-pass and band-pass for the direct path and for each effect send.
   - Auxiliary effect slots with EAX reverb.
   - An internal ambisonic mix that HRTF decodes, so reverb comes out binaural as well.
3. **OpenAL Soft has a convolution reverb effect, still marked in-progress.** It is `AL_SOFT_convolution_effect`, with `AL_EFFECT_CONVOLUTION_SOFT = 0xA000` and `AL_CONVOLUTION_ORIENTATION_SOFT`, defined in `alc/inprogext.h` (not the public `alext.h`, and not bound by LWJGL). An effect slot takes an impulse response (IR) from an AL buffer, including B-format (ambisonic) IRs. That gives us true convolution with IRs synthesized from our own raytrace, with no mixer of our own. Because the extension is in-progress, it is a tier detected at runtime, with parametric EAX reverb as the fallback.
4. **`AL_SOFT_callback_buffer`** lets a source pull PCM from a native callback on OpenAL's mixer thread. That is how Yak VC voice can join the game's OpenAL context with no Java on the audio path.
5. **Yak VC mixes voice itself** and plays it on its own `cpal` stream, so seamless integration needs a change in yakvc. That change is the voice sink requested in [desocketed/yakvc#174](https://github.com/desocketed/yakvc/issues/174). Since the evening of 2026-10-11 (yakvc 0.1.0-alpha.2, main at `4dcef89`, ABI 10), yakvc also does simple acoustics of its own for voice, still in stereo with no HRTF:
   - **Occlusion:** one ray per player, with material shares (half for solid blocks, 15 % wool, 70 % glass, 85 % leaves, 80 % per fluid block), giving up to a 75 % gain cut plus a per-source low-pass.
   - **Room reverb:** 32 rays around the listener set a send level and decay for a small Freeverb-style reverb in the mixer.
   - **Paths around walls** (#173): a breadth-first search over open blocks moves the voice to the opening it comes through.

   This is MIT/Apache code, so we may borrow from it. With a sink set, yakvc should skip all of it, so voice isn't processed twice. Without Earshot, or before the sink lands, voice still gets yakvc's own occlusion and reverb.
6. **Prior art.**
   - Sound Physics Remastered (henkelmax) does raycast occlusion and EFX reverb with four fixed reverb slots. It is GPL-3.0, so we take no code from it. Its newest Fabric build I could find is for 26.2.
   - Steam Audio (Valve, Apache-2.0, 4.8.1) does HRTF, raytraced reflections, convolution and pathing.
7. **Minecraft is a voxel grid.** Rays can step through blocks with a 3D DDA (Amanatides–Woo) without triangle meshes or a BVH. That is the main reason to write our own tracer instead of adopting Steam Audio.

## M0 results so far (2026-10-11)

**OpenAL render cost** (`mod/src/bench`, `./gradlew alBench`).
- Measured on the M3 Pro VM with LWJGL 3.4.3's own natives, which identify as `1.1 ALSOFT 1.25.2`. Each row renders 20 s of 48 kHz stereo from moving looping-noise sources on a loopback device, on one thread, which is what OpenAL Soft's mixer thread does in the game.
- `AL_SOFTX_convolution_effect` is present. B-format impulse responses load, and swapping a slot's IR while it plays works.

| Configuration | Sources | Share of one core | IR swap |
| --- | --- | --- | --- |
| dry, stereo panning | 32 | 0.2 % | |
| dry, HRTF | 32 | 2.8 % | |
| HRTF + occlusion filters | 32 | 3.3 % | |
| HRTF + filters + 1 EAX reverb | 32 | 4.3 % | |
| HRTF + filters + 2 EAX reverbs | 32 | 5.4 % | |
| HRTF + filters + 1 convolution (1.5 s IR) | 32 | 9.9 % | 10.4 ms |
| HRTF + filters + 2 convolutions (1.5 s IR) | 32 | 17.1 % | 11.6 ms |
| HRTF + filters + 2 convolutions (3 s IR) | 32 | 32.0 % | 20.7 ms |
| HRTF + filters + 2 convolutions (1.5 s IR) | 64 | 21.9 % | 12.4 ms |

**What this changes**
- The convolution tier is confirmed.
- Keep only one convolution slot running. Attach the effect to the twin slot only for the length of a crossfade, so High costs about 10 % of a core most of the time and about 17 % during a swap.
- 3 s IRs double the cost. Ultra should use a convolved early part plus an EAX tail (hybrid) rather than long IRs.
- An IR swap blocks the calling thread for 10–20 ms while OpenAL Soft prepares it. Swaps therefore run on an Earshot thread, never on Minecraft's sound thread or the client thread.
- EAX reverb is cheap (about 1 % per slot), so the parametric tier is nearly free.

**The same bench on GitHub's runners** (macos-latest, ubuntu-latest x86):
- One 1.5 s convolution: 13–14 % of a core.
- Two convolutions: about 21 %.
- HRTF + filters + 1 EAX reverb: about 6 %.
- So moderate hardware costs about 1.4× the M3.

**In game** (client gametest, OpenAL Soft's null backend):
- `1.1 ALSOFT 1.25.2`, 48 kHz.
- 255 mono and 1 stereo sources.
- EFX with EAX reverb.
- `AL_SOFTX_convolution_effect`, and the convolution effect type works.
- `AL_SOFT_callback_buffer`.
- HRTF: `Built-In HRTF` only.
- Minecraft's context allows only **2 auxiliary sends per source**, so Earshot must ask for 4 when the context is created.

**Mixin targets in 26.3** (`./gradlew dumpSoundClasses`):
- `com.mojang.blaze3d.audio.Library`
  - `init(String, DeviceList, boolean)`
  - `createAttributes(MemoryStack, boolean)` builds the context attributes: add `ALC_MAX_AUXILIARY_SENDS` here.
  - `getChannelCount()`: reserve voice sources.
  - `acquireChannel(Library.Pool)`, `releaseChannel(Channel)`, `cleanup()`.
- `com.mojang.blaze3d.audio.Channel`
  - private `int source`
  - `create()`, `destroy()`, `play()`, `stop()`
  - `setSelfPosition(Vec3)`, `setRelative(boolean)`
  - `linearAttenuation(float)`, `disableAttenuation()`
  - `attachStaticBuffer(SoundBuffer)`, `attachBufferStream(AudioStream)`
- `net.minecraft.client.sounds.SoundEngine`
  - `play(SoundInstance)`, `tick(boolean)`, `updateSource(Camera)`
  - the channel setup lambda in `play`, where a `SoundInstance` meets its `Channel`
- `ChannelAccess.createHandle(Library.Pool)`
- `ChannelAccess.ChannelHandle.execute(Consumer<Channel>)`

M0 is done apart from tidying: the convolution tier is confirmed, the mixin targets exist, and CI builds, tests and benchmarks on every push.

## M1 results so far (2026-10-11)

**Path tracer** (`cargo run --release -p earshot-sim --example trace_bench`, M3 Pro VM with 4 cores).
- Setup: 4096 rays × up to 16 bounces, reach 64 blocks, median of 5 runs.
- The DDA looks up the section only when a ray crosses into a new one, and section positions use an FxHash-style hasher.

| World | Legs | Escaped at once | Mean free path | 1 thread | Legs/s | 4 threads |
| --- | --- | --- | --- | --- | --- | --- |
| stone hall 40×20×40 | 69 632 | 0 % | 19.8 blocks | 22.6 ms | 3.1 M | 6.7 ms |
| noise caves 160×80×160 | 65 162 | 0 % | 9.0 blocks | 13.0 ms | 5.0 M | 3.3 ms |
| open ground | 6 093 | 51 % | 6.1 blocks | 4.0 ms | 1.5 M | 1.1 ms |

- The High preset's reverb trace (4096 × 16 at 10 Hz) costs at most about 23 % of one core, or about 7 % spread over 4. That is well inside the plan's one-core budget.
- Open-air legs are the slowest per leg: they step through 64 empty blocks. Skipping empty sections is the obvious next optimization, but it isn't needed yet.

## M2 results so far (2026-10-11)

What every positional game sound now gets:
- **Direct path:** the mean of five rays to points 0.35 blocks around the source, so edges muffle gradually. The blocks at either end don't count.
- **Pathing:** a 49³ shortest-air-path field around the listener (Dial's algorithm, 26 neighbours, no corner cutting), rebuilt on the engine's own thread when the listener moves half a block, or 200 ms after blocks change.
- **Round the corner:** a blocked sound is heard round the corner when that loses less in the mid band. It is then heard from the last opening the listener can see, at the path's length, and bending costs 3 / 6 / 12 dB per right angle in the low / mid / high bands.
- **OpenAL:** each source gets a low-pass direct filter (gain = low band, HF gain = high / low) and OpenAL's air absorption. Underwater, everything is multiplied by 0.7 / 0.2 / 0.05.
- **Smoothing:** about 100 ms on gains and on the heard-from offset, so moving sources aren't dragged behind.
- **The context** now asks for 4 auxiliary sends (`Library.createAttributes`), and the probe confirms it gets 4.

| Pathing field build, 49³ (M3 VM) | |
| --- | --- |
| stone hall | 2.8 ms |
| caves | 5.4 ms |
| open ground | 5.4 ms |

The gametest checks a looping beacon hum in three states:
- in a closed stone box: muffled;
- with a doorway cut in the box's far side: heard from the doorway's side;
- in the open: clear, from where it is.

## Approaches considered

| Option | What it is | Verdict |
| --- | --- | --- |
| **A. Drive OpenAL Soft** | Our raytracer computes acoustic parameters, and the OpenAL Soft already in the game renders them: EFX filters for occlusion, effect slots for reverb (convolution with raytraced IRs, or parametric EAX), HRTF for direction. Voice comes in through callback-buffer sources in the same context. | **Chosen.** Reuses the game's mixer, keeps vanilla sound behaviour and other sound mods working, and costs the least CPU. |
| B. Own renderer | Replace Minecraft's OpenAL layer and send every channel's PCM into a Rust mixer (HRTF, early reflections, convolution, then one output stream). | Not now. It would re-implement pitch, streaming, looping, attenuation and device handling, and it would break other sound mods. Revisit only if OpenAL Soft's convolution turns out unusable *and* parametric reverb isn't good enough. |
| C. Steam Audio | Valve's SDK over FFM. | Not now. Its scene is triangle meshes while the world is voxels, its pathing needs baked probes that don't suit a world that changes, and it adds a large native library per platform. Its material table (3-band absorption, scattering and transmission, Apache-2.0) is a good source for our defaults. |
| D. Pure Java | Tracer in Java, no natives. | Rejected. Voice has to move between real-time threads (yakvc's audio thread to OpenAL's mixer thread) with no JVM in between. Rust also gives SIMD and thread QoS. Players already need `--enable-native-access` for Yak VC. |

## Architecture

```
 Minecraft client (Java, Fabric)                      earshot native (Rust, via FFM)
┌──────────────────────────────────┐   sections    ┌──────────────────────────────────────┐
│ ClientLevel: chunk load/unload,  │──────────────▶│ World mirror: 16³ sections of        │
│ block changes                    │   + deltas    │ material ids, empty/solid flags      │
│                                  │               │                                      │
│ Sound mixins: play / move / stop │──── source ──▶│ Acoustic sim (worker pool, QoS util.)│
│ (SoundEngine, Channel, Library)  │     poses     │  • direct path: DDA occlusion and    │
│                                  │               │    transmission per band             │
│ EFX applier (sound thread):      │◀── params ────│  • pathing: air distance field from  │
│ filters, sends, virtual position,│  shared table │    listener → virtual source position│
│ effect slots, IR buffers         │◀── IRs ───────│  • reverb: listener path trace →     │
└───────────────┬──────────────────┘               │    energy histograms → EAX params    │
                │ LWJGL OpenAL                     │    or B-format IR synthesis          │
                ▼                                  │                                      │
     OpenAL Soft 1.25 (inside the game)            │ Voice bridge: one ring per speaker   │
     HRTF · EFX filters · EAX/convolution  ◀───────│  ◀── yakvc voice sink (C fn ptrs)    │
     reverb → Minecraft's sound device   callback  │  ──▶ AL callback-buffer sources      │
                                          buffers  └──────────────────────────────────────┘
```

Java owns everything that touches game state and makes the OpenAL calls on the sound thread. Rust owns the world mirror, the simulation, IR synthesis and the real-time voice path. They share a parameter table in native memory, so the per-tick handover is one bulk read, not one FFM call per source.

### 1. World mirror

- Sparse map of 16³ sections keyed by section position, one byte of material id per block, plus per-section "all air" and "all solid" flags so rays skip whole sections. Later, a 4³ occupancy bitmask per brick for faster empty-space skipping.
- Java converts `BlockState` to a material id through a flat array indexed by the block-state registry id, rebuilt on resource reload. Uploads happen on chunk load (Fabric `ClientChunkEvents`), and single-block deltas come from a mixin on client block changes. Doors, trapdoors and fence gates map to different materials when open and closed.
- Uploads run under a per-tick budget, nearest sections first, so logging in or teleporting doesn't hitch.
- Units: 1 block = 1 m, speed of sound 343 m/s.

### 2. Materials

- Each material has absorption in 3 bands (low ≤ 500 Hz, mid, high ≥ 2 kHz), a scattering coefficient and transmission loss per block in each band.
- Defaults are seeded from Steam Audio's published table (rock, wood, glass, carpet and so on), then mapped by block tags and `SoundType`. Wool, carpet and moss absorb, stone reflects, glass passes low frequencies, leaves are mostly transmissive and scattering, and water is its own case. This way modded blocks get sensible values without a list.
- A data-driven override (`assets/<ns>/earshot/materials.json`, resource-pack friendly) is added in M5.

### 3. Tracer

- 3D DDA through the mirror, written in Rust with a fixed-seed RNG for reproducible tests.
- Workers come from a small pool (2–4 threads) at macOS utility QoS. On Apple Silicon that lands on the E-cores and stays off the render thread.
- Rough estimate, measured in M1: a core steps tens of millions of voxels per second, which is on the order of a million ray segments per second at 20–40 steps per segment.

### 4. Direct path (occlusion and transmission)

- For each positional source: a few jittered rays over a small disc around the source to the listener. Material thickness along each ray gives a per-band transmission gain, and averaging the rays gives soft partial occlusion instead of flicker at block edges.
- The result maps to the source's EFX direct filter as a band-pass with `gain`, `gainLF` and `gainHF`. Air absorption is added for long distances, which gives thunder its distant rumble.
- Underwater: when the camera is in water, everything gets a strong low-pass and an underwater reverb.
- Changes are smoothed over about 50–100 ms so moving past a pillar doesn't zipper.

### 5. Pathing (sound around corners)

- A shortest-path distance field over air cells, grown outward from the listener with Dijkstra or fast marching (26-neighbour) in a cube around them (64³ on High). It is rebuilt incrementally a few times a second.
- For a source inside the field:
  - the path length gives extra distance (quieter, later);
  - the last point on the path the listener can see is the opening the sound arrives through, so the source is played from a **virtual position** in that direction at the path distance;
  - the total turning angle gives a diffraction loss that removes more highs the sharper the bends.
- The through-wall component (section 4) and the around-the-corner component are blended by energy, so a thin wooden wall and an open door both contribute.
- Sources outside the field fall back to direct-path occlusion only.

### 6. Reverb and echo

- **Listener path trace.** N rays from the listener, each bouncing up to K times. Each bounce takes specular or diffuse reflection by the material's scattering, and absorbs per band. Rays escaping to open sky are counted as openness. Arrivals back near the listener go into energy histograms (1 ms bins early, coarser later) for each band and for each first-order ambisonic channel (W, X, Y, Z), which keeps direction of arrival.
- **Derived metrics.** RT60 per band (Schroeder integration of the histogram), early-reflection delay, level and direction, late-reverb level and direction, mean free path (room size), openness, and discrete late echoes (strong isolated arrivals over 50 ms, such as ravine and canyon walls). All are smoothed over time (about 0.5 s) so the room never flickers.
- **Parametric tier (every preset).** The metrics drive an EAX reverb slot: decay time, HF and LF ratios, reflections and late gains, delays and pan vectors, density and diffusion from room size and scattering, echo time and depth from detected echoes. This is cheap, and it is already "physically driven".
- **Convolution tier (High and Ultra, if the extension is present).**
  - IR synthesis turns the histograms into a 4-channel B-format IR. Early part (≤ 80 ms): explicit taps for the strongest specular paths with their delays, directions and band gains. Late part: decorrelated noise per channel, shaped per band by the measured energy envelope.
  - The IR is loaded into one of two convolution slots, and the slot gains crossfade over about 300 ms so updates never click.
  - Updates are at most about 2 Hz, and only when the metrics change meaningfully.
- **Sends.**
  - Every positional source sends to the listener-space reverb.
  - Sources behind walls send through the same transmission filter. A cave behind a wall still rumbles, just muffled.
  - Send level follows distance more slowly than the direct path, so far sources sound more reverberant than near ones. That direct-to-reverberant ratio is the main cue the ear uses for distance.
- **Later (Ultra, M5).** Up to two source-space slots. Loud sources in a different space (a ghast in a cave you're standing outside) get the reverb of the space they are in.
- **Requested OpenAL context attributes:** 4 auxiliary sends per source (listener slot A and B for the crossfade, plus two source-space slots).

### 7. Rendering through OpenAL

- Mixins (targets verified against the 26.3 jar in M0; the names are from earlier versions and Mojang's mappings):
  - `com.mojang.blaze3d.audio.Library`: context attributes (aux sends, HRTF choice), creating and destroying our EFX objects when the device opens and closes (device change, F3+T), and reserving sources for voice.
  - `com.mojang.blaze3d.audio.Channel`: binding the source id to our source state on create and destroy; reading the true position and writing the virtual one on `setSelfPosition`.
  - `net.minecraft.client.sounds.SoundEngine` / `ChannelAccess`: the `SoundInstance` behind each channel (category, relative, attenuation).
  - Client block changes for the world mirror.

  Everything else uses Fabric API events.
- Non-positional sounds (relative or `Attenuation.NONE`: music, UI, some ambience) stay dry and untouched. Jukebox records are processed. Each category can be turned off.
- The applier runs on Minecraft's sound thread at tick rate. It reads the shared table and calls OpenAL only for values that changed by more than a small epsilon.
- HRTF: vanilla's Directional Audio option stays the switch. M5 adds a choice of HRTF dataset (OpenAL Soft `.mhr` files, selected with `ALC_HRTF_ID_SOFT`). Bundled datasets need a licence check first. SADIE II and MIT KEMAR are the candidates.

### 8. Yak VC voice

**Contract (owned by yakvc, so any renderer could provide it).** It is specified in full in [yakvc-voice-sink-issue.md](yakvc-voice-sink-issue.md), the issue we're asking yakvc's maintainer to implement.
- Earshot builds a versioned `YakVcVoiceSink` C struct (`wants_frame`, `frame`, `stream_end`, plus a `user` pointer) that lives for the whole process. It puts its address into Fabric Loader's `ObjectShare` under `yakvc:voice_sink`.
- yakvc checks that key every tick and calls `yakvc_set_voice_sink` (ABI 8) when it changes. Neither mod has a compile-time dependency on the other.
- Each `frame()` call carries a `YakVcVoiceFrame`: the speaker's UUID, 960 samples of 48 kHz mono, a shared `tick`, the per-player volume, the local voice range, and `GROUP` / `UNPOSITIONED` flags.

**What yakvc does in external mode.**
- yakvc keeps doing everything it does today up to the mix step: jitter buffer, Opus FEC/PLC, the talking indicator, and all the rules (mute, deafen, groups, spectators, blocked players, range discard).
- It skips its own distance gain, panning and soft limiter, and doesn't open an output device. Each playable peer frame goes to `frame()` with only the per-peer volume applied.
- The game's Voice slider and Master are then applied by the renderer and OpenAL, so nothing is applied twice.

**What Earshot does with it.**
- One SPSC ring per speaker, and one OpenAL callback-buffer source per active speaker. OpenAL's mixer thread drains the ring through a native callback. The source sits at the speaker's head (`level.getPlayerByUUID`, updated each tick) and gets exactly the same occlusion, pathing, reverb and HRTF as any game sound.
- Distance falloff is linear and clamped, at full volume within 4 blocks down to 0 at `range`, which is yakvc's current curve. A player is never heard beyond yakvc's range, and the privacy rule holds.
- Group voice plays head-relative, centred and dry (an optional "radio" filter comes later).
- Pacing: `wants_frame` follows the samples OpenAL actually consumed, which also absorbs clock drift. While OpenAL is down (device change, F3+T) it falls back to a wall clock and drops frames. The struct lives for the whole process, so it never dangles.

**Optional own-voice reverb (later, a follow-up to the issue).** yakvc also passes the local mic frames (`SELF`), and Earshot sends them only to the reverb, with no dry path. Talk in a cave and the cave answers. It is off by default and meant for headphones.

**Either mod alone.** Without Earshot, yakvc is exactly as today. Without yakvc, Earshot never touches voice.

**Changes in yakvc** are listed file by file in the issue. It asks for no wire or protocol change, and nothing changes in yakvc unless a sink is set.

### 9. Settings and debugging

- An "Acoustics…" row under Options → Music & Sounds, added the same way yakvc does it, with no mixin. A Mod Menu entry if Mod Menu is present.
- Presets Off, Low, Medium, High and Ultra, plus Auto, which picks a preset from core count and a short benchmark. Toggles for occlusion, pathing, reverb, convolution, air absorption, underwater, Doppler (later), voice integration and own-voice reverb.
- Debug overlay: per-source occlusion and path data, RT60 per band, openness, active tier, sim time, rays per second, and an IR plot. Ray and path visualization in the world. A bypass key for instant A/B listening.

## Performance budget (target: M3, High preset)

| Item | Budget |
| --- | --- |
| Render thread | < 0.2 ms per frame (nothing but the block-change hook) |
| Client thread | < 0.5 ms per tick (section uploads within budget, writing source poses) |
| Sound thread | < 0.5 ms per tick (reading the table, changed AL calls only) |
| Simulation | about 1 core on average, spread over 2–4 utility-QoS workers |
| OpenAL Soft (measured, M0) | HRTF + filters + EAX: about 4–5 % of a core. One convolution slot with a 1.5 s B-format IR: about 10 %, about 17 % while two run during a crossfade |
| Memory | world mirror ~4 KiB per non-empty section, so ~10–20 MiB at 12-chunk render distance |

The simulation measures its own time and scales ray counts down to stay inside the budget, the way dynamic resolution works for graphics.

| Preset | Reverb rays × bounces @ rate | Occlusion rays per source @ rate | Pathing field | Reverb |
| --- | --- | --- | --- | --- |
| Low | 256 × 4 @ 4 Hz | 1 @ 10 Hz | 32³ @ 4 Hz | EAX |
| Medium | 1 024 × 8 @ 8 Hz | 5 @ 20 Hz | 64³ @ 5 Hz | EAX |
| High (Apple Silicon default) | 4 096 × 16 @ 10 Hz | 9 @ 20 Hz | 64³ @ 10 Hz | convolution, 1.5 s B-format IR (EAX fallback) |
| Ultra | 16 384 × 32 @ 10 Hz | 9 @ 20 Hz | 96³ @ 10 Hz | convolution, 3 s IR, source-space slots |

## Repository, build and release

The layout mirrors yakvc so the toolchain, natives packaging and CI carry over almost unchanged:

```
earshot/
├── Cargo.toml          # workspace, MIT OR Apache-2.0, Rust edition 2024
├── crates/
│   ├── earshot-world/  # voxel mirror, materials, DDA
│   ├── earshot-sim/    # occlusion, pathing, reverb tracing, metrics, IR synthesis, scheduler
│   ├── earshot-ffi/    # C ABI (cbindgen), shared parameter table, voice bridge, AL callback
│   └── earshot-cli/    # offline: trace a saved scene → IR .wav + RT60 report (dev and CI)
├── xtask/              # natives, dist, header (as in yakvc)
├── mod/                # Fabric Loom, Java 25, Loader 0.19.5, Fabric API 0.162.0+26.3, Gradle 9.7.1
├── docs/               # this plan, later DESIGN.md and a user guide
└── .github/workflows/  # ci.yml
```

- **Natives:** `cargo xtask natives` per OS, as in yakvc. Targets: macOS universal (arm64 + x86_64), Linux x86_64 and aarch64, Windows x86_64, and maybe Windows arm64, since 26.3 ships it. Natives are xz-compressed into the jar and unpacked by a `NativeLoader` copied from yakvc's.
- **CI:**
  - `rust`: fmt, clippy, test.
  - `natives` matrix on Ubuntu, Windows and macOS.
  - `mod`: Gradle build plus a headless client gametest on Linux, using Xvfb and lavapipe as in yakvc's `scripts/headless-env.sh`.
  - A `v*` tag publishes a GitHub release.
- **Players:** the same `--enable-native-access=ALL-UNNAMED` JVM flag that Yak VC already needs.
- **Fabric metadata:** client environment, `breaks: sound_physics_remastered` (both drive EFX on the same sources), `suggests: yakvc`.

## Testing

- **Rust unit tests**
  - DDA against a brute-force voxel walk (property tests).
  - Occlusion in hand-built scenes.
  - Shoebox rooms: RT60 within ±20 % of Sabine/Eyring.
  - Energy conservation, and decay slope of the synthesized IR matching the measured RT60.
  - Determinism with a fixed seed.
- **Golden scenes:** small structures stored as files. `earshot-cli` renders each to an IR and metrics, and CI compares them with tolerances.
- **Java unit tests:** material mapping, parameter smoothing, the shared-table reader.
- **Client gametest**
  - Build a stone room, play a sound inside and outside it, and assert the engine's occlusion and reverb values.
  - End to end: run OpenAL Soft with its `wave` writer backend (`ALSOFT_DRIVERS=wave`, if LWJGL's build includes it) to record the game's real output in CI, and check that the decay is there.
- **Listening:** a "sound lab" world with reference spaces: closet, wooden house, stone hall, large cave, ravine, open plain and underwater. A/B with the bypass key.

## Milestones

0. **M0 Groundwork and spikes.**
   - Scaffold the repo and CI from yakvc's patterns.
   - The jar loads on 26.3 and logs the OpenAL Soft version, extensions (EFX, callback buffer, convolution), the maximum aux sends and the HRTF list.
   - Verify the mixin targets against the 26.3 jar.
   - Spike: play a sound through a convolution slot with a hand-made B-format IR, and measure CPU on the M3.
   - **Exit:** the convolution tier is confirmed or dropped to parametric only, and the mixin target list is final.
1. **M1 World and tracer.** World mirror, materials, DDA, ray visualization, and a tracer benchmark on the M3 recorded in the docs. **Exit:** rays visible in game, unit tests green.
2. **M2 Direct sound.** Occlusion and transmission, air absorption, underwater, pathing with virtual sources. **Exit:** in the sound lab, an open and a closed door sound clearly different, and a sound around a corner comes from the doorway.
3. **M3 Reverb.** Listener path trace, parametric EAX for every preset, convolution with synthesized IRs, twin-slot crossfades, distance-dependent wet/dry. **Exit:** shoebox RT60 within ±20 %; cave, house and outdoors are clearly distinct; no zipper or pumping when walking through a doorway.
4. **M4 Yak VC.** The yakvc PR (ABI 8, `ObjectShare` contract) and Earshot's voice bridge. **Exit:** in a `just session a b` dev session, voice behind a wall is muffled like footsteps and picks up cave reverb, and each mod alone behaves as before.
5. **M5 Polish and first prerelease.** Settings UI, presets and Auto, adaptive budget, source-space slots, HRTF dataset choice, Doppler, own-voice reverb, material overrides, user guide, `v0.1.0-alpha.1` on GitHub Releases.
6. **M6 Stretch.**
   - GPU tracing (wgpu on Metal or Vulkan), only if profiling shows the CPU is the limit.
   - 2nd/3rd-order ambisonic IRs.
   - Per-source early reflections (image sources) for the few loudest sources.

## Risks

| Risk | Mitigation |
| --- | --- |
| OpenAL Soft's convolution effect is in-progress and could change or disappear in a later LWJGL | Detected at runtime and gated on the OpenAL Soft version. The parametric EAX tier is always there and is what Low and Medium use anyway. |
| Mixins into the sound engine clash with other sound mods | Keep mixins few and narrow, `breaks` Sound Physics Remastered, and test with common client mods (Sodium, AmbientSounds, Mod Menu). |
| Section uploads hitch at login or teleport | Upload budget per tick, nearest first. Sections the sim hasn't got yet count as unknown, which means no occlusion rather than wrong occlusion. |
| Audible artifacts from parameter changes (zipper, pumping, IR swap clicks) | Smoothing on every parameter, twin-slot crossfade for IRs, rate limits on IR updates. |
| OpenAL context re-created on device change or F3+T | Rebuild EFX objects and voice sources from `Library` init hooks. The voice sink outlives contexts. |
| yakvc and Earshot drift apart | A versioned struct, a contract owned by yakvc, an ABI check at hand-over, and fallback to cpal output on mismatch. |
| Not enough OpenAL sources for voice | Reserve a fixed number of voice sources out of vanilla's pool (vanilla rarely fills it). |
| Older or weaker hardware | Auto preset, adaptive ray budget, and an Off switch for each feature. |

## Open questions

1. **Name and GitHub repo.** Keep "Earshot"? Public or private? Public repos get free macOS and Windows runners. Private repos use paid minutes, and macOS minutes cost 10×. `gh` isn't installed on this machine, so either install it or give me the remote URL.
2. **yakvc.** It is maintained by someone else, so the voice sink is an upstream request. It touches no wire protocol, so a patched client still talks to stock yakvc users. If it is declined, voice keeps yakvc's own stereo panning and Earshot processes game sounds only.
3. **License.** MIT OR Apache-2.0 to match yakvc?
4. **Distribution.** GitHub Releases only, like yakvc, or also Modrinth?
5. **Distance falloff.** Keep vanilla's linear falloff by default (gameplay stays as is), with "realistic falloff" as an option?
