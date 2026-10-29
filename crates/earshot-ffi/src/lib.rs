//! C ABI over the Earshot acoustics engine, called from Java through the FFM
//! API.
//!
//! This is the only crate allowed to use `unsafe`. Run `cargo xtask header`
//! after changing any export to regenerate `include/earshot.h`.
//!
//! Conventions, as in yakvc: every `int32_t` return is [`EARSHOT_OK`] or a
//! negative `EARSHOT_ERR_*` code, with a message from [`earshot_last_error`].
//! Positions are block coordinates as `x, y, z` doubles. Native code never
//! keeps a pointer after a call returns. Every function except
//! [`earshot_destroy`] may be called from any thread.

#![allow(unsafe_code)]

mod guard;
mod voice;

use std::ffi::c_void;

use earshot_sim::{
    DiffractionStandard, Engine, Heard, Quality, Room, early_reflections, eax_reverb,
    speed_of_sound, trace_paths,
};
use earshot_world::{BANDS, MAX_REACH_METRES, Material, SECTION_BLOCKS, SectionPos, Vec3, World};

use crate::guard::{FfiError, array_arg, guard, guard_value, out_arg, slice_arg, slice_mut_arg};

/// Version of this C ABI. Java refuses to use a library whose version differs
/// from the one it was built against. Bump on any incompatible change.
/// 2: the engine, its world mirror, transmission and path traces.
/// 3: `earshot_set_listener` and `earshot_hear`.
/// 4: `earshot_room`.
/// 5: `earshot_impulse_response`.
/// 6: Yak VC's voice: `earshot_voice_sink`, `earshot_voice_callback`,
///    `earshot_voice_stream` and `earshot_voice_streams`.
/// 7: `earshot_set_quality` and `earshot_stats`.
/// 8: `earshot_room_at`.
/// 9: `earshot_voice_sink` takes the sink version; `earshot_voice_polls` and
///    `earshot_voice_own`.
/// 10: `earshot_early`.
/// 11: `earshot_set_quality` takes the room rays' reach.
/// 12: `earshot_impulse_response_for`.
/// 13: `earshot_hear` adds the opening's size.
/// 14: section and block bytes pack the fill fraction into the high nibble.
/// 15: `earshot_set_quality` takes the diffraction standard.
/// 16: `earshot_set_air`.
pub const EARSHOT_ABI_VERSION: u32 = 16;

pub const EARSHOT_OK: i32 = 0;
/// A null pointer, unknown material or out-of-range value.
pub const EARSHOT_ERR_INVALID_ARGUMENT: i32 = -1;
/// `earshot_create` was passed a different ABI version.
pub const EARSHOT_ERR_ABI_MISMATCH: i32 = -2;
/// The call panicked.
pub const EARSHOT_ERR_PANIC: i32 = -3;

/// Doubles per segment from `earshot_trace_paths`: from `x, y, z`, to
/// `x, y, z`, the energy at its start, and 1 if it escaped (else 0).
pub const EARSHOT_SEGMENT_DOUBLES: usize = 8;

/// Doubles per source from `earshot_hear`: the low, mid and high gains, where
/// it seems to come from as `x, y, z`, how far it travels, and how big the
/// opening it comes round through is, in square blocks (64 when straight or
/// wide open).
pub const EARSHOT_HEARD_DOUBLES: usize = 8;

/// Doubles from `earshot_room`: the room (decay low, mid and high in
/// seconds, mean free path, enclosure, first reflection distance,
/// scattering), then the 14 EAX reverb parameters for it (density,
/// diffusion, gain, gain HF, gain LF, decay time, decay HF ratio, decay LF
/// ratio, reflections gain, reflections delay, late reverb gain, late reverb
/// delay, echo time, echo depth).
pub const EARSHOT_ROOM_DOUBLES: usize = 21;

/// Doubles per stream from `earshot_voice_streams`: the latest frame's flags
/// (`YAKVC_VOICE_*`), range, volume, occlusion and where it is heard from as
/// `x, y, z`; seconds since that frame (infinite before one); 1 if the
/// stream has ended (else 0); and seconds of voice queued.
pub const EARSHOT_VOICE_DOUBLES: usize = 10;

/// Doubles for `earshot_set_quality`: the room measurement's rays (0 for no
/// room, so no reverb), bounces and interval in seconds; occlusion rays per
/// source (0, 1, 5 or 9; 0 turns occlusion off); how far the pathing field
/// reaches each way in blocks (0 to 64; 0 turns sound round corners off);
/// the share of one core the room measurement may take, above which it
/// uses fewer rays; how far room rays reach in blocks (8 to 256); and
/// which diffraction curve the way round an edge follows (0 for ISO 9613-2,
/// 1 for HJ 2.4's Maekawa curve).
pub const EARSHOT_QUALITY_DOUBLES: usize = 8;

/// Doubles from `earshot_stats`: rays in the latest room measurement (after
/// the budget), its share of one core (smoothed), the latest pathing field's
/// build time in milliseconds, and how far that field reaches (0 for none).
pub const EARSHOT_STATS_DOUBLES: usize = 4;

/// Doubles per source from `earshot_early`: the first reflections' energy
/// (mid band, relative to the direct sound in the open), how much later than
/// the direct sound the first arrives in seconds, where they come from on
/// average as a world-axes `x, y, z` from the listener (no longer than 1),
/// and how many there are.
pub const EARSHOT_EARLY_DOUBLES: usize = 6;

/// Opaque engine handle.
#[derive(Debug)]
pub struct EarshotEngine {
    engine: Engine,
}

impl EarshotEngine {
    /// Changes the world without making the game's thread wait.
    fn world_mut(&self, change: impl FnOnce(&mut World) -> bool + Send + 'static) {
        self.engine.world_change_soon(change);
    }

    fn world<T>(&self, read: impl FnOnce(&World) -> T) -> T {
        self.engine.world(read)
    }
}

/// Runs `f` on a live engine behind the panic barrier.
///
/// # Safety
/// `engine` must be null or live.
unsafe fn with_engine(
    engine: *mut EarshotEngine,
    f: impl FnOnce(&EarshotEngine) -> Result<(), FfiError>,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is null or live; live engines
    // are only freed by `earshot_destroy`, which must be the last call.
    match unsafe { engine.as_ref() } {
        Some(engine) => guard(|| f(engine)),
        None => guard(|| Err(FfiError::invalid("engine is null"))),
    }
}

/// Returns [`EARSHOT_ABI_VERSION`]. Safe to call before anything else.
#[unsafe(no_mangle)]
pub extern "C" fn earshot_abi_version() -> u32 {
    // Infallible today, but held behind the barrier like every other export
    // so a later edit cannot unwind through `extern "C"` into Java's FFM call.
    guard_value(|| EARSHOT_ABI_VERSION, 0)
}

/// Starts an engine with an empty world, and its pathing thread.
///
/// # Safety
/// `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_create(abi_version: u32, out: *mut *mut EarshotEngine) -> i32 {
    guard(|| {
        // SAFETY: the caller guarantees `out` is writable.
        let out = unsafe { out_arg(out, "out") }?;
        *out = std::ptr::null_mut();
        if abi_version != EARSHOT_ABI_VERSION {
            return Err(FfiError::new(
                EARSHOT_ERR_ABI_MISMATCH,
                format!("native ABI is {EARSHOT_ABI_VERSION}, Java expects {abi_version}"),
            ));
        }
        *out = Box::into_raw(Box::new(EarshotEngine {
            engine: Engine::start(),
        }));
        Ok(())
    })
}

/// Stops the pathing thread and frees the engine. Must be the last call on
/// `engine`. Null is ignored.
///
/// # Safety
/// `engine` must come from `earshot_create` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_destroy(engine: *mut EarshotEngine) {
    guard_value(
        || {
            if engine.is_null() {
                return;
            }
            // SAFETY: `engine` came from `Box::into_raw` in `earshot_create`,
            // and the caller never uses it again.
            let engine = unsafe { Box::from_raw(engine) };
            drop(engine);
        },
        (),
    );
}

/// Writes the material names in id order as a JSON array of strings, for
/// Java to check its own list against. `*needed` is the full length; if it
/// exceeds `cap`, nothing is written and the caller retries.
///
/// # Safety
/// `buf` must be writable for `cap` bytes; `needed` must be writable; the
/// `buf` and `needed` regions must not overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_material_names(
    buf: *mut u8,
    cap: usize,
    needed: *mut usize,
) -> i32 {
    guard(|| {
        // SAFETY: the caller guarantees `needed` is writable.
        let needed = unsafe { out_arg(needed, "needed") }?;
        // SAFETY: the caller guarantees `buf` is writable for `cap` bytes.
        let buf = unsafe { slice_mut_arg(buf, cap, "buf") }?;
        let names: Vec<String> = Material::ALL
            .iter()
            .map(|m| format!("\"{}\"", m.name()))
            .collect();
        let json = format!("[{}]", names.join(","));
        if let Some(dest) = buf.get_mut(..json.len()) {
            dest.copy_from_slice(json.as_bytes());
        }
        *needed = json.len();
        Ok(())
    })
}

/// Replaces the section at section coordinates `x, y, z` with `blocks`:
/// 4096 packed block bytes in Minecraft's order, index
/// `(y << 8) | (z << 4) | x` — the material id in the low nibble, how much
/// of the block is there in the high one (0 = full; otherwise sixteenths,
/// so a slab is 8).
///
/// # Safety
/// `engine` must be live; `blocks` must be readable for 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_set_section(
    engine: *mut EarshotEngine,
    x: i32,
    y: i32,
    z: i32,
    blocks: *const u8,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `blocks` is readable for 4096 bytes.
            let blocks: [u8; SECTION_BLOCKS] = array_arg(blocks, "blocks")?;
            if let Some(&byte) = blocks
                .iter()
                .find(|&&byte| byte & 0x0F > Material::Rug.id())
            {
                return Err(FfiError::invalid(format!(
                    "unknown material {}",
                    byte & 0x0F
                )));
            }
            e.world_mut(move |world| world.set_section(SectionPos::new(x, y, z), &blocks));
            Ok(())
        })
    }
}

/// Forgets the section at section coordinates `x, y, z`, as when its chunk
/// unloads.
///
/// # Safety
/// `engine` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_remove_section(
    engine: *mut EarshotEngine,
    x: i32,
    y: i32,
    z: i32,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            e.world_mut(move |world| world.remove_section(SectionPos::new(x, y, z)));
            Ok(())
        })
    }
}

/// Sets block `x, y, z` to a packed block byte, as when the game changes
/// it: the material id in the low nibble, how much of the block is there in
/// the high one (0 = full; otherwise sixteenths, so a slab is 8).
///
/// # Safety
/// `engine` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_set_block(
    engine: *mut EarshotEngine,
    x: i32,
    y: i32,
    z: i32,
    block: u8,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            if block & 0x0F > Material::Rug.id() {
                return Err(FfiError::invalid(format!(
                    "unknown material {}",
                    block & 0x0F
                )));
            }
            e.world_mut(move |world| world.set_block_raw(x, y, z, block));
            Ok(())
        })
    }
}

/// Forgets every section, as when the player changes dimension or leaves.
///
/// # Safety
/// `engine` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_clear_world(engine: *mut EarshotEngine) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            e.world_mut(World::clear);
            Ok(())
        })
    }
}

/// Writes how many sections hold anything but air.
///
/// # Safety
/// `engine` must be live; `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_section_count(engine: *mut EarshotEngine, out: *mut usize) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `out` is writable.
            let out = out_arg(out, "out")?;
            *out = e.world(World::sections);
            Ok(())
        })
    }
}

/// Writes to `out` the share of sound energy, low, mid and high, that gets
/// straight through the blocks between `from` and `to`. The blocks at either
/// end don't count.
///
/// # Safety
/// `engine` must be live; `from` and `to` must point to 3 doubles; `out` must
/// be writable for 3 floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_transmission(
    engine: *mut EarshotEngine,
    from: *const f64,
    to: *const f64,
    out: *mut f32,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `from` and `to` point to 3 doubles.
            let (from, to) = (point_arg(from, "from")?, point_arg(to, "to")?);
            // SAFETY: the caller guarantees `out` is writable for 3 floats.
            let out = slice_mut_arg(out, BANDS, "out")?;
            let through = e.world(|world| earshot_world::transmission(world, from, to));
            out.copy_from_slice(&through);
            Ok(())
        })
    }
}

/// Where the listener is now. The pathing field around it follows on the
/// engine's own thread.
///
/// # Safety
/// `engine` must be live; `listener` must point to 3 doubles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_set_listener(
    engine: *mut EarshotEngine,
    listener: *const f64,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `listener` points to 3 doubles.
            e.engine.set_listener(point_arg(listener, "listener")?);
            Ok(())
        })
    }
}

/// How warm the air is for the timing paths, in °C (see
/// [`earshot_sim::speed_of_sound`]), called on the same clock as
/// [`earshot_set_listener`]. Must be finite and within −60..=60; anything
/// else is refused rather than stored, so a bad reading from the game never
/// reaches the speed maths.
///
/// # Safety
/// `engine` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_set_air(engine: *mut EarshotEngine, celsius: f64) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // `contains` is false for NaN and for both infinities too, so
            // this one range check rejects every unusable reading.
            if !(-60.0..=60.0).contains(&celsius) {
                return Err(FfiError::invalid(format!(
                    "air temperature {celsius} not in -60..=60"
                )));
            }
            e.engine.set_air(celsius);
            Ok(())
        })
    }
}

/// How each of `count` sources reaches `listener`: straight through whatever
/// is in the way, or round it through the air. `sources` holds `x, y, z` per
/// source; `out` gets [`EARSHOT_HEARD_DOUBLES`] doubles per source.
///
/// # Safety
/// `engine` must be live; `listener` must point to 3 doubles, `sources` to
/// `3 * count` doubles, and `out` must be writable for
/// `EARSHOT_HEARD_DOUBLES * count` doubles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_hear(
    engine: *mut EarshotEngine,
    listener: *const f64,
    sources: *const f64,
    count: usize,
    out: *mut f64,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `listener` points to 3 doubles.
            let listener = point_arg(listener, "listener")?;
            let len = count
                .checked_mul(EARSHOT_HEARD_DOUBLES)
                .ok_or_else(|| FfiError::invalid("count is too large"))?;
            // SAFETY: the caller guarantees `sources` points to `3 * count`
            // doubles (`3 * count` fits, being less than `len`), each a
            // usable point.
            let points: Vec<Vec3> = points_arg(sources, count, "sources")?
                .iter()
                .map(|&[x, y, z]| Vec3::new(x, y, z))
                .collect();
            // SAFETY: the caller guarantees `out` is writable for `len` doubles.
            let out = slice_mut_arg(out, len, "out")?;
            let mut heard = vec![
                Heard {
                    aperture: earshot_sim::MAX_APERTURE,
                    gains: [1.0; BANDS],
                    heard_from: Vec3::ZERO,
                    path_length: 0.0,
                };
                count
            ];
            e.engine.hear(listener, &points, &mut heard);
            for (out, heard) in out
                .as_chunks_mut::<EARSHOT_HEARD_DOUBLES>()
                .0
                .iter_mut()
                .zip(&heard)
            {
                let [low, mid, high] = heard.gains.map(f64::from);
                let at = heard.heard_from;
                *out = [
                    low,
                    mid,
                    high,
                    at.x,
                    at.y,
                    at.z,
                    heard.path_length,
                    f64::from(heard.aperture),
                ];
            }
            Ok(())
        })
    }
}

/// Writes the room around the listener and the EAX reverb for it into `out`
/// ([`EARSHOT_ROOM_DOUBLES`] doubles). The engine measures it a few times a
/// second once it knows where the listener is, smoothed; before then it is
/// open air.
///
/// # Safety
/// `engine` must be live; `out` must be writable for `EARSHOT_ROOM_DOUBLES`
/// doubles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_room(engine: *mut EarshotEngine, out: *mut f64) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `out` is writable for
            // `EARSHOT_ROOM_DOUBLES` doubles.
            let out = slice_mut_arg(out, EARSHOT_ROOM_DOUBLES, "out")?;
            let speed = speed_of_sound(e.engine.air_celsius());
            write_room(&e.engine.room().unwrap_or(Room::OPEN_AIR), out, speed);
            Ok(())
        })
    }
}

/// The first reflections off the walls around each of `count` sources, as
/// heard at `listener`: [`EARSHOT_EARLY_DOUBLES`] doubles per source into
/// `out`. For an early-reflections effect on the loudest few sounds.
///
/// # Safety
/// `engine` must be live; `listener` must point to 3 doubles, `sources` to
/// `3 * count` doubles, and `out` must be writable for
/// `EARSHOT_EARLY_DOUBLES * count` doubles; the `sources` and `out` regions
/// must not overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_early(
    engine: *mut EarshotEngine,
    listener: *const f64,
    sources: *const f64,
    count: usize,
    out: *mut f64,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `listener` points to 3 doubles.
            let listener = point_arg(listener, "listener")?;
            let len = count
                .checked_mul(EARSHOT_EARLY_DOUBLES)
                .ok_or_else(|| FfiError::invalid("count is too large"))?;
            // SAFETY: the caller guarantees `sources` points to `3 * count`
            // doubles (which fits, being less than `len`), each a usable
            // point.
            let points = points_arg(sources, count, "sources")?;
            // SAFETY: the caller guarantees `out` is writable for `len` doubles.
            let out = slice_mut_arg(out, len, "out")?;
            e.world(|world| {
                let speed = speed_of_sound(e.engine.air_celsius());
                for (&[x, y, z], out) in points
                    .iter()
                    .zip(out.as_chunks_mut::<EARSHOT_EARLY_DOUBLES>().0)
                {
                    let early = early_reflections(world, listener, Vec3::new(x, y, z), speed);
                    let d = early.direction;
                    *out = [
                        f64::from(early.energy),
                        f64::from(early.delay),
                        d.x,
                        d.y,
                        d.z,
                        f64::from(early.count),
                    ];
                }
            });
            Ok(())
        })
    }
}

/// Measures the room around `at` now, on the calling thread, with `rays` ×
/// `bounces`, and writes it with the EAX reverb for it into `out`
/// ([`EARSHOT_ROOM_DOUBLES`] doubles, as `earshot_room`). For the reverb of
/// the space a sound is in when the listener is elsewhere. A point inside a
/// solid block is measured from the open block beside it.
///
/// # Safety
/// `engine` must be live; `at` must point to 3 doubles; `out` must be
/// writable for `EARSHOT_ROOM_DOUBLES` doubles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_room_at(
    engine: *mut EarshotEngine,
    at: *const f64,
    rays: u32,
    bounces: u32,
    seed: u64,
    out: *mut f64,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `at` points to 3 doubles.
            let at = point_arg(at, "at")?;
            // SAFETY: the caller guarantees `out` is writable for
            // `EARSHOT_ROOM_DOUBLES` doubles.
            let out = slice_mut_arg(out, EARSHOT_ROOM_DOUBLES, "out")?;
            if !(1..=1 << 14).contains(&rays) || bounces > 64 {
                return Err(FfiError::invalid("rays or bounces out of range"));
            }
            write_room(
                &e.engine.room_at(at, rays, bounces, seed),
                out,
                speed_of_sound(e.engine.air_celsius()),
            );
            Ok(())
        })
    }
}

/// Writes `room` and the EAX reverb for it as `earshot_room` does.
fn write_room(room: &Room, out: &mut [f64], speed: f64) {
    let [low, mid, high] = room.decay.map(f64::from);
    out[..7].copy_from_slice(&[
        low,
        mid,
        high,
        room.mean_free_path,
        f64::from(room.enclosure),
        room.first_reflection,
        f64::from(room.scattering),
    ]);
    for (out, value) in out[7..].iter_mut().zip(eax_reverb(room, speed).to_array()) {
        *out = f64::from(value);
    }
}

/// Synthesizes an impulse response for the room around the listener, for
/// OpenAL Soft's convolution effect: first-order ambisonics in FuMa order and
/// scaling (W, X, Y, Z interleaved, `AL_FORMAT_BFORMAT3D_FLOAT32`) in world
/// axes, at most `max_seconds` long. Writes it to `out` if `cap` floats are
/// enough, and how many floats it needs to `*needed` either way; `*needed`
/// is 0 before the room has been measured.
///
/// # Safety
/// `engine` must be live; `out` must be writable for `cap` floats; `needed`
/// must be writable; the `out` and `needed` regions must not overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_impulse_response(
    engine: *mut EarshotEngine,
    sample_rate: u32,
    max_seconds: f32,
    seed: u64,
    out: *mut f32,
    cap: usize,
    needed: *mut usize,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `needed` is writable.
            let needed = out_arg(needed, "needed")?;
            // SAFETY: the caller guarantees `out` is writable for `cap` floats.
            let out = slice_mut_arg(out, cap, "out")?;
            if !(8_000..=192_000).contains(&sample_rate) || !(0.1..=10.0).contains(&max_seconds) {
                return Err(FfiError::invalid("sample rate or length out of range"));
            }
            let ir = e
                .engine
                .impulse_response(sample_rate, max_seconds, seed)
                .unwrap_or_default();
            if let Some(dest) = out.get_mut(..ir.len()) {
                dest.copy_from_slice(&ir);
            }
            *needed = ir.len();
            Ok(())
        })
    }
}

/// [`earshot_impulse_response`] for a sound at `source` (3 doubles) as the
/// listener hears it: the room measured round the sound with `rays` ×
/// `bounces` (on the calling thread), and its first reflections off the
/// walls round it, timed after its direct sound. `*needed` is 0 before the
/// listener is known.
///
/// # Safety
/// `engine` must be live; `source` must point to 3 doubles; `out` must be
/// writable for `cap` floats; `needed` must be writable; the `out` and
/// `needed` regions must not overlap.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_impulse_response_for(
    engine: *mut EarshotEngine,
    source: *const f64,
    rays: u32,
    bounces: u32,
    sample_rate: u32,
    max_seconds: f32,
    seed: u64,
    out: *mut f32,
    cap: usize,
    needed: *mut usize,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `source` points to 3 doubles.
            let source = point_arg(source, "source")?;
            // SAFETY: the caller guarantees `needed` is writable.
            let needed = out_arg(needed, "needed")?;
            // SAFETY: the caller guarantees `out` is writable for `cap` floats.
            let out = slice_mut_arg(out, cap, "out")?;
            if !(8_000..=192_000).contains(&sample_rate)
                || !(0.1..=10.0).contains(&max_seconds)
                || !(1..=1 << 14).contains(&rays)
                || bounces > 64
            {
                return Err(FfiError::invalid(
                    "rate, length, rays or bounces out of range",
                ));
            }
            let ir = e
                .engine
                .impulse_response_for(source, rays, bounces, sample_rate, max_seconds, seed)
                .unwrap_or_default();
            if let Some(dest) = out.get_mut(..ir.len()) {
                dest.copy_from_slice(&ir);
            }
            *needed = ir.len();
            Ok(())
        })
    }
}

/// Traces `rays` paths from `origin`, evenly spread, with up to `bounces`
/// reflections and at most `reach` blocks per leg. Writes the first `cap`
/// legs to `out`, [`EARSHOT_SEGMENT_DOUBLES`] doubles each, and how many it
/// wrote to `*written`. `stats` gets four doubles: legs traced, rays that
/// escaped on their first leg, legs that escaped, and the mean free path in
/// blocks.
///
/// # Safety
/// `engine` must be live; `origin` must point to 3 doubles; `out` must be
/// writable for `cap * EARSHOT_SEGMENT_DOUBLES` doubles; `written` must be
/// writable; `stats` must be writable for 4 doubles; the `out`, `written`
/// and `stats` regions must not overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_trace_paths(
    engine: *mut EarshotEngine,
    origin: *const f64,
    rays: u32,
    bounces: u32,
    reach: f64,
    seed: u64,
    out: *mut f64,
    cap: usize,
    written: *mut usize,
    stats: *mut f64,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `origin` points to 3 doubles.
            let origin = point_arg(origin, "origin")?;
            let len = cap
                .checked_mul(EARSHOT_SEGMENT_DOUBLES)
                .ok_or_else(|| FfiError::invalid("cap is too large"))?;
            // SAFETY: the caller guarantees `out` is writable for `len` doubles.
            let out = slice_mut_arg(out, len, "out")?;
            // SAFETY: the caller guarantees `written` is writable.
            let written = out_arg(written, "written")?;
            // SAFETY: the caller guarantees `stats` is writable for 4 doubles.
            let stats = slice_mut_arg(stats, 4, "stats")?;
            // A reach past the bound would walk the ray until its block
            // coordinates overflowed, so it is refused here.
            if !(reach.is_finite() && reach > 0.0 && reach <= MAX_REACH_METRES)
                || rays > 1 << 20
                || bounces > 64
            {
                return Err(FfiError::invalid("rays, bounces or reach out of range"));
            }
            let mut legs = out.as_chunks_mut::<EARSHOT_SEGMENT_DOUBLES>().0.iter_mut();
            *written = 0;
            let traced = e.world(|world| {
                trace_paths(world, origin, rays, bounces, reach, seed, |segment| {
                    let Some(leg) = legs.next() else {
                        return;
                    };
                    let (a, b) = (segment.from, segment.to);
                    let escaped = if segment.escaped { 1.0 } else { 0.0 };
                    let energy = f64::from(segment.energy);
                    *leg = [a.x, a.y, a.z, b.x, b.y, b.z, energy, escaped];
                    *written += 1;
                })
            });
            stats.copy_from_slice(&[
                f64::from(traced.segments),
                f64::from(traced.escaped_first),
                f64::from(traced.escaped),
                traced.mean_free_path,
            ]);
            Ok(())
        })
    }
}

/// The address of Earshot's `YakVcVoiceSink` (`yakvc.h`) as `version` 1
/// (other players' voices) or 2 (also the player's own, as
/// `YAKVC_VOICE_SELF` frames), for Java to offer Yak VC in Fabric's object
/// share under `yakvc:voice_sink`; null for another version. Both live for
/// the rest of the process and need no engine: Yak VC may call them from its
/// audio thread at any time.
#[unsafe(no_mangle)]
pub extern "C" fn earshot_voice_sink(version: u32) -> *const c_void {
    // Static addresses only, but held behind the barrier like every export:
    // unwinding through `extern "C"` aborts the process.
    guard_value(
        || match version {
            1 => std::ptr::from_ref(&voice::SINK.0).cast(),
            2 => std::ptr::from_ref(&voice::SINK_WITH_OWN_VOICE.0).cast(),
            _ => std::ptr::null(),
        },
        std::ptr::null(),
    )
}

/// How often Yak VC has asked a sink for a frame. It asks every few
/// milliseconds once it has taken one, so a count that stays put after
/// offering a sink means Yak VC refused it.
#[unsafe(no_mangle)]
pub extern "C" fn earshot_voice_polls() -> u64 {
    guard_value(voice::polls, 0)
}

/// Keeps the player's own voice (`YAKVC_VOICE_SELF` frames) as a stream
/// from now on, or drops it as it comes.
#[unsafe(no_mangle)]
pub extern "C" fn earshot_voice_own(keep: bool) {
    guard_value(|| voice::set_own_voice(keep), ());
}

/// The address of the OpenAL buffer callback (`ALBUFFERCALLBACKTYPESOFT`)
/// that plays a voice stream, for `alBufferCallbackSOFT` with mono float32
/// at 48 kHz and the stream's pointer from `earshot_voice_stream`.
#[unsafe(no_mangle)]
pub extern "C" fn earshot_voice_callback() -> *const c_void {
    guard_value(
        || {
            let callback: unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> i32 =
                voice::callback;
            callback as *const c_void
        },
        std::ptr::null(),
    )
}

/// Writes to `*out` the pointer that `earshot_voice_callback` plays the
/// voice of speaker `uuid` (16 bytes) from. It stays valid for the rest of
/// the process. A speaker heard before keeps its stream for the rest of the
/// process; a new one once the voice-stream cap (Java's `MAX_VOICE_STREAMS`)
/// is reached fails with [`EARSHOT_ERR_INVALID_ARGUMENT`] and writes null.
///
/// # Safety
/// `uuid` must point to 16 bytes; `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_voice_stream(uuid: *const u8, out: *mut *mut c_void) -> i32 {
    guard(|| {
        // SAFETY: the caller guarantees `uuid` points to 16 bytes.
        let uuid: [u8; 16] = unsafe { array_arg(uuid, "uuid") }?;
        // SAFETY: the caller guarantees `out` is writable.
        let out = unsafe { out_arg(out, "out") }?;
        *out = std::ptr::null_mut();
        *out = voice::stream_pointer(uuid)
            .ok_or_else(|| FfiError::invalid("voice stream cap reached"))?;
        Ok(())
    })
}

/// Lists the voice streams heard this session: writes the first `cap`
/// speakers' UUIDs to `uuids` (16 bytes each) and [`EARSHOT_VOICE_DOUBLES`]
/// doubles each to `info`, and how many there are to `*count`.
///
/// # Safety
/// `uuids` must be writable for `16 * cap` bytes, `info` for
/// `EARSHOT_VOICE_DOUBLES * cap` doubles; `count` must be writable; the
/// `uuids`, `info` and `count` regions must not overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_voice_streams(
    uuids: *mut u8,
    info: *mut f64,
    cap: usize,
    count: *mut usize,
) -> i32 {
    guard(|| {
        let too_large = || FfiError::invalid("cap is too large");
        let uuid_bytes = cap.checked_mul(16).ok_or_else(too_large)?;
        let info_len = cap
            .checked_mul(EARSHOT_VOICE_DOUBLES)
            .ok_or_else(too_large)?;
        // SAFETY: the caller guarantees `uuids` is writable for `16 * cap`
        // bytes.
        let uuids = unsafe { slice_mut_arg(uuids, uuid_bytes, "uuids") }?;
        // SAFETY: the caller guarantees `info` is writable for `info_len`
        // doubles.
        let info = unsafe { slice_mut_arg(info, info_len, "info") }?;
        // SAFETY: the caller guarantees `count` is writable.
        let count = unsafe { out_arg(count, "count") }?;
        let streams = voice::streams();
        let now = std::time::Instant::now();
        for ((uuid, out), (id, stream, queued)) in uuids
            .as_chunks_mut::<16>()
            .0
            .iter_mut()
            .zip(info.as_chunks_mut::<EARSHOT_VOICE_DOUBLES>().0)
            .zip(&streams)
        {
            *uuid = *id;
            let [x, y, z] = stream.heard_from;
            let age = stream
                .last_frame
                .map_or(f64::INFINITY, |at| now.duration_since(at).as_secs_f64());
            *out = [
                f64::from(stream.flags),
                f64::from(stream.range),
                f64::from(stream.volume),
                f64::from(stream.occlusion),
                x,
                y,
                z,
                age,
                if stream.ended { 1.0 } else { 0.0 },
                *queued as f64 / 48_000.0,
            ];
        }
        *count = streams.len();
        Ok(())
    })
}

/// Sets how much the engine traces from now on (see
/// [`EARSHOT_QUALITY_DOUBLES`]).
///
/// # Safety
/// `engine` must be live; `quality` must point to `EARSHOT_QUALITY_DOUBLES`
/// doubles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_set_quality(
    engine: *mut EarshotEngine,
    quality: *const f64,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `quality` points to
            // `EARSHOT_QUALITY_DOUBLES` doubles.
            let [
                rays,
                bounces,
                every,
                occlusion,
                half,
                budget,
                reach,
                diffraction,
            ] = array_arg::<f64, EARSHOT_QUALITY_DOUBLES>(quality, "quality")?;
            let whole = |v: f64| v.is_finite() && v >= 0.0 && v.fract() == 0.0 && v <= 1e6;
            if !(whole(rays) && whole(bounces) && whole(occlusion) && whole(half))
                || !(every.is_finite() && budget.is_finite() && reach.is_finite())
                || !(diffraction.is_finite() && diffraction.fract() == 0.0 && diffraction >= 0.0)
            {
                return Err(FfiError::invalid("quality values out of range"));
            }
            let diffraction = match diffraction as u32 {
                0 => DiffractionStandard::Iso9613_2,
                1 => DiffractionStandard::Hj24,
                _ => {
                    return Err(FfiError::invalid(format!(
                        "diffraction standard {diffraction} not in 0..=1"
                    )));
                }
            };
            let quality = Quality {
                room_rays: rays as u32,
                room_bounces: bounces as u32,
                room_every: std::time::Duration::from_secs_f64(every.clamp(0.0, 60.0)),
                occlusion_rays: occlusion as u32,
                field_half: half as i32,
                room_reach: reach,
                budget: budget as f32,
                diffraction,
            };
            if !quality.is_valid() {
                return Err(FfiError::invalid(format!(
                    "quality out of range: {quality:?}"
                )));
            }
            e.engine.set_quality(quality);
            Ok(())
        })
    }
}

/// Writes what the engine is spending into `out` (see
/// [`EARSHOT_STATS_DOUBLES`]).
///
/// # Safety
/// `engine` must be live; `out` must be writable for `EARSHOT_STATS_DOUBLES`
/// doubles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_stats(engine: *mut EarshotEngine, out: *mut f64) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            // SAFETY: the caller guarantees `out` is writable for
            // `EARSHOT_STATS_DOUBLES` doubles.
            let out = slice_mut_arg(out, EARSHOT_STATS_DOUBLES, "out")?;
            let stats = e.engine.stats();
            out.copy_from_slice(&[
                f64::from(stats.room_rays),
                f64::from(stats.room_cost),
                stats.field_build.as_secs_f64() * 1e3,
                f64::from(stats.field_half),
            ]);
            Ok(())
        })
    }
}

/// Copies this thread's last error message into `buf` (truncated to `cap`,
/// at a character boundary) and returns how many bytes it wrote, or 0 if
/// there is none. The return never counts bytes `buf` did not receive, so a
/// caller decoding exactly that many finds no stray characters past the
/// message.
///
/// # Safety
/// `buf` must be writable for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_last_error(buf: *mut u8, cap: usize) -> usize {
    // The one export that reads rather than reports a code: its length
    // return cannot be an error code, so a panic falls back to 0 ("no
    // message") instead of unwinding into the FFM call.
    guard_value(
        || {
            let message = guard::last_error();
            // SAFETY: the caller guarantees `buf` is writable for `cap` bytes.
            let Ok(buf) = (unsafe { slice_mut_arg(buf, cap, "buf") }) else {
                return 0;
            };
            let n = message.floor_char_boundary(buf.len());
            buf[..n].copy_from_slice(&message.as_bytes()[..n]);
            n
        },
        0,
    )
}

/// A finite point from 3 doubles, within the range the sim's block
/// arithmetic can carry (see [`Vec3::is_usable`]).
///
/// # Safety
/// `ptr` must be null or point to 3 doubles.
unsafe fn point_arg(ptr: *const f64, what: &str) -> Result<Vec3, FfiError> {
    // SAFETY: forwarded from the caller.
    let [x, y, z] = unsafe { array_arg::<f64, 3>(ptr, what) }?;
    let point = Vec3::new(x, y, z);
    if !point.is_usable() {
        return Err(FfiError::invalid(format!(
            "{what} is not finite or out of range"
        )));
    }
    Ok(point)
}

/// The `count` source points at `ptr`, each a finite, in-range `[x, y, z]`:
/// the same check [`point_arg`] makes of one listener, over a whole list, so
/// one bad point from any mod fails the call rather than reaching the engine.
///
/// # Safety
/// `ptr` must be null or point to `3 * count` doubles.
unsafe fn points_arg<'a>(
    ptr: *const f64,
    count: usize,
    what: &str,
) -> Result<&'a [[f64; 3]], FfiError> {
    // SAFETY: forwarded from the caller.
    let xyz = unsafe { slice_arg(ptr, 3 * count, what) }?;
    let points = xyz.as_chunks::<3>().0;
    if !points
        .iter()
        .all(|&[x, y, z]| Vec3::new(x, y, z).is_usable())
    {
        return Err(FfiError::invalid("sources are not finite or out of range"));
    }
    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_version_matches_constant() {
        assert_eq!(earshot_abi_version(), EARSHOT_ABI_VERSION);
    }

    /// `cargo xtask header --check` keeps the header current, but only when
    /// it runs: this catches a function added without regenerating it.
    #[test]
    fn header_declares_every_exported_function() {
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut exported = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(crate_dir.join("src")).unwrap() {
            let source = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            for after in source.split("#[unsafe(no_mangle)]\npub ").skip(1) {
                let (_, function) = after.split_once("extern \"C\" fn ").unwrap();
                exported.insert(identifier(function));
            }
        }
        let header = std::fs::read_to_string(crate_dir.join("include/earshot.h")).unwrap();
        let declared: std::collections::BTreeSet<String> = header
            .match_indices("earshot_")
            .map(|(at, _)| identifier(&header[at..]))
            .filter(|name| header.contains(&format!("{name}(")))
            .collect();
        assert!(!exported.is_empty());
        assert_eq!(exported, declared, "run `cargo xtask header`");
    }

    /// The identifier `text` starts with.
    fn identifier(text: &str) -> String {
        text.chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect()
    }

    fn engine() -> *mut EarshotEngine {
        let mut engine = std::ptr::null_mut();
        // SAFETY: `engine` is writable.
        let code = unsafe { earshot_create(EARSHOT_ABI_VERSION, &mut engine) };
        assert_eq!(code, EARSHOT_OK);
        engine
    }

    #[test]
    fn a_wall_set_through_the_abi_muffles() {
        let engine = engine();
        let mut blocks = [0u8; SECTION_BLOCKS];
        // A wall of rock at x = 8 across the whole section.
        for y in 0..16 {
            for z in 0..16 {
                blocks[(y << 8) | (z << 4) | 8] = Material::Rock.id();
            }
        }
        let (from, to) = ([2.5, 8.5, 8.5], [14.5, 8.5, 8.5]);
        let mut through = [0f32; 3];
        let mut count = 0;
        // SAFETY: `engine` is live and every buffer is valid for its length.
        unsafe {
            assert_eq!(
                earshot_set_section(engine, 0, 0, 0, blocks.as_ptr()),
                EARSHOT_OK
            );
            assert_eq!(earshot_section_count(engine, &mut count), EARSHOT_OK);
            let code =
                earshot_transmission(engine, from.as_ptr(), to.as_ptr(), through.as_mut_ptr());
            assert_eq!(code, EARSHOT_OK);
        }
        assert_eq!(count, 1);
        assert_eq!(through, Material::Rock.acoustics().transmission);
        // SAFETY: `engine` is live, then never used again.
        unsafe {
            assert_eq!(earshot_set_block(engine, 8, 8, 8, 0), EARSHOT_OK);
            assert_eq!(
                earshot_set_block(engine, 0, 0, 0, 14),
                EARSHOT_ERR_INVALID_ARGUMENT
            );
            earshot_destroy(engine);
        }
    }

    #[test]
    fn traces_fill_the_buffer_and_report_stats() {
        let engine = engine();
        let origin = [0.5, 0.5, 0.5];
        let mut out = vec![0.0; 10 * EARSHOT_SEGMENT_DOUBLES];
        let (mut written, mut stats) = (0, [0.0; 4]);
        // SAFETY: `engine` is live and every buffer is valid for its length.
        let code = unsafe {
            earshot_trace_paths(
                engine,
                origin.as_ptr(),
                32,
                4,
                16.0,
                1,
                out.as_mut_ptr(),
                10,
                &mut written,
                stats.as_mut_ptr(),
            )
        };
        assert_eq!(code, EARSHOT_OK);
        // An empty world: every ray escapes on its first leg.
        assert_eq!(written, 10);
        assert_eq!(stats[..3], [32.0, 32.0, 32.0]);
        assert_eq!(out[7], 1.0, "escaped");
        // SAFETY: `engine` is live, then never used again.
        unsafe { earshot_destroy(engine) };
    }

    #[test]
    fn a_path_ray_has_a_reach_limit() {
        let engine = engine();
        let origin = [0.5, 0.5, 0.5];
        let mut out = [0.0; EARSHOT_SEGMENT_DOUBLES];
        let (mut written, mut stats) = (0, [0.0; 4]);
        // SAFETY: `engine` is live and every buffer is valid for its length.
        unsafe {
            // A reach past the limit would walk the ray until its block
            // coordinates overflowed, so it is refused.
            assert_eq!(
                earshot_trace_paths(
                    engine,
                    origin.as_ptr(),
                    1,
                    0,
                    1e300,
                    1,
                    out.as_mut_ptr(),
                    1,
                    &mut written,
                    stats.as_mut_ptr(),
                ),
                EARSHOT_ERR_INVALID_ARGUMENT
            );
            for reach in [64.0, 4096.0] {
                assert_eq!(
                    earshot_trace_paths(
                        engine,
                        origin.as_ptr(),
                        1,
                        0,
                        reach,
                        1,
                        out.as_mut_ptr(),
                        1,
                        &mut written,
                        stats.as_mut_ptr(),
                    ),
                    EARSHOT_OK,
                    "reach {reach}"
                );
            }
            earshot_destroy(engine);
        }
    }

    #[test]
    fn hearing_a_batch_of_sources() {
        let engine = engine();
        let listener = [0.5, 0.5, 0.5];
        let sources = [5.5, 0.5, 0.5, 0.5, 0.5, 8.5];
        let mut out = [0.0; 2 * EARSHOT_HEARD_DOUBLES];
        // SAFETY: `engine` is live and every buffer is valid for its length.
        unsafe {
            assert_eq!(
                earshot_set_block(engine, 3, 0, 0, Material::Rock.id()),
                EARSHOT_OK
            );
            assert_eq!(earshot_set_listener(engine, listener.as_ptr()), EARSHOT_OK);
            let code = earshot_hear(
                engine,
                listener.as_ptr(),
                sources.as_ptr(),
                2,
                out.as_mut_ptr(),
            );
            assert_eq!(code, EARSHOT_OK);
        }
        // Behind the rock block: muffled. In the open: as is, from where it is.
        assert!(out[1] < 0.9, "{out:?}");
        let open = &out[EARSHOT_HEARD_DOUBLES..];
        assert_eq!(open[..3], [1.0, 1.0, 1.0]);
        assert_eq!(open[3..6], [0.5, 0.5, 8.5]);
        // SAFETY: `engine` is live, then never used again.
        unsafe { earshot_destroy(engine) };
    }

    #[test]
    fn an_impulse_response_comes_once_the_room_is_measured() {
        let engine = engine();
        let listener = [0.5, 0.5, 0.5];
        let mut needed = usize::MAX;
        // SAFETY: `engine` is live; a null buffer with capacity 0 is not written.
        unsafe {
            assert_eq!(
                earshot_set_block(engine, 3, 0, 0, Material::Rock.id()),
                EARSHOT_OK
            );
            let code = earshot_impulse_response(
                engine,
                48_000,
                1.0,
                1,
                std::ptr::null_mut(),
                0,
                &mut needed,
            );
            assert_eq!(code, EARSHOT_OK);
            assert_eq!(earshot_set_listener(engine, listener.as_ptr()), EARSHOT_OK);
        }
        // Not measured yet, or measured: either way a whole number of frames.
        assert_eq!(needed % 4, 0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while needed == 0 || needed == usize::MAX {
            assert!(std::time::Instant::now() < deadline, "no impulse response");
            std::thread::sleep(std::time::Duration::from_millis(20));
            // SAFETY: as above.
            let code = unsafe {
                earshot_impulse_response(
                    engine,
                    48_000,
                    1.0,
                    1,
                    std::ptr::null_mut(),
                    0,
                    &mut needed,
                )
            };
            assert_eq!(code, EARSHOT_OK);
        }
        let mut ir = vec![0.0f32; needed];
        // SAFETY: `engine` is live and `ir` is writable for its length.
        let code = unsafe {
            earshot_impulse_response(
                engine,
                48_000,
                1.0,
                1,
                ir.as_mut_ptr(),
                ir.len(),
                &mut needed,
            )
        };
        assert_eq!(code, EARSHOT_OK);
        assert!(ir.iter().all(|s| s.is_finite()));
        // SAFETY: `engine` is live, then never used again.
        unsafe { earshot_destroy(engine) };
    }

    #[test]
    fn the_room_starts_as_open_air() {
        let engine = engine();
        let mut out = [f64::NAN; EARSHOT_ROOM_DOUBLES];
        // SAFETY: `engine` is live and `out` is writable for its length.
        let code = unsafe { earshot_room(engine, out.as_mut_ptr()) };
        assert_eq!(code, EARSHOT_OK);
        assert!(out.iter().all(|v| v.is_finite()), "{out:?}");
        assert_eq!(out[4], 0.0, "not enclosed");
        // SAFETY: `engine` is live, then never used again.
        unsafe { earshot_destroy(engine) };
    }

    #[test]
    fn material_names_are_json_in_id_order() {
        let mut buf = [0u8; 256];
        let mut needed = 0;
        // SAFETY: `buf` and `needed` are writable.
        let code = unsafe { earshot_material_names(buf.as_mut_ptr(), buf.len(), &mut needed) };
        assert_eq!(code, EARSHOT_OK);
        let json = std::str::from_utf8(&buf[..needed]).unwrap();
        assert!(json.starts_with("[\"air\",\"rock\",\"wood\""), "{json}");
    }

    #[test]
    fn voice_reaches_the_streams_through_the_sink() {
        let uuid = [42u8; 16];
        let pcm = [0.5f32; 960];
        assert!(earshot_voice_sink(3).is_null());
        let sink = earshot_voice_sink(1).cast::<voice::YakVcVoiceSink>();
        // SAFETY: the sink is a static `YakVcVoiceSink`; the frame and its
        // samples are valid for the call, as yakvc promises.
        unsafe {
            assert_eq!((*sink).version, 1);
            let frame = voice::YakVcVoiceFrame {
                uuid,
                pcm: pcm.as_ptr(),
                samples: pcm.len(),
                tick: 1,
                volume: 1.0,
                range: 32.0,
                flags: 0,
                heard_from: [4.0, 5.0, 6.0],
                occlusion: 0.0,
            };
            // Two frames: a voice starts once 40 ms is queued.
            ((*sink).frame.unwrap())((*sink).user, &frame);
            ((*sink).frame.unwrap())((*sink).user, &frame);
        }
        let mut uuids = [0u8; 16 * 64];
        let mut info = [0.0; EARSHOT_VOICE_DOUBLES * 64];
        let (mut count, mut stream) = (0, std::ptr::null_mut());
        // SAFETY: every buffer is valid for its length.
        unsafe {
            assert_eq!(
                earshot_voice_streams(uuids.as_mut_ptr(), info.as_mut_ptr(), 64, &mut count),
                EARSHOT_OK
            );
            assert_eq!(earshot_voice_stream(uuid.as_ptr(), &mut stream), EARSHOT_OK);
        }
        let at = (0..count)
            .find(|&i| uuids[16 * i..16 * i + 16] == uuid)
            .unwrap();
        let info = &info[EARSHOT_VOICE_DOUBLES * at..][..EARSHOT_VOICE_DOUBLES];
        assert_eq!(info[1..7], [32.0, 1.0, 0.0, 4.0, 5.0, 6.0]);
        assert!(info[7] < 1.0 && info[8] == 0.0, "{info:?}");
        assert_eq!(info[9], 0.04);
        // OpenAL's callback plays it.
        let callback: unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> i32 =
            // SAFETY: `earshot_voice_callback` is that function.
            unsafe { std::mem::transmute(earshot_voice_callback()) };
        let mut out = [0.0f32; 960];
        // SAFETY: `stream` came from `earshot_voice_stream`; `out` is
        // writable for its length in bytes.
        let bytes = unsafe { callback(stream, out.as_mut_ptr().cast(), 960 * 4) };
        assert_eq!(bytes, 960 * 4);
        assert_eq!(out, pcm);
    }

    #[test]
    fn quality_is_checked_and_stats_follow_it() {
        let engine = engine();
        let off = [0.0, 0.0, 0.1, 0.0, 0.0, 0.25, 64.0, 0.0];
        let bad = [1024.0, 8.0, 0.1, 3.0, 24.0, 0.25, 64.0, 0.0];
        let unknown_diffraction = [0.0, 0.0, 0.1, 0.0, 0.0, 0.25, 64.0, 2.0];
        let mut stats = [f64::NAN; EARSHOT_STATS_DOUBLES];
        // SAFETY: `engine` is live and every buffer is valid for its length.
        unsafe {
            assert_eq!(earshot_set_quality(engine, off.as_ptr()), EARSHOT_OK);
            assert_eq!(
                earshot_set_quality(engine, bad.as_ptr()),
                EARSHOT_ERR_INVALID_ARGUMENT
            );
            assert_eq!(
                earshot_set_quality(engine, unknown_diffraction.as_ptr()),
                EARSHOT_ERR_INVALID_ARGUMENT,
                "diffraction standard must be 0 (ISO) or 1 (HJ 2.4)"
            );
            assert_eq!(earshot_stats(engine, stats.as_mut_ptr()), EARSHOT_OK);
            earshot_destroy(engine);
        }
        assert!(stats.iter().all(|v| v.is_finite()), "{stats:?}");
    }

    #[test]
    fn air_temperature_is_accepted_in_band_and_refused_outside() {
        let engine = engine();
        // SAFETY: `engine` is live.
        unsafe {
            assert_eq!(earshot_set_air(engine, 12.5), EARSHOT_OK, "a sane reading");
            // Both ends of the band are inside it.
            for edge in [-60.0, 60.0] {
                assert_eq!(earshot_set_air(engine, edge), EARSHOT_OK, "{edge} °C");
            }
            for absurd in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 200.0, -200.0] {
                assert_eq!(
                    earshot_set_air(engine, absurd),
                    EARSHOT_ERR_INVALID_ARGUMENT,
                    "{absurd}"
                );
            }
            // The engine still takes work afterwards.
            assert_eq!(earshot_set_air(engine, 18.0), EARSHOT_OK);
            earshot_destroy(engine);
        }
    }

    #[test]
    fn absurd_coordinates_are_refused_and_nothing_breaks() {
        let engine = engine();
        let huge = [9e18, -9e18, 9e18];
        let listener = [0.5, 0.5, 0.5];
        let sources = [0.5, 0.5, 0.5, 9e18, 0.0, 0.0];
        let mut out = [0.0; 2 * EARSHOT_HEARD_DOUBLES];
        // SAFETY: `engine` is live and every buffer is valid for its length.
        unsafe {
            assert_eq!(
                earshot_set_listener(engine, huge.as_ptr()),
                EARSHOT_ERR_INVALID_ARGUMENT
            );
            assert_eq!(
                earshot_hear(
                    engine,
                    listener.as_ptr(),
                    sources.as_ptr(),
                    2,
                    out.as_mut_ptr()
                ),
                EARSHOT_ERR_INVALID_ARGUMENT
            );
            // The engine still takes work afterwards.
            assert_eq!(earshot_set_listener(engine, listener.as_ptr()), EARSHOT_OK);
            earshot_destroy(engine);
        }
    }

    #[test]
    fn nan_coordinates_are_refused_and_nothing_breaks() {
        let engine = engine();
        let nan = [f64::NAN, 0.0, 0.0];
        let listener = [0.5, 0.5, 0.5];
        let sources = [0.5, 0.5, 0.5, f64::NAN, 0.0, 0.0];
        let mut out = [0.0; 2 * EARSHOT_HEARD_DOUBLES];
        // SAFETY: `engine` is live and every buffer is valid for its length.
        unsafe {
            assert_eq!(
                earshot_set_listener(engine, nan.as_ptr()),
                EARSHOT_ERR_INVALID_ARGUMENT
            );
            assert_eq!(
                earshot_hear(
                    engine,
                    listener.as_ptr(),
                    sources.as_ptr(),
                    2,
                    out.as_mut_ptr()
                ),
                EARSHOT_ERR_INVALID_ARGUMENT
            );
            // The engine still takes work afterwards.
            assert_eq!(earshot_set_listener(engine, listener.as_ptr()), EARSHOT_OK);
            earshot_destroy(engine);
        }
    }

    #[test]
    fn a_null_engine_is_refused() {
        let mut count = 0;
        // SAFETY: the null engine is refused before anything else is used.
        let code = unsafe { earshot_section_count(std::ptr::null_mut(), &mut count) };
        assert_eq!(code, EARSHOT_ERR_INVALID_ARGUMENT);
    }
}
