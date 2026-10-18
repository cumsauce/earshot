//! Probe (investigation, 2026-10-18): how loud is the convolution wet path
//! against the EAX wet path for the *same* cave-like room, in the units the
//! game mixes them in?
//!
//! The game's listener IR is `Engine::impulse_response` = `measure`'s room and
//! first reflections through `impulse_response` (48 kHz, 1.5 s max) — the same
//! call this probe makes — and mixes it at `ConvolutionReverb.LEVEL = 0.5`
//! (`mod/src/main/java/io/github/cumsauce/earshot/audio/ConvolutionReverb.java`).
//! The EAX path for the same room is `eax_reverb`'s 14 EFX parameters played
//! through an EFX EAX reverb at slot gain 1. The EAX reverb body lives inside
//! OpenAL Soft and cannot be rendered here, so its wet is modelled the way the
//! EFX parameters read: a diffuse tail of `decay_time` seconds whose onset
//! power is `(gain · late_reverb_gain)²` of the send. That model is stated in
//! the output so the comparison's assumption is visible.
//!
//! Run with `--nocapture`; buffers land in `/tmp/*.f32` for the Python side
//! (`/tmp/reverb_probe.py`): total energy, energy-over-time (RT60) and band
//! energies.

use earshot_sim::{SPEED_OF_SOUND, eax_reverb, impulse_response, late_energy, measure_with_early};
use earshot_world::{Material, Vec3, World};

/// The convolution slot's gain (ConvolutionReverb.LEVEL).
const LEVEL: f32 = 1.0;
/// What the game uploads (ConvolutionReverb.MAX_SECONDS).
const MAX_SECONDS: f32 = 3.0;
const RATE: u32 = 48_000;

/// A closed rock chamber, `size` blocks square and `height` high inside — a
/// large cave room: fully enclosed, mean free path of several blocks, a
/// volume of thousands of cubic metres.
fn cavern(size: i32, height: i32) -> World {
    let mut world = World::new();
    for x in -1..=size {
        for z in -1..=size {
            for y in -1..=height {
                let shell = x == -1 || x == size || z == -1 || z == size || y == -1 || y == height;
                if shell {
                    world.set_block(x, y, z, Material::Rock);
                }
            }
        }
    }
    world
}

/// Open ground: a rock floor and the sky (rays escape).
fn open_ground() -> World {
    let mut world = World::new();
    for x in -64..=64 {
        for z in -64..=64 {
            world.set_block(x, -1, z, Material::Rock);
        }
    }
    world
}

/// The wet of a unit-energy diffuse input through an EAX reverb set from these
/// parameters: a decaying noise tail, `decay_time` to −60 dB, onset power
/// `(gain · late_reverb_gain)²` (EFX: output = gain × (reflections + late),
/// late scaled by late_reverb_gain). Deterministic noise.
fn eax_reference_wet(gain: f32, late_gain: f32, decay_time: f32, seed: u32) -> Vec<f32> {
    let seconds = decay_time.min(8.0);
    let n = (seconds * RATE as f32) as usize;
    let onset = gain * late_gain;
    let mut state = seed | 1;
    let mut noise = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state as f32 / (1u32 << 31) as f32) - 1.0
    };
    (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            onset * noise() * (-6.907_755 * t / decay_time.max(0.05)).exp()
        })
        .collect()
}

/// An 8-sample click (the dry "drip" stand-in): a half-sine of unit energy.
fn click() -> Vec<f32> {
    let n = 8;
    let mut out: Vec<f32> = (0..n)
        .map(|i| (std::f32::consts::PI * i as f32 / n as f32).sin())
        .collect();
    let energy: f32 = out.iter().map(|x| x * x).sum();
    let scale = 1.0 / energy.sqrt();
    out.iter_mut().for_each(|x| *x *= scale);
    out
}

/// Every dry tap against the response's W channel (FuMa W carries half the
/// field's energy — `ir.rs` counts `2·ΣW²` as the response's energy — so the
/// W path is the energy-representative one to compare levels on).
fn convolve_w_channel(dry: &[f32], ir: &[f32]) -> Vec<f32> {
    let frames = ir.len() / 4;
    let mut out = vec![0.0f32; frames];
    for (k, d) in dry.iter().enumerate() {
        for i in k..frames {
            out[i] += d * ir[(i - k) * 4];
        }
    }
    out
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|s| f64::from(*s) * f64::from(*s)).sum()
}

/// Writes one mono f32 stream per name, for the Python analysis.
fn dump(name: &str, x: &[f32]) {
    let mut bytes = Vec::with_capacity(x.len() * 4);
    for s in x {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(format!("/tmp/{name}.f32"), bytes).expect("write /tmp");
}

fn report(label: &str, world: &World, listener: Vec3, seed: u64) {
    let (room, early) = measure_with_early(world, listener, 1024, 8, 64.0, seed);
    let late = late_energy(&room);
    let ir = impulse_response(
        &room,
        &early,
        &[],
        RATE,
        MAX_SECONDS,
        SPEED_OF_SOUND,
        seed ^ 0x9E37,
    );
    let eax = eax_reverb(&room, SPEED_OF_SOUND);
    let ir_energy: f64 = ir
        .chunks_exact(4)
        .map(|f| 2.0 * f64::from(f[0]) * f64::from(f[0]))
        .sum();
    let capped = ir_energy > 1.0 + 1e-6;

    let dry = click();
    let wet_w = convolve_w_channel(&dry, &ir);
    let mixed: Vec<f32> = wet_w.iter().map(|s| s * LEVEL).collect();
    let eax_wet = eax_reference_wet(eax.gain, eax.late_reverb_gain, eax.decay_time, 0xC0FFEE);

    println!("=== {label} ===");
    println!(
        "room: decay [{:.2} {:.2} {:.2}] s, early [{:.2} {:.2} {:.2}] s, mfp {:.2}, enclosure {:.2}, first {:.2}, scatter {:.2}, volume {:.0}",
        room.decay[0],
        room.decay[1],
        room.decay[2],
        room.early_decay[0],
        room.early_decay[1],
        room.early_decay[2],
        room.mean_free_path,
        room.enclosure,
        room.first_reflection,
        room.scattering,
        room.volume
    );
    println!("late_energy = {late:.4}  (reverberant energy vs direct at one block)");
    println!(
        "ir: {} frames ({:.2} s), raw energy (2·ΣW²) = {ir_energy:.4}{}",
        ir.len() / 4,
        ir.len() as f64 / 4.0 / f64::from(RATE),
        if capped { "  <- CAPPED at 1.0" } else { "" }
    );
    println!(
        "eax: gain {:.3}, late_reverb_gain {:.3}, decay_time {:.2} s, reflections_gain {:.3}, density {:.3}, diffusion {:.3}",
        eax.gain,
        eax.late_reverb_gain,
        eax.decay_time,
        eax.reflections_gain,
        eax.density,
        eax.diffusion
    );
    println!(
        "energy: dry {:.5} | wet_conv(as mixed, LEVEL {LEVEL}) {:.5} | wet_conv(raw ir) {:.5} | wet_eax(model) {:.5}",
        energy(&dry),
        energy(&mixed),
        energy(&wet_w),
        energy(&eax_wet)
    );
    println!(
        "wet/dry dB: conv {:.1} | conv without LEVEL {:.1} | eax(model) {:.1}",
        10.0 * (energy(&mixed) / energy(&dry)).log10(),
        10.0 * (energy(&wet_w) / energy(&dry)).log10(),
        10.0 * (energy(&eax_wet) / energy(&dry)).log10()
    );

    dump(&format!("{label}_ir"), &ir);
    dump(&format!("{label}_dry"), &dry);
    dump(&format!("{label}_wet_conv"), &mixed);
    dump(&format!("{label}_wet_conv_raw"), &wet_w);
    dump(&format!("{label}_wet_eax"), &eax_wet);
}

#[test]
fn cave_wet_levels() {
    // A big closed chamber: the cave case the report is about.
    let cave = cavern(24, 10);
    report("cave", &cave, Vec3::new(12.0, 3.0, 12.0), 7);
    // Open ground, for contrast: the same machinery where the user does not
    // hear a difference.
    let open = open_ground();
    report("open", &open, Vec3::new(0.0, 1.7, 0.0), 7);
}
