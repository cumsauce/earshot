**Title:** External voice output: let another mod play decoded voice (voice sink, ABI 8)

## Why

I'm building a client-only acoustics mod for 26.3 (working name Earshot). It drives the OpenAL Soft that ships with Minecraft:

- EFX filters for occlusion and transmission through blocks;
- reverb whose parameters (and convolution impulse responses) come from raytracing the voxel world;
- sound bending around corners;
- HRTF.

I'd like Yak VC voice to go through exactly the same processing, so a player talking behind a wall sounds muffled like their footsteps, and talking in a cave picks up the cave's reverb.

That isn't possible today: `AudioLoop::play` pans and mixes voice itself and writes it to yakvc's own cpal stream. DESIGN.md lists "OpenAL/HRTF output" as post-v1 (Milestones, item 8). This issue asks for a smaller, generic piece of that. yakvc hands decoded voice to whoever registers a sink, and doesn't need to know anything about OpenAL.

## Summary

An opt-in **voice sink**. While one is set, each playable remote voice frame goes to the sink instead of yakvc's mixer and speakers.

- **No wire or protocol change:** the voice protocol version is unchanged and players with and without a sink talk to each other normally. This is entirely receiver-side.
- **No behaviour change without a sink:** every existing path and test stays as it is.
- **All of yakvc's rules still decide what is heard:** mute, deafen, `friends_only`, groups, spectators, blocked players and range discard. The sink only gets frames `VoiceState::playback` would have played.

## Behaviour while a sink is set

**Unchanged**
- Receive path: the jitter buffer, FEC/PLC, `end_of_talk` resets, `Talking` events with the −70 dBFS threshold and hangover, and per-peer stats.
- The send path and microphone.

**Changed, in `AudioLoop::play` (`crates/yakvc-client/src/voice.rs`)**
- Each peer whose pull is `Pulled::Voice` and that the rules allow to be heard produces one `frame()` call on the sink. A frame is 960 samples of 48 kHz mono, exactly the decoded `MonoFrame`. The call carries:
  - `volume`: the per-player volume from `PeerAudio` (0–2);
  - `range`: the local `voice_range` in blocks;
  - flags (below).
- Not applied, because the sink's renderer does its own:
  - distance gain, panning and behind attenuation (`rules::spatial`);
  - `game_volume` (Voice × Master);
  - the `Mixer` soft limiter.
- Frames that `playback` would play centred carry `YAKVC_VOICE_UNPOSITIONED`: group audio (which also carries `YAKVC_VOICE_GROUP`) and `ignore_world` direct calls. For these, `range` is 0.
- Silence, muted or out-of-range frames produce no call. The sink treats a missing frame as silence.
- All frames from one `play()` call share a `tick` value (a `u64` counter of play calls), so the sink can line up streams.

**Pacing.** The `while … wants_frame() { self.play() }` loop in `AudioLoop::run` uses the voice sink's `wants_frame` instead of the speakers'. The renderer's own device clock then paces playback, the same way cpal's does today, and clock drift is handled for free.

**Speakers.** While a sink is set, yakvc doesn't open, keep open or write to the speakers. A device yakvc opened itself is closed, and is reopened by the normal `Device::maintain` logic when the sink is cleared. A sink supplied through `EngineBuilder::audio_sink` is simply left unused. The microphone is unaffected.

**`AudioLoop::receive`.** It currently returns early when `self.sink.io.is_none()` ("with nothing to play to, buffering would only grow"). It must count a voice sink as something to play to.

**Stream end.** When a remote is forgotten (`drop_gone_remotes`), the sink gets `stream_end(uuid)` so it can free that speaker's resources.

**Stats.** Add `external_output: bool` to `AudioStats` and to the `audio` object in `yakvc_stats` JSON. `speakers_open` is false while a sink is set. The debug overlay can show "Output: external" (nice to have).

## Rust API (`yakvc-client`)

A safe trait, so `yakvc-client` stays free of `unsafe`:

```rust
/// Plays decoded voice in place of the speakers. Called on the audio thread:
/// every method must return quickly and never block.
pub trait VoiceSink: Send {
    /// Whether the sink wants the next 20 ms of voice now. Paces playback
    /// like `FrameSink::wants_frame`.
    fn wants_frame(&mut self) -> bool;
    /// One playable 20 ms frame from one speaker.
    fn frame(&mut self, frame: &VoiceFrame<'_>);
    /// `uuid`'s voice link is gone; no more frames from it until it returns.
    fn stream_end(&mut self, uuid: Uuid);
}

pub struct VoiceFrame<'a> {
    pub uuid: Uuid,
    pub pcm: &'a MonoFrame,
    pub tick: u64,
    pub volume: f32,
    /// The local voice range in blocks; 0 when not positioned.
    pub range: f32,
    pub group: bool,
    pub positioned: bool,
}

impl Engine {
    /// Sends playable voice to `sink` instead of the speakers; `None` goes back
    /// to the speakers. Takes effect on the audio thread's next tick.
    pub fn set_voice_sink(&self, sink: Option<Box<dyn VoiceSink>>);
}
```

**Implementation notes**
- Hand the sink to the audio thread through a slot in `Shared`, for example a `Mutex<Option<Option<Box<dyn VoiceSink>>>>` that `AudioLoop::run` takes once per tick. Don't put it in `VoiceState`, which is cloned copy-on-write.
- Refactor `VoiceState::playback` so the speaker path and the sink path share one decision function. Have it return "positioned at `pos`" or "centred", plus the per-player volume, and let `spatial()` apply on top only for the speakers. The rules must not drift apart between the two paths.
- Export `VoiceSink` and `VoiceFrame` from `yakvc-client/src/lib.rs`.

## C ABI (`yakvc-ffi`, ABI 7 → 8)

```c
#define YAKVC_VOICE_SINK_VERSION 1
#define YAKVC_VOICE_GROUP        (1 << 0)  // group audio
#define YAKVC_VOICE_UNPOSITIONED (1 << 1)  // play centred: group audio and direct calls

typedef struct YakVcVoiceFrame {
    uint8_t      uuid[16];   // big-endian, as everywhere in the ABI
    const float *pcm;        // `samples` floats, valid only during the call
    size_t       samples;    // 960 (48 kHz mono, 20 ms)
    uint64_t     tick;       // frames from one play tick share it
    float        volume;     // per-player volume, 0..2
    float        range;      // local voice range in blocks; 0 when unpositioned
    uint32_t     flags;      // YAKVC_VOICE_*
} YakVcVoiceFrame;

typedef struct YakVcVoiceSink {
    uint32_t version;                                            // YAKVC_VOICE_SINK_VERSION
    void    *user;
    bool   (*wants_frame)(void *user);
    void   (*frame)(void *user, const YakVcVoiceFrame *frame);
    void   (*stream_end)(void *user, const uint8_t uuid[16]);
} YakVcVoiceSink;

// Sends playable voice to `sink` instead of the speakers; NULL goes back to the speakers.
// The struct is copied, but `user` and the function pointers must stay valid for the rest
// of the process: the audio thread may still call the old sink for a tick after a change,
// and a shutdown that abandons a stuck audio thread may call it later still.
// Callbacks run on yakvc's audio thread. They must not block, must not unwind, and must
// not call back into yakvc_*.
int32_t yakvc_set_voice_sink(YakVcEngine *engine, const YakVcVoiceSink *sink);
```

**Implementation notes**
- Model the function pointers in Rust as `Option<unsafe extern "C" fn(...)>`, so cbindgen emits them as above and a null pointer is rejected rather than called. Errors (`YAKVC_ERR_INVALID_ARGUMENT`):
  - a different `version`;
  - a null function pointer.
- `yakvc-ffi` wraps the C struct in a type implementing `VoiceSink`. It needs an `unsafe impl Send`, with a `// SAFETY:` comment pointing at the contract above. This crate is already the one allowed `unsafe`.
- Bump `YAKVC_ABI_VERSION` to 8 with a history line ("8: `yakvc_set_voice_sink`"), then run `cargo xtask header`.

## Java side (`mod/`)

- `NativeBridge`: `ABI_VERSION = 8`, plus a `setVoiceSink(MemorySegment engine, MemorySegment sink)` handle (`FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS)`).
- **Discovery** goes through Fabric Loader's `ObjectShare`, so neither mod needs the other at compile time.
  - Key: `yakvc:voice_sink`.
  - Value: a `java.lang.foreign.MemorySegment` whose address is a `YakVcVoiceSink`.
  - Each client tick, as one more `step(...)` in `GameStateFeeder`, read `FabricLoader.getInstance().getObjectShare().get("yakvc:voice_sink")`. When its address changes, call `setVoiceSink`.
  - A missing key, `MemorySegment.NULL`, or a value that isn't a `MemorySegment` means speakers.
  - As with other values, one the engine rejects is logged once and not resent until it changes.
  - Log at INFO when voice output switches to or from the external sink.
- `setGameVolume` keeps being sent. The engine ignores it while a sink is set, because the renderer applies the game's Voice and Master volumes itself.

## Tests

- **`rules.rs`:** the shared decision gives the same answer on both paths. For the sink path:
  - nothing for a muted, deafened, out-of-range, untracked or spectator case;
  - `positioned` with the right `range` for a nearby player;
  - `group` and not `positioned` (range 0) for a group mate;
  - the per-player volume passed through unscaled by distance.
- **`voice.rs`:**
  - with a recording `VoiceSink` set, frames from a remote reach it and the `FrameSink` given through `AudioIo` receives nothing;
  - clearing the sink resumes the `FrameSink`;
  - `stream_end` fires when the link goes;
  - `receive` keeps buffering with only a voice sink.
- **testkit:** two engines, the talker on a tone source and the listener with a recording sink. The sink gets non-silent 960-sample frames with the talker's UUID, `Talking` events still fire, and a group pair gets `GROUP | UNPOSITIONED`.
- **`yakvc-ffi/tests/abi.rs`:**
  - add `yakvc_set_voice_sink` to `every_engine_call_rejects_a_null_engine` (the test enforces this);
  - a sink built from Rust `extern "C"` functions is accepted and NULL is accepted;
  - a wrong `version` and a null function pointer are rejected with `YAKVC_ERR_INVALID_ARGUMENT`.
- **Java:** `NativeBridgeTest` and `VoiceStatsTest` updated for the ABI bump and `external_output`.

## DESIGN.md

- Voice transport and audio pipeline:
  - Receive path step 3 and step 5: in the external-output mode, frames go to the sink without gain, pan, Voice × Master or limiter.
  - Threads: callbacks run on the audio thread.
- Fabric mod and FFM bridge: the C ABI block, the `GameStateFeeder` row (the `ObjectShare` step), and the ABI history.
- Milestones, item 8: OpenAL/HRTF output is now possible for any renderer mod through the sink; yakvc itself still doesn't render to OpenAL.

## Out of scope

- Any OpenAL or rendering code in yakvc.
- Possible follow-up, not in this issue: also passing the local microphone's processed frames (`YAKVC_VOICE_SELF`), so a renderer can add room reverb to your own voice on headphones.

## Acceptance

- [ ] `cargo test --workspace --all-features`, clippy with `-D warnings`, `cargo xtask header --check` and `cargo deny check` are green.
- [ ] Without a sink, behaviour and existing tests are unchanged.
- [ ] With a sink, the speakers aren't opened, and every rule (mute, deafen, friends_only, groups, spectators, blocked, range) still decides what reaches the sink.
- [ ] ABI 8 in `yakvc.h`, `NativeBridge` and the DESIGN.md history.
