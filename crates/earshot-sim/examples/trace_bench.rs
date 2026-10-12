//! How fast the path tracer runs (M1): `cargo run --release -p earshot-sim
//! --example trace_bench`. Prints a Markdown table, and appends it to
//! `$GITHUB_STEP_SUMMARY` when CI sets it.

use std::time::Instant;

use earshot_sim::{FIELD_HALF, Field, trace_paths};
use earshot_world::{Material, Vec3, World};

const RAYS: u32 = 4096;
const BOUNCES: u32 = 16;
const REACH: f64 = 64.0;
const RUNS: usize = 5;

fn main() {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut table = format!(
        "### Path tracer\n\n{RAYS} rays × up to {BOUNCES} bounces, reach {REACH} blocks, \
         median of {RUNS} runs, {} {}.\n\n\
         | World | Legs | Escaped first | Mean free path | 1 thread | Legs/s, 1 thread | \
         {threads} threads |\n| --- | --- | --- | --- | --- | --- | --- |\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    for (name, world, origin) in [hall(), caves(), open_ground()] {
        let stats = trace_paths(&world, origin, RAYS, BOUNCES, REACH, 1, |_| {});
        let single = median(|| {
            trace_paths(&world, origin, RAYS, BOUNCES, REACH, 1, |_| {});
        });
        let parallel = median(|| {
            std::thread::scope(|scope| {
                for seed in 0..threads {
                    let world = &world;
                    let rays = RAYS / threads as u32;
                    scope.spawn(move || {
                        trace_paths(world, origin, rays, BOUNCES, REACH, seed as u64, |_| {})
                    });
                }
            });
        });
        table += &format!(
            "| {name} | {} | {:.0} % | {:.1} blocks | {:.1} ms | {:.2} M | {:.1} ms |\n",
            stats.segments,
            100.0 * f64::from(stats.escaped_first) / f64::from(RAYS),
            stats.mean_free_path,
            single * 1e3,
            f64::from(stats.segments) / single / 1e6,
            parallel * 1e3,
        );
    }
    let side = 2 * FIELD_HALF + 1;
    table += &format!(
        "\n### Pathing field\n\n{side}³ blocks around the listener, median of {RUNS} builds.\n\n\
         | World | Build |\n| --- | --- |\n"
    );
    for (name, world, origin) in [hall(), caves(), open_ground()] {
        let build = median(|| {
            Field::build(&world, origin, FIELD_HALF);
        });
        table += &format!("| {name} | {:.1} ms |\n", build * 1e3);
    }
    print!("{table}");
    if let Ok(summary) = std::env::var("GITHUB_STEP_SUMMARY") {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(summary)
            .expect("open the step summary");
        writeln!(file, "{table}").expect("write the step summary");
    }
}

/// The median wall time of `RUNS` runs of `run`, in seconds.
fn median(mut run: impl FnMut()) -> f64 {
    let mut times: Vec<f64> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed().as_secs_f64()
        })
        .collect();
    times.sort_by(f64::total_cmp);
    times[RUNS / 2]
}

/// A stone hall, 40 × 20 × 40 inside.
fn hall() -> (&'static str, World, Vec3) {
    let mut world = World::new();
    let (w, h) = (40, 20);
    for a in -1..=w {
        for b in -1..=w {
            world.set_block(a, -1, b, Material::Rock);
            world.set_block(a, h, b, Material::Rock);
        }
        for y in -1..=h {
            for wall in [-1, w] {
                world.set_block(wall, y, a, Material::Rock);
                world.set_block(a, y, wall, Material::Rock);
            }
        }
    }
    ("stone hall", world, Vec3::new(20.5, 1.6, 20.5))
}

/// 160 × 80 × 160 of rock with caves carved by 3D value noise, the listener
/// in the open cell nearest the middle.
fn caves() -> (&'static str, World, Vec3) {
    let mut world = World::new();
    let (w, h) = (160, 80);
    for x in 0..w {
        for y in 0..h {
            for z in 0..w {
                if noise(x, y, z) < 0.55 {
                    world.set_block(x, y, z, Material::Rock);
                }
            }
        }
    }
    let middle = (w / 2, h / 2, w / 2);
    let mut origin = Vec3::new(0.5, 0.5, 0.5);
    'search: for r in 0..w / 2 {
        for dx in -r..=r {
            for dy in -r.min(h / 2 - 1)..=r.min(h / 2 - 1) {
                for dz in -r..=r {
                    let (x, y, z) = (middle.0 + dx, middle.1 + dy, middle.2 + dz);
                    if world.block(x, y, z) == Material::Air {
                        origin =
                            Vec3::new(f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5);
                        break 'search;
                    }
                }
            }
        }
    }
    ("caves", world, origin)
}

/// Flat ground 200 × 200, the listener standing on it.
fn open_ground() -> (&'static str, World, Vec3) {
    let mut world = World::new();
    for x in -100..100 {
        for z in -100..100 {
            world.set_block(x, -1, z, Material::Soil);
        }
    }
    ("open ground", world, Vec3::new(0.5, 1.6, 0.5))
}

/// Smooth noise in 0..1: two octaves of trilinear value noise.
fn noise(x: i32, y: i32, z: i32) -> f64 {
    let octave = |scale: f64| {
        let (fx, fy, fz) = (
            f64::from(x) / scale,
            f64::from(y) / scale,
            f64::from(z) / scale,
        );
        let (x0, y0, z0) = (fx.floor(), fy.floor(), fz.floor());
        let (tx, ty, tz) = (fx - x0, fy - y0, fz - z0);
        let corner = |dx: f64, dy: f64, dz: f64| lattice(x0 + dx, y0 + dy, z0 + dz);
        let lerp = |a: f64, b: f64, t: f64| a + (b - a) * (t * t * (3.0 - 2.0 * t));
        let x00 = lerp(corner(0.0, 0.0, 0.0), corner(1.0, 0.0, 0.0), tx);
        let x10 = lerp(corner(0.0, 1.0, 0.0), corner(1.0, 1.0, 0.0), tx);
        let x01 = lerp(corner(0.0, 0.0, 1.0), corner(1.0, 0.0, 1.0), tx);
        let x11 = lerp(corner(0.0, 1.0, 1.0), corner(1.0, 1.0, 1.0), tx);
        lerp(lerp(x00, x10, ty), lerp(x01, x11, ty), tz)
    };
    octave(16.0) * 0.7 + octave(6.0) * 0.3
}

/// A pseudo-random value in 0..1 for a lattice point.
fn lattice(x: f64, y: f64, z: f64) -> f64 {
    let mut h = (x as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (y as i64 as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
        ^ (z as i64 as u64).wrapping_mul(0x1656_67b1_9e37_79f9);
    h ^= h >> 31;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 29;
    (h >> 11) as f64 / (1u64 << 53) as f64
}
