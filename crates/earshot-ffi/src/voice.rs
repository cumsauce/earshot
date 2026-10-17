//! Yak VC's voice, played through the game's OpenAL so it gets the same
//! acoustics as every other sound (desocketed/yakvc#174, docs/PLAN.md M4).
//!
//! yakvc's audio thread hands each speaker's decoded frames to [`SINK`], a
//! `YakVcVoiceSink` that lives for the whole process, which queues them per
//! speaker. Java gives each speaker an OpenAL source whose buffer is a
//! callback buffer: OpenAL's mixer thread pulls the samples straight from
//! the queue through [`pull`], with no Java in between. The queue is a
//! lock-free ring ([`Ring`]): the mixer never waits on yakvc's thread, nor
//! allocates.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::guard::guard_value;

/// yakvc's `YAKVC_VOICE_SELF`: a frame of the player's own voice, which
/// only a version 2 sink gets.
pub(crate) const VOICE_SELF: u32 = 1 << 2;

/// Samples per second and per frame of yakvc's voice.
const RATE: u32 = 48_000;
const FRAME: usize = 960;
/// A speaker's queue is kept below this, dropping the oldest samples: 200
/// ms, while nothing plays it.
const MAX_QUEUED: usize = RATE as usize / 5;
/// While it plays, once more than this is queued beyond what OpenAL asks
/// for (100 ms), the oldest is dropped down to [`LEAD`]'s worth, so a stall
/// or a late start never leaves the voice lagging.
const MAX_LAG: usize = RATE as usize / 10;
const LAG_TARGET: usize = RATE as usize / 25;
/// A voice starts, and starts again after running dry, only once this much
/// is queued (40 ms): yakvc hands frames over in real time and no sooner, and
/// OpenAL asks for a mixing period at a time, so playing from the first
/// frame would stutter.
const PRIME: usize = LAG_TARGET;
/// yakvc is asked for frames this far ahead of real time, to ride over
/// OpenAL's mixing periods.
const LEAD: Duration = Duration::from_millis(40);
/// After a stall, this many frames are caught up at most.
const MAX_BACKLOG_FRAMES: u32 = 5;

/// One frame as yakvc hands it over (yakvc's `YakVcVoiceFrame`).
#[repr(C)]
#[derive(Debug)]
pub struct YakVcVoiceFrame {
    pub uuid: [u8; 16],
    pub pcm: *const f32,
    pub samples: usize,
    pub tick: u64,
    pub volume: f32,
    pub range: f32,
    pub flags: u32,
    pub heard_from: [f64; 3],
    pub occlusion: f32,
}

/// yakvc's `YakVcVoiceSink`.
#[repr(C)]
#[derive(Debug)]
pub struct YakVcVoiceSink {
    pub version: u32,
    pub user: *mut c_void,
    pub wants_frame: Option<unsafe extern "C" fn(user: *mut c_void) -> bool>,
    pub frame: Option<unsafe extern "C" fn(user: *mut c_void, frame: *const YakVcVoiceFrame)>,
    pub stream_end: Option<unsafe extern "C" fn(user: *mut c_void, uuid: *const u8)>,
}

/// The sink, as a static: yakvc may call it from its audio thread for the
/// rest of the process.
#[derive(Debug)]
pub struct Sink(pub YakVcVoiceSink);

// SAFETY: the struct is never written after it is made; `user` is null and
// the callbacks are thread-safe functions of this module.
unsafe impl Sync for Sink {}

/// The sink as version 1, for other players' voices only, which every yakvc
/// with a voice sink takes.
pub static SINK: Sink = sink(1);
/// The sink as version 2, which also asks for the player's own voice; a
/// yakvc from before own voice refuses it.
pub static SINK_WITH_OWN_VOICE: Sink = sink(2);

const fn sink(version: u32) -> Sink {
    Sink(YakVcVoiceSink {
        version,
        user: std::ptr::null_mut(),
        wants_frame: Some(wants_frame),
        frame: Some(frame),
        stream_end: Some(stream_end),
    })
}

/// How often yakvc has asked either sink for a frame: it does every few
/// milliseconds once it has taken one.
static POLLS: AtomicU64 = AtomicU64::new(0);
/// Whether own-voice frames are kept (else dropped as they come).
static OWN_VOICE: AtomicBool = AtomicBool::new(false);

/// How often yakvc has asked for a frame, to tell whether it took the sink.
pub fn polls() -> u64 {
    POLLS.load(Ordering::Relaxed)
}

/// Keeps the player's own voice from now on, or drops it.
pub fn set_own_voice(keep: bool) {
    OWN_VOICE.store(keep, Ordering::Relaxed);
}

/// Room for [`MAX_QUEUED`] samples and a few frames more.
const CAPACITY: usize = MAX_QUEUED + 4 * FRAME;

/// One speaker's samples, from yakvc's thread (the only writer) to OpenAL's
/// mixer (the only reader), with neither ever waiting nor allocating. Each
/// sample is an `f32` held as its bits; `written` and `read` count samples
/// ever written and read. Past [`MAX_QUEUED`] the oldest go: the writer can't
/// move `read`, so it raises `floor`, which the reader never reads below.
#[derive(Debug)]
struct Ring {
    samples: Box<[AtomicU32]>,
    written: AtomicUsize,
    read: AtomicUsize,
    floor: AtomicUsize,
    /// Where a stream ended: everything written before it is dropped, and
    /// everything after it is the reconnected speaker's and must play.
    cut: AtomicUsize,
}

impl Default for Ring {
    fn default() -> Self {
        Ring {
            samples: (0..CAPACITY).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            floor: AtomicUsize::new(0),
            cut: AtomicUsize::new(0),
        }
    }
}

impl Ring {
    /// Where the oldest sample still queued is.
    fn start(&self) -> usize {
        self.read
            .load(Ordering::Acquire)
            .max(self.floor.load(Ordering::Acquire))
            .max(self.cut.load(Ordering::Acquire))
    }

    fn len(&self) -> usize {
        self.written
            .load(Ordering::Acquire)
            .saturating_sub(self.start())
    }

    /// The writer: adds `pcm` (at most a few frames) times `gain`, letting
    /// the oldest go past [`MAX_QUEUED`]. It only overwrites samples below
    /// the floor, which the reader has given up.
    fn push(&self, pcm: &[f32], gain: f32) {
        let pcm = &pcm[..pcm.len().min(CAPACITY - MAX_QUEUED)];
        let written = self.written.load(Ordering::Relaxed);
        let end = written + pcm.len();
        self.floor
            .fetch_max(end.saturating_sub(MAX_QUEUED), Ordering::AcqRel);
        for (i, sample) in pcm.iter().enumerate() {
            self.samples[(written + i) % CAPACITY]
                .store((sample * gain).to_bits(), Ordering::Relaxed);
        }
        self.written.store(end, Ordering::Release);
    }

    /// The reader: drops `n` of the oldest.
    fn skip(&self, n: usize) {
        let start = self.start();
        self.read
            .store(start + n.min(self.len()), Ordering::Release);
    }

    /// The reader: takes up to `out.len()` into `out`; how many.
    fn pop(&self, out: &mut [f32]) -> usize {
        let read = self.start();
        let n = self.len().min(out.len());
        for (i, out) in out[..n].iter_mut().enumerate() {
            *out = f32::from_bits(self.samples[(read + i) % CAPACITY].load(Ordering::Relaxed));
        }
        self.read.store(read + n, Ordering::Release);
        n
    }
}

/// One speaker's voice.
#[derive(Debug, Default)]
pub struct Stream {
    queue: Ring,
    /// Whether the voice is playing out of the queue, rather than holding
    /// back until [`PRIME`] is buffered.
    primed: AtomicBool,
    info: Mutex<Info>,
}

/// What the latest frame said about where and how to play the voice.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Info {
    pub flags: u32,
    pub range: f32,
    pub volume: f32,
    pub occlusion: f32,
    pub heard_from: [f64; 3],
    pub last_frame: Option<Instant>,
    pub ended: bool,
}

#[derive(Debug, Default)]
struct Voice {
    /// Every speaker heard this session. Streams are never freed: OpenAL
    /// may still be pulling from one, and there are only as many as players
    /// met.
    streams: Mutex<HashMap<[u8; 16], Arc<Stream>>>,
    clock: Mutex<Clock>,
}

static VOICE: LazyLock<Voice> = LazyLock::new(Voice::default);

/// Paces yakvc in real time: a frame is due each 20 ms, [`LEAD`] ahead.
#[derive(Debug, Default)]
struct Clock {
    start: Option<Instant>,
    /// Counted in u64 because a session can outlast a u32 of 20 ms frames
    /// (about 994 days); a u32 would wrap and stall the clock for as long.
    frames: u64,
}

impl Clock {
    fn due(&mut self, now: Instant) -> bool {
        let start = *self.start.get_or_insert(now);
        let elapsed = now.duration_since(start) + LEAD;
        let elapsed_frames = u64::try_from(elapsed.as_millis() / 20).unwrap_or(u64::MAX);
        // After a stall, skip ahead rather than rush a backlog.
        self.frames = self
            .frames
            .max(elapsed_frames.saturating_sub(u64::from(MAX_BACKLOG_FRAMES)));
        if self.frames <= elapsed_frames {
            self.frames = self.frames.saturating_add(1);
            true
        } else {
            false
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The stream for `uuid`, made if new.
pub fn stream(uuid: [u8; 16]) -> Arc<Stream> {
    lock(&VOICE.streams).entry(uuid).or_default().clone()
}

/// Every stream heard this session, with what its latest frame said and how
/// many samples it has queued.
pub fn streams() -> Vec<([u8; 16], Info, usize)> {
    lock(&VOICE.streams)
        .iter()
        .map(|(uuid, stream)| (*uuid, *lock(&stream.info), stream.queue.len()))
        .collect()
}

/// Copies up to `out.len()` queued samples of `stream` into `out`, and
/// silence after them if the queue runs dry.
pub fn pull(stream: &Stream, out: &mut [f32]) {
    let queue = &stream.queue;
    if !stream.primed.load(Ordering::Relaxed) {
        if queue.len() < PRIME {
            out.fill(0.0);
            return;
        }
        stream.primed.store(true, Ordering::Relaxed);
    }
    if queue.len() > out.len() + MAX_LAG {
        queue.skip(queue.len() - out.len() - LAG_TARGET);
    }
    let n = queue.pop(out);
    out[n..].fill(0.0);
    if n < out.len() {
        // Ran dry: buffer again before playing on.
        stream.primed.store(false, Ordering::Relaxed);
    }
}

/// Whether yakvc should play the next 20 ms now.
unsafe extern "C" fn wants_frame(_user: *mut c_void) -> bool {
    // Behind the same barrier as every export: a panic must not unwind
    // through `extern "C"` into yakvc (it would abort the process). Not due
    // is the safe answer.
    guard_value(
        || {
            POLLS.fetch_add(1, Ordering::Relaxed);
            lock(&VOICE.clock).due(Instant::now())
        },
        false,
    )
}

/// Queues one speaker's frame, at the volume yakvc gives it.
///
/// # Safety
/// `frame` must point to a valid frame whose `pcm` holds `samples` floats
/// (yakvc's contract).
unsafe extern "C" fn frame(_user: *mut c_void, frame: *const YakVcVoiceFrame) {
    // Behind the same barrier as every export: a panic must not unwind
    // through `extern "C"` into yakvc (it would abort the process).
    guard_value(
        || {
            // SAFETY: yakvc passes a valid frame for the length of the call.
            let Some(frame) = (unsafe { frame.as_ref() }) else {
                return;
            };
            if frame.flags & VOICE_SELF != 0 && !OWN_VOICE.load(Ordering::Relaxed) {
                return;
            }
            let pcm = if frame.pcm.is_null() || frame.samples > FRAME * 4 {
                &[][..]
            } else {
                // SAFETY: yakvc's contract: `pcm` holds `samples` floats
                // during the call.
                unsafe { std::slice::from_raw_parts(frame.pcm, frame.samples) }
            };
            let stream = stream(frame.uuid);
            stream.queue.push(pcm, frame.volume);
            *lock(&stream.info) = Info {
                flags: frame.flags,
                range: frame.range,
                volume: frame.volume,
                occlusion: frame.occlusion,
                heard_from: frame.heard_from,
                last_frame: Some(Instant::now()),
                ended: false,
            };
        },
        (),
    )
}

/// A speaker's link is gone: Java lets its source go.
///
/// # Safety
/// `uuid` must point to 16 bytes (yakvc's contract).
unsafe extern "C" fn stream_end(_user: *mut c_void, uuid: *const u8) {
    // Behind the same barrier as every export: a panic must not unwind
    // through `extern "C"` into yakvc (it would abort the process).
    guard_value(
        || {
            if uuid.is_null() {
                return;
            }
            // SAFETY: yakvc passes 16 readable bytes for the length of the
            // call.
            let uuid: [u8; 16] = unsafe { std::ptr::read_unaligned(uuid.cast()) };
            if let Some(stream) = lock(&VOICE.streams).get(&uuid) {
                lock(&stream.info).ended = true;
                // Cut at the samples already written: the new link starts
                // from here, and frames queued after this call survive.
                stream.queue.cut.fetch_max(
                    stream.queue.written.load(Ordering::Acquire),
                    Ordering::AcqRel,
                );
                stream.primed.store(false, Ordering::Relaxed);
            }
        },
        (),
    )
}

/// OpenAL's callback-buffer callback (`ALBUFFERCALLBACKTYPESOFT`): fills
/// `bytes` bytes of mono float samples from the stream `user` points to, and
/// always all of them (fewer would end the source).
///
/// # Safety
/// `user` must be a pointer from [`stream_pointer`], and `data` writable for
/// `bytes` bytes, as OpenAL guarantees.
pub unsafe extern "C" fn callback(user: *mut c_void, data: *mut c_void, bytes: i32) -> i32 {
    // Silence first: the paths that fail to produce samples (a null argument,
    // or a panic caught below) must leave silence, not the buffer's stale
    // samples, which OpenAL would re-mix as an audible repeat. The pull below
    // overwrites this when it runs.
    // SAFETY: OpenAL hands `data` writable for `bytes` bytes; `silence`
    // writes nothing when `data` is null or there are no samples.
    unsafe { silence(data, bytes) };
    // Behind the same barrier as every export: a panic must not unwind
    // through `extern "C"` into OpenAL's mixer thread (it would abort the
    // process). As on the null-argument path, the buffer counts as filled:
    // fewer bytes would end the source.
    guard_value(
        || {
            let samples = usize::try_from(bytes).unwrap_or(0) / size_of::<f32>();
            if user.is_null() || data.is_null() || samples == 0 {
                return bytes.max(0);
            }
            // SAFETY: `user` came from `stream_pointer`, whose stream lives
            // for the process; OpenAL hands `data` writable for `bytes`
            // bytes, float-aligned for a float format.
            let (stream, out) = unsafe {
                (
                    &*user.cast::<Stream>(),
                    std::slice::from_raw_parts_mut(data.cast::<f32>(), samples),
                )
            };
            pull(stream, out);
            bytes
        },
        bytes.max(0),
    )
}

/// Fills the `samples` floats `data` points at with silence, for the paths of
/// [`callback`] that produce none: OpenAL would otherwise re-mix the buffer's
/// stale samples as an audible repeat. Writes nothing when there is nothing to
/// write to.
///
/// # Safety
/// `data` must be writable for `bytes` bytes when it is not null, as OpenAL
/// guarantees for [`callback`].
unsafe fn silence(data: *mut c_void, bytes: i32) {
    if data.is_null() {
        return;
    }
    let samples = usize::try_from(bytes).unwrap_or(0) / size_of::<f32>();
    if samples == 0 {
        return;
    }
    // SAFETY: OpenAL hands `data` writable for `bytes` bytes, float-aligned
    // for a float format.
    unsafe { std::slice::from_raw_parts_mut(data.cast::<f32>(), samples) }.fill(0.0);
}

/// A pointer to `uuid`'s stream for OpenAL's callback; valid for the rest of
/// the process.
pub fn stream_pointer(uuid: [u8; 16]) -> *mut c_void {
    Arc::as_ptr(&stream(uuid)).cast_mut().cast()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// yakvc's frame flags.
    const VOICE_GROUP: u32 = 1 << 0;
    const VOICE_UNPOSITIONED: u32 = 1 << 1;

    fn deliver(uuid: [u8; 16], pcm: &[f32], flags: u32) {
        let frame_struct = YakVcVoiceFrame {
            uuid,
            pcm: pcm.as_ptr(),
            samples: pcm.len(),
            tick: 1,
            volume: 0.5,
            range: 48.0,
            flags,
            heard_from: [1.0, 2.0, 3.0],
            occlusion: 0.25,
        };
        // SAFETY: the frame and its samples are valid for the call.
        unsafe { (SINK.0.frame.unwrap())(std::ptr::null_mut(), &frame_struct) };
    }

    /// yakvc.h's layout, which the gametest's fake speaker writes by hand.
    #[test]
    fn the_frame_layout_matches_yakvc() {
        assert_eq!(size_of::<YakVcVoiceFrame>(), 88);
        assert_eq!(std::mem::offset_of!(YakVcVoiceFrame, pcm), 16);
        assert_eq!(std::mem::offset_of!(YakVcVoiceFrame, tick), 32);
        assert_eq!(std::mem::offset_of!(YakVcVoiceFrame, volume), 40);
        assert_eq!(std::mem::offset_of!(YakVcVoiceFrame, flags), 48);
        assert_eq!(std::mem::offset_of!(YakVcVoiceFrame, heard_from), 56);
        assert_eq!(std::mem::offset_of!(YakVcVoiceFrame, occlusion), 80);
        assert_eq!(std::mem::offset_of!(YakVcVoiceSink, frame), 24);
    }

    #[test]
    fn frames_queue_per_speaker_and_pull_out_in_order() {
        let uuid = [7; 16];
        deliver(uuid, &[1.0; FRAME], 0);
        deliver(uuid, &[0.5; FRAME], 0);
        let stream = stream(uuid);
        let mut out = vec![9.0; FRAME + 10];
        pull(&stream, &mut out);
        assert!(out[..FRAME].iter().all(|&s| s == 0.5), "at yakvc's volume");
        assert_eq!(out[FRAME..], [0.25; 10]);
        // The rest, then silence once it runs dry.
        let mut rest = vec![9.0; FRAME];
        pull(&stream, &mut rest);
        assert!(rest[..FRAME - 10].iter().all(|&s| s == 0.25));
        assert_eq!(rest[FRAME - 10..], [0.0; 10]);
        let info = streams().into_iter().find(|s| s.0 == uuid).unwrap().1;
        assert_eq!(info.heard_from, [1.0, 2.0, 3.0]);
        assert_eq!(info.occlusion, 0.25);
    }

    #[test]
    fn a_voice_that_runs_dry_buffers_again_before_playing_on() {
        let uuid = [13; 16];
        deliver(uuid, &[1.0; FRAME], 0);
        deliver(uuid, &[1.0; FRAME], 0);
        let stream = stream(uuid);
        let mut out = vec![0.0; 3 * FRAME];
        pull(&stream, &mut out);
        assert_eq!(out[2 * FRAME - 1], 0.5, "played what there was");
        assert_eq!(out[2 * FRAME], 0.0, "then silence");
        // One frame isn't enough to start again.
        deliver(uuid, &[1.0; FRAME], 0);
        pull(&stream, &mut out[..10]);
        assert_eq!(out[..10], [0.0; 10]);
        deliver(uuid, &[1.0; FRAME], 0);
        pull(&stream, &mut out[..10]);
        assert_eq!(out[..10], [0.5; 10]);
    }

    #[test]
    fn a_queue_never_grows_past_its_cap() {
        let uuid = [8; 16];
        for _ in 0..100 {
            deliver(uuid, &[1.0; FRAME], 0);
        }
        assert_eq!(stream(uuid).queue.len(), MAX_QUEUED);
    }

    #[test]
    fn a_late_start_does_not_leave_the_voice_lagging() {
        let uuid = [11; 16];
        for i in 0..10 {
            deliver(uuid, &[i as f32; FRAME], 0);
        }
        // 200 ms queued; OpenAL asks for 20 ms and gets the newest audio.
        let mut out = vec![0.0; FRAME];
        pull(&stream(uuid), &mut out);
        assert_eq!(stream(uuid).queue.len(), LAG_TARGET);
        assert!(out[0] >= 3.0 * 0.5, "{}", out[0]);
    }

    #[test]
    fn the_openal_callback_fills_every_byte() {
        let uuid = [9; 16];
        let mut out = [1.0f32; 8];
        let bytes = (out.len() * 4) as i32;
        let play = |out: &mut [f32; 8]| {
            // SAFETY: the pointer comes from `stream_pointer` and `out` is
            // writable for `bytes` bytes.
            unsafe { callback(stream_pointer(uuid), out.as_mut_ptr().cast(), bytes) }
        };
        // Less than 40 ms queued: silence, holding back, but every byte.
        deliver(uuid, &[0.75; 4], 0);
        assert_eq!(play(&mut out), bytes);
        assert_eq!(out, [0.0; 8], "held back");
        // Enough: it plays, at half volume.
        deliver(uuid, &[0.75; FRAME], 0);
        deliver(uuid, &[0.75; FRAME], 0);
        assert_eq!(play(&mut out), bytes);
        assert_eq!(out, [0.375; 8], "at half volume");
    }

    #[test]
    fn the_openal_callback_writes_silence_when_there_is_no_stream() {
        // A null stream fails to produce samples: the buffer must come back
        // as silence, not as the stale samples OpenAL would re-mix.
        let mut out = [1.0f32; 8];
        let bytes = (out.len() * 4) as i32;
        // SAFETY: `out` is writable for `bytes` bytes; the null user fails.
        let filled = unsafe { callback(std::ptr::null_mut(), out.as_mut_ptr().cast(), bytes) };
        assert_eq!(filled, bytes);
        assert_eq!(out, [0.0; 8], "silence, not the stale buffer");
    }

    #[test]
    fn own_voice_is_kept_only_when_asked_and_polls_are_counted() {
        let uuid = [12; 16];
        let own = |pcm: &[f32]| {
            let frame_struct = YakVcVoiceFrame {
                uuid,
                pcm: pcm.as_ptr(),
                samples: pcm.len(),
                tick: 1,
                volume: 1.0,
                range: 0.0,
                flags: VOICE_SELF | VOICE_UNPOSITIONED,
                heard_from: [0.0; 3],
                occlusion: 0.0,
            };
            // SAFETY: the frame and its samples are valid for the call.
            unsafe { (SINK_WITH_OWN_VOICE.0.frame.unwrap())(std::ptr::null_mut(), &frame_struct) };
        };
        own(&[1.0; FRAME]);
        assert!(
            streams().iter().all(|s| s.0 != uuid),
            "kept while not asked"
        );
        set_own_voice(true);
        own(&[1.0; FRAME]);
        set_own_voice(false);
        let (_, info, queued) = streams().into_iter().find(|s| s.0 == uuid).unwrap();
        assert_eq!(info.flags & VOICE_SELF, VOICE_SELF);
        assert_eq!(queued, FRAME);
        assert_eq!(SINK_WITH_OWN_VOICE.0.version, 2);

        let before = polls();
        // SAFETY: the callback ignores its argument.
        unsafe { (SINK.0.wants_frame.unwrap())(std::ptr::null_mut()) };
        assert!(polls() > before);
    }

    #[test]
    fn the_end_of_a_stream_is_noted() {
        let uuid = [10; 16];
        deliver(uuid, &[1.0; FRAME], VOICE_GROUP | VOICE_UNPOSITIONED);
        // SAFETY: `uuid` is 16 readable bytes.
        unsafe { (SINK.0.stream_end.unwrap())(std::ptr::null_mut(), uuid.as_ptr()) };
        let info = streams().into_iter().find(|s| s.0 == uuid).unwrap().1;
        assert!(info.ended);
        assert_eq!(info.flags, VOICE_GROUP | VOICE_UNPOSITIONED);
    }

    #[test]
    fn a_frame_after_a_stream_end_is_played_not_dropped() {
        let uuid = [14; 16];
        deliver(uuid, &[1.0; FRAME], 0);
        deliver(uuid, &[1.0; FRAME], 0);
        // SAFETY: `uuid` is 16 readable bytes.
        unsafe { (SINK.0.stream_end.unwrap())(std::ptr::null_mut(), uuid.as_ptr()) };
        deliver(uuid, &[3.0; FRAME], 0);
        deliver(uuid, &[3.0; FRAME], 0);
        let mut out = vec![9.0; 2 * FRAME];
        pull(&stream(uuid), &mut out);
        assert!(
            out.iter().all(|&s| s == 1.5),
            "the frames after the end play, without silence or old audio"
        );
    }

    #[test]
    fn the_clock_keeps_its_place_past_a_full_u32_of_frames() {
        let mut clock = Clock::default();
        let start = Instant::now();
        clock.start = Some(start);
        clock.frames = u64::from(u32::MAX);
        // About 994 days in: past where a u32 frame count would wrap to 0.
        let later = start + Duration::from_millis(20 * (u64::from(u32::MAX) + 1) - 40);
        assert!(
            clock.due(later),
            "the clock is still due past a u32 of frames"
        );
        assert_eq!(clock.frames, u64::from(u32::MAX) + 1);
    }

    #[test]
    fn the_clock_wants_a_frame_every_20_ms_with_a_lead() {
        let mut clock = Clock::default();
        let start = Instant::now();
        // At once: the lead's worth (40 ms) plus the first frame.
        let mut due = 0;
        while clock.due(start) {
            due += 1;
        }
        assert_eq!(due, 3);
        assert!(!clock.due(start + Duration::from_millis(10)));
        assert!(clock.due(start + Duration::from_millis(20)));
        // A long stall catches up at most a few frames.
        let mut caught_up = 0;
        while clock.due(start + Duration::from_secs(5)) {
            caught_up += 1;
        }
        assert!(caught_up <= MAX_BACKLOG_FRAMES as usize + 1, "{caught_up}");
    }
}
