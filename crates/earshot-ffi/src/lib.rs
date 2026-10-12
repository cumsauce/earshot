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

use earshot_sim::{Engine, Heard, Room, eax_reverb, trace_paths};
use earshot_world::{BANDS, Material, SECTION_BLOCKS, SectionPos, Vec3, World};

use crate::guard::{FfiError, array_arg, guard, out_arg, slice_arg, slice_mut_arg};

/// Version of this C ABI. Java refuses to use a library whose version differs
/// from the one it was built against. Bump on any incompatible change.
/// 2: the engine, its world mirror, transmission and path traces.
/// 3: `earshot_set_listener` and `earshot_hear`.
/// 4: `earshot_room`.
/// 5: `earshot_impulse_response`.
pub const EARSHOT_ABI_VERSION: u32 = 5;

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
/// it seems to come from as `x, y, z`, and how far it travels.
pub const EARSHOT_HEARD_DOUBLES: usize = 7;

/// Doubles from `earshot_room`: the room (decay low, mid and high in
/// seconds, mean free path, enclosure, first reflection distance,
/// scattering), then the 14 EAX reverb parameters for it (density,
/// diffusion, gain, gain HF, gain LF, decay time, decay HF ratio, decay LF
/// ratio, reflections gain, reflections delay, late reverb gain, late reverb
/// delay, echo time, echo depth).
pub const EARSHOT_ROOM_DOUBLES: usize = 21;

/// Opaque engine handle.
#[derive(Debug)]
pub struct EarshotEngine {
    engine: Engine,
}

impl EarshotEngine {
    fn world_mut(&self, change: impl FnOnce(&mut World)) {
        self.engine.world_mut(change);
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
    EARSHOT_ABI_VERSION
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
    if engine.is_null() {
        return;
    }
    // SAFETY: `engine` came from `Box::into_raw` in `earshot_create`, and the
    // caller never uses it again.
    let engine = unsafe { Box::from_raw(engine) };
    guard(|| {
        drop(engine);
        Ok(())
    });
}

/// Writes the material names in id order as a JSON array of strings, for
/// Java to check its own list against. `*needed` is the full length; if it
/// exceeds `cap`, nothing is written and the caller retries.
///
/// # Safety
/// `buf` must be writable for `cap` bytes; `needed` must be writable.
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
/// 4096 material ids in Minecraft's order, index `(y << 8) | (z << 4) | x`.
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
            if let Some(&id) = blocks.iter().find(|&&id| Material::from_id(id).id() != id) {
                return Err(FfiError::invalid(format!("unknown material {id}")));
            }
            e.world_mut(|world| world.set_section(SectionPos::new(x, y, z), &blocks));
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
            e.world_mut(|world| world.remove_section(SectionPos::new(x, y, z)));
            Ok(())
        })
    }
}

/// Sets block `x, y, z` to `material`, as when the game changes it.
///
/// # Safety
/// `engine` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_set_block(
    engine: *mut EarshotEngine,
    x: i32,
    y: i32,
    z: i32,
    material: u8,
) -> i32 {
    // SAFETY: the caller guarantees `engine` is live.
    unsafe {
        with_engine(engine, |e| {
            let known = Material::from_id(material);
            if known.id() != material {
                return Err(FfiError::invalid(format!("unknown material {material}")));
            }
            e.world_mut(|world| world.set_block(x, y, z, known));
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
            // doubles (`3 * count` fits, being less than `len`).
            let xyz = slice_arg(sources, 3 * count, "sources")?;
            // SAFETY: the caller guarantees `out` is writable for `len` doubles.
            let out = slice_mut_arg(out, len, "out")?;
            if !xyz.iter().all(|v| v.is_finite()) {
                return Err(FfiError::invalid("sources are not finite"));
            }
            let points: Vec<Vec3> = xyz
                .as_chunks::<3>()
                .0
                .iter()
                .map(|&[x, y, z]| Vec3::new(x, y, z))
                .collect();
            let mut heard = vec![
                Heard {
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
                *out = [low, mid, high, at.x, at.y, at.z, heard.path_length];
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
            let room = e.engine.room().unwrap_or(Room::OPEN_AIR);
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
            for (out, value) in out[7..].iter_mut().zip(eax_reverb(&room).to_array()) {
                *out = f64::from(value);
            }
            Ok(())
        })
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
/// must be writable.
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
/// writable; `stats` must be writable for 4 doubles.
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
            if !(reach.is_finite() && reach > 0.0) || rays > 1 << 20 || bounces > 64 {
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

/// Copies this thread's last error message into `buf` (truncated to `cap`,
/// at a character boundary) and returns its full length, or 0 if there is
/// none.
///
/// # Safety
/// `buf` must be writable for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn earshot_last_error(buf: *mut u8, cap: usize) -> usize {
    let message = guard::last_error();
    // SAFETY: the caller guarantees `buf` is writable for `cap` bytes.
    if let Ok(buf) = unsafe { slice_mut_arg(buf, cap, "buf") } {
        let n = message.floor_char_boundary(buf.len());
        buf[..n].copy_from_slice(&message.as_bytes()[..n]);
    }
    message.len()
}

/// A finite point from 3 doubles.
///
/// # Safety
/// `ptr` must be null or point to 3 doubles.
unsafe fn point_arg(ptr: *const f64, what: &str) -> Result<Vec3, FfiError> {
    // SAFETY: forwarded from the caller.
    let [x, y, z] = unsafe { array_arg::<f64, 3>(ptr, what) }?;
    if !(x.is_finite() && y.is_finite() && z.is_finite()) {
        return Err(FfiError::invalid(format!("{what} is not finite")));
    }
    Ok(Vec3::new(x, y, z))
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
            for after in source.split("extern \"C\" fn ").skip(1) {
                exported.insert(identifier(after));
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
                earshot_set_block(engine, 0, 0, 0, 99),
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
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
    fn a_null_engine_is_refused() {
        let mut count = 0;
        // SAFETY: the null engine is refused before anything else is used.
        let code = unsafe { earshot_section_count(std::ptr::null_mut(), &mut count) };
        assert_eq!(code, EARSHOT_ERR_INVALID_ARGUMENT);
    }
}
