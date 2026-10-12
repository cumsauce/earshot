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

use std::sync::{PoisonError, RwLock};

use earshot_sim::trace_paths;
use earshot_world::{BANDS, Material, SECTION_BLOCKS, SectionPos, Vec3, World};

use crate::guard::{FfiError, array_arg, guard, out_arg, slice_mut_arg};

/// Version of this C ABI. Java refuses to use a library whose version differs
/// from the one it was built against. Bump on any incompatible change.
/// 2: the engine, its world mirror, transmission and path traces.
pub const EARSHOT_ABI_VERSION: u32 = 2;

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

/// Opaque engine handle.
#[derive(Debug, Default)]
pub struct EarshotEngine {
    world: RwLock<World>,
}

impl EarshotEngine {
    fn world_mut(&self, change: impl FnOnce(&mut World)) {
        change(&mut self.world.write().unwrap_or_else(PoisonError::into_inner));
    }

    fn world<T>(&self, read: impl FnOnce(&World) -> T) -> T {
        read(&self.world.read().unwrap_or_else(PoisonError::into_inner))
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

/// Starts an engine with an empty world.
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
        *out = Box::into_raw(Box::default());
        Ok(())
    })
}

/// Frees the engine. Must be the last call on `engine`. Null is ignored.
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
