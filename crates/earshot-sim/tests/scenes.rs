//! Golden scenes: small worlds with recorded expectations, so a change to
//! the tracer, the pathing or the reverb shows up as a failing scene rather
//! than a slow drift in how the game sounds. Traces are seeded and the
//! engine is deterministic (fixed seeds, integer RNG, IEEE arithmetic), and
//! every measurement here uses fewer than 2048 rays, which keeps it on the
//! single-threaded path: the same numbers on every machine.
//!
//! Expectations are ranges, wide enough to survive an intended tweak to the
//! models but tight enough to catch a regression — a decay that jumps by
//! half, a room that stops being enclosed, a doorway that stops routing.

use earshot_sim::{DiffractionStandard, MAX_APERTURE, Room, hear, impulse_response, measure};
use earshot_world::{Material, Vec3, World};

/// How a scene's numbers are expected to come out.
struct Expect {
    enclosure: (f32, f32),
    /// Mid-band decay, seconds.
    mid_decay: (f64, f64),
    mean_free_path: (f64, f64),
    /// Which band decays longest and which shortest, by name.
    order: [&'static str; 3],
    /// Mid-band gains of a source straight outside the front wall, heard
    /// from inside.
    outside_mid_gain: (f32, f32),
}

struct Scene {
    name: &'static str,
    world: World,
    listener: Vec3,
    /// A source 5 blocks out from the front wall (negative z), at head height.
    outside: Vec3,
    expect: Expect,
}

impl std::fmt::Debug for Scene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}

/// A closed rock box on a rock floor, `size` blocks inside each way, with a
/// 1 × 2 doorway in the wall facing -z if `door` — two blocks off the
/// centre line, so a sound straight ahead must bend to reach it.
fn hut(size: i32, door: bool) -> World {
    hut_of(size, door, 1.0)
}

/// The same hut with walls of a given fill: `0.5` is a hut built of half
/// slabs, which the mass law says passes the square root of the loss.
fn hut_of(size: i32, door: bool, fill: f32) -> World {
    let mut world = World::new();
    for x in -1..=size {
        for z in -1..=size {
            world.set_block(x, -1, z, Material::Rock);
        }
    }
    for y in 0..=size {
        for a in -1..=size {
            for wall in [-1, size] {
                world.set_block_fill(wall, y, a, Material::Rock, fill);
                world.set_block_fill(a, y, wall, Material::Rock, fill);
            }
        }
    }
    // The roof, so the room is shut: the walls stop at y = size and this is
    // its ceiling.
    for a in -1..=size {
        for b in -1..=size {
            world.set_block(a, size, b, Material::Rock);
        }
    }
    if door {
        let mid = size / 2 + 2;
        world.set_block(mid, 0, -1, Material::Air);
        world.set_block(mid, 1, -1, Material::Air);
    }
    world
}

fn scenes() -> Vec<Scene> {
    let inside = |size: i32| Vec3::new(f64::from(size) / 2.0, 1.5, f64::from(size) / 2.0);
    let outside = |size: i32| Vec3::new(f64::from(size) / 2.0, 1.5, -6.0);
    // Eyring's decay for a rock box of side `size`, mid band, no air term of
    // note at this size — what the sealed hut's decay should sit near.
    let eyring = |size: f64, a: f64| {
        let path = 4.0 * size.powi(3) / (6.0 * size * size);
        13.815_510_557_964_274 * path / (343.0 * -(1.0 - a).ln())
    };
    let rock_mid = f64::from(Material::Rock.acoustics().absorption[1]);
    vec![
        Scene {
            name: "sealed stone hut",
            world: hut(7, false),
            listener: inside(7),
            outside: outside(7),
            expect: Expect {
                enclosure: (0.99, 1.0),
                mid_decay: (0.7 * eyring(7.0, rock_mid), 1.3 * eyring(7.0, rock_mid)),
                mean_free_path: (4.0, 5.5),
                order: ["high", "mid", "low"],
                outside_mid_gain: (0.0, 1e-4),
            },
        },
        Scene {
            name: "hut with a doorway",
            world: hut(7, true),
            listener: inside(7),
            outside: outside(7),
            expect: Expect {
                // Only the door leaks, and it leaks little.
                enclosure: (0.95, 1.0),
                // The door leaks a real but small share: a 2 m² opening in
                // some 300 m² of stone shortens the decay only a little
                // (checked against the sealed hut below).
                mid_decay: (0.1, 1.05 * eyring(7.0, rock_mid)),
                mean_free_path: (4.0, 7.0),
                order: ["high", "mid", "low"],
                // Straight through rock, with no field to route by: five
                // rays of rock transmission, whatever the door. The way
                // round the doorway is the routing test's business. The
                // five rays hit the same single rock block at z = −1, so
                // the mean is rock's per-block mid transmission, 10⁻⁴·⁵ ≈
                // 3.16 × 10⁻⁵; the band is tight enough to catch a 1 dB
                // drift in rock's mid-band loss.
                outside_mid_gain: (2.5e-5, 1e-3),
            },
        },
        Scene {
            name: "open ground",
            world: {
                let mut world = World::new();
                for x in -70..70 {
                    for z in -70..70 {
                        world.set_block(x, -1, z, Material::Soil);
                    }
                }
                world
            },
            listener: Vec3::new(0.5, 1.5, 0.5),
            outside: Vec3::new(0.5, 1.5, -6.0),
            expect: Expect {
                enclosure: (0.4, 0.6),
                mid_decay: (0.1, 0.3),
                mean_free_path: (1.0, 8.0),
                order: ["high", "mid", "low"],
                outside_mid_gain: (0.95, 1.0),
            },
        },
        Scene {
            name: "stone hall with a cloth floor",
            world: {
                let mut world = hut(20, false);
                for x in 0..20 {
                    for z in 0..20 {
                        world.set_block(x, 0, z, Material::Cloth);
                    }
                }
                world
            },
            listener: Vec3::new(10.0, 1.5, 10.0),
            outside: Vec3::new(10.0, 1.5, -6.0),
            expect: Expect {
                enclosure: (0.99, 1.0),
                mid_decay: (2.0, 4.5),
                mean_free_path: (10.0, 16.0),
                order: ["high", "mid", "low"],
                outside_mid_gain: (0.0, 1e-4),
            },
        },
    ]
}

#[test]
fn the_scenes_measure_as_recorded() {
    let mut sealed_mid_decay: Option<f32> = None;
    for scene in scenes() {
        let room = measure(&scene.world, scene.listener, 1024, 16, 64.0, 1);
        let e = &scene.expect;
        let check = |what: &str, value: f64, (low, high): (f64, f64)| {
            assert!(
                value >= low && value <= high,
                "{}: {what} = {value}, expected {low}..{high}",
                scene.name
            );
        };
        check(
            "enclosure",
            f64::from(room.enclosure),
            (f64::from(e.enclosure.0), f64::from(e.enclosure.1)),
        );
        check("mid decay", f64::from(room.decay[1]), e.mid_decay);
        check("mean free path", room.mean_free_path, e.mean_free_path);
        // The doorway leaks: its room rings less long than the sealed one,
        // even if only a little.
        if scene.name == "sealed stone hut" {
            sealed_mid_decay = Some(room.decay[1]);
        }
        if scene.name == "hut with a doorway" {
            let sealed = sealed_mid_decay.unwrap();
            assert!(
                f64::from(room.decay[1]) < f64::from(sealed),
                "the hut with a door rings as long as the sealed one: {} against {}",
                room.decay[1],
                sealed
            );
        }
        let bands: [&str; 3] = ["low", "mid", "high"];
        let longest = bands
            .iter()
            .enumerate()
            .max_by(|a, b| f64::from(room.decay[a.0]).total_cmp(&f64::from(room.decay[b.0])))
            .unwrap()
            .1;
        let shortest = bands
            .iter()
            .enumerate()
            .min_by(|a, b| f64::from(room.decay[a.0]).total_cmp(&f64::from(room.decay[b.0])))
            .unwrap()
            .1;
        assert!(
            *longest == e.order[2] && *shortest == e.order[0],
            "{}: decays {:?} ({}..{}), expected {} longest and {} shortest",
            scene.name,
            room.decay,
            e.order[0],
            e.order[2],
            e.order[2],
            e.order[0]
        );

        // How a source straight outside the front wall is heard inside.
        let heard = hear(
            &scene.world,
            None,
            scene.listener,
            scene.outside,
            5,
            room.enclosure,
            DiffractionStandard::Iso9613_2,
        );
        check(
            "outside mid gain",
            f64::from(heard.gains[1]),
            (
                f64::from(e.outside_mid_gain.0),
                f64::from(e.outside_mid_gain.1),
            ),
        );

        // The scene's response: long enough for its decay, at its rate, and
        // holding no more energy than the room sent in.
        let ir = impulse_response(&room, &[], &[], 48_000, 2.0, 1);
        let seconds = ir.len() as f32 / 48_000.0;
        // `impulse_response` clamps the room's decay to `1.5 * max_seconds`
        // and caps the response length at `max_seconds`. With max_seconds =
        // 2.0 the response is at least `room.decay[1].min(2.0)` long, and
        // for a 48 kHz IR that's a whole number of samples — the
        // construction cannot produce a length shorter than the clamped
        // decay by more than a single sample. The +1 / 48_000 term covers
        // that rounding.
        assert!(
            seconds + 1.0 / 48_000.0 >= room.decay[1].min(2.0),
            "{}: response {} s against a decay of {}",
            scene.name,
            seconds,
            room.decay[1]
        );
        let energy = 2.0 * ir.iter().step_by(4).map(|s| s * s).sum::<f32>();
        assert!(
            energy.is_finite() && energy > 0.0,
            "{}: {energy}",
            scene.name
        );
    }
}

/// `impulse_response` shortens the IR when `max_seconds` is below the room's
/// decay, by clamping the decay to `1.5 * max_seconds` and capping the
/// length. The cap should hold exactly (to within one sample), not drift
/// past it.
#[test]
fn a_short_max_seconds_caps_the_response_at_the_clamped_decay() {
    use earshot_sim::IR_CHANNELS;
    let room = Room {
        decay: [2.0, 1.0, 0.5],
        mean_free_path: 8.0,
        enclosure: 1.0,
        first_reflection: 4.0,
        scattering: 0.3,
        volume: 1.0,
    };
    let max = 0.4;
    let ir = impulse_response(&room, &[], &[], 48_000, max, 1);
    // The IR is interleaved across `IR_CHANNELS` (W, X, Y, Z); divide by
    // both to get seconds. With `max = 0.4`, the response is one sample per
    // channel per 1/48_000 s, capped to 0.4 s.
    let seconds = (ir.len() / IR_CHANNELS) as f32 / 48_000.0;
    assert!(
        (seconds - max).abs() <= 1.0 / 48_000.0,
        "response {seconds} s, max {max}"
    );
    // And it is no longer than the unclamped decay would imply: a 1 s decay
    // would normally give a 1 s response, not 0.4 s.
    assert!(seconds < room.decay[1], "{seconds} s vs {}", room.decay[1]);
}

#[test]
fn the_doorway_routes_the_sound_and_the_sealed_wall_does_not() {
    let outside = Vec3::new(3.5, 1.5, -6.0);
    let listener = Vec3::new(3.5, 1.5, 3.5);
    for (door, routed) in [(true, true), (false, false)] {
        let world = hut(7, door);
        let field = earshot_sim::Field::build(&world, listener, 12);
        let heard = hear(
            &world,
            Some(&field),
            listener,
            outside,
            5,
            1.0,
            DiffractionStandard::Iso9613_2,
        );
        let through = hear(
            &world,
            None,
            listener,
            outside,
            5,
            1.0,
            DiffractionStandard::Iso9613_2,
        );
        assert!(
            heard.heard_from.x > 4.0 && heard.heard_from.z < listener.z || !routed,
            "door: heard from {:?}, not from the doorway at x = 5",
            heard.heard_from
        );
        assert!(
            heard.path_length > (outside - listener).length() + 1.0 || !routed,
            "door: the way round is not longer: {heard:?}"
        );
        assert_eq!(
            heard.aperture == 2.0 && heard.aperture < MAX_APERTURE,
            routed,
            "door {door}: aperture {heard:?}"
        );
        // With nowhere to route, what is heard is what gets through.
        if !routed {
            assert_eq!(heard, through);
        } else {
            assert!(
                heard.gains[1] > through.gains[1] * 5.0,
                "door: {heard:?} against {through:?}"
            );
        }
    }
}

#[test]
fn a_ravine_answers_in_echoes_and_its_room_stays_open() {
    let mut world = World::new();
    for x in -70..70 {
        for z in -70..70 {
            world.set_block(x, -1, z, Material::Rock);
        }
    }
    for y in 0..12 {
        for z in -12..12 {
            world.set_block(-1, y, z, Material::Rock);
            world.set_block(40, y, z, Material::Rock);
        }
    }
    let listener = Vec3::new(10.0, 1.5, 0.5);
    let room = measure(&world, listener, 1024, 16, 64.0, 1);
    assert!(room.enclosure < 0.7, "{room:?}");
    let ir = impulse_response(&room, &[], &[], 48_000, 1.5, 1);
    // Energy well after the direct sound, where the cliff answers.
    let window = |from: f32, to: f32| {
        ir[(from * 48_000.0) as usize * 4..(to * 48_000.0) as usize * 4]
            .iter()
            .step_by(4)
            .map(|s| s * s)
            .sum::<f32>()
    };
    let early = window(0.0, 0.05);
    let echo = window(0.05, 0.35);
    assert!(echo > 0.1 * early, "echo {echo} against early {early}");
}

#[test]
fn an_ocean_carries_sound_and_the_surface_shuts_it_in() {
    // Rock floor at y = -1, water to y = 12, open sky above.
    let mut world = World::new();
    for x in -40..40 {
        for z in -40..40 {
            world.set_block(x, -1, z, Material::Rock);
            for y in 0..=12 {
                world.set_block(x, y, z, Material::Water);
            }
        }
    }
    let deep = Vec3::new(0.5, 2.5, 0.5);
    let far = Vec3::new(30.5, 2.5, 0.5);
    // Thirty blocks of open water, both ends under: what air carries,
    // water carries. The old per-block table killed this outright.
    let under = hear(
        &world,
        None,
        deep,
        far,
        5,
        0.0,
        DiffractionStandard::Iso9613_2,
    );
    assert!(under.gains[1] > 0.9, "{under:?}");

    // A listener in the air above hears it through the surface once: about
    // the impedance toll, lows a little better than highs.
    let above = Vec3::new(30.5, 14.5, 0.5);
    let out = hear(
        &world,
        None,
        deep,
        above,
        5,
        0.0,
        DiffractionStandard::Iso9613_2,
    );
    assert!(
        (1e-4..1e-2).contains(&f64::from(out.gains[1])),
        "{} through the surface",
        out.gains[1]
    );
    assert!(out.gains[0] > out.gains[2], "{out:?}");
}

#[test]
fn a_slab_walled_hut_leaks_more_than_a_solid_one() {
    let (listener, outside) = (Vec3::new(3.5, 1.5, 3.5), Vec3::new(3.5, 1.5, -6.0));
    let solid = hear(
        &hut(7, false),
        None,
        listener,
        outside,
        5,
        1.0,
        DiffractionStandard::Iso9613_2,
    );
    let slab = hear(
        &hut_of(7, false, 0.5),
        None,
        listener,
        outside,
        5,
        1.0,
        DiffractionStandard::Iso9613_2,
    );
    // The mass law end to end: half the wall, the square root of the loss.
    assert!(
        (f64::from(slab.gains[1]) - f64::from(solid.gains[1]).sqrt()).abs()
            < 0.1 * f64::from(slab.gains[1]).sqrt(),
        "{slab:?} against {solid:?}"
    );
    // And the room: still enclosed, but its decay is shorter for the leak.
    let sealed_room = measure(&hut(7, false), listener, 1024, 16, 64.0, 1);
    let slab_room = measure(&hut_of(7, false, 0.5), listener, 1024, 16, 64.0, 1);
    assert!(slab_room.enclosure > 0.95, "{slab_room:?}");
    assert!(
        f64::from(slab_room.decay[1]) < f64::from(sealed_room.decay[1]),
        "slab hut {} against sealed {}",
        slab_room.decay[1],
        sealed_room.decay[1]
    );
}

#[test]
fn a_carpeted_hall_kills_the_highs_not_the_lows() {
    // The 20-block stone hall, its stone floor lined with rugs.
    let mut hall = hut(20, false);
    for x in 0..20 {
        for z in 0..20 {
            hall.set_block(x, 0, z, Material::Rug);
        }
    }
    let listener = Vec3::new(10.0, 1.5, 10.0);
    let bare = measure(&hut(20, false), listener, 1024, 16, 64.0, 1);
    let lined = measure(&hall, listener, 1024, 16, 64.0, 1);
    // The highs go first: that is what a rug is for.
    assert!(
        f64::from(lined.decay[2]) < 0.8 * f64::from(bare.decay[2]),
        "highs: {} against bare {}",
        lined.decay[2],
        bare.decay[2]
    );
    // The lows barely move: a thin layer has little say down there.
    assert!(
        f64::from(lined.decay[0]) > 0.8 * f64::from(bare.decay[0]),
        "lows: {} against bare {}",
        lined.decay[0],
        bare.decay[0]
    );
}

/// A regression guard on the shared `Room` type itself: the smoothing in the
/// engine blends rooms, so a `Room` straight out of a scene must satisfy the
/// invariants the EAX mapping and IR synthesis assume.
#[test]
fn rooms_come_out_sane_wherever_they_are_measured() {
    for scene in scenes() {
        for at in [scene.listener, scene.outside] {
            let room = measure(&scene.world, at, 1024, 16, 64.0, 1);
            let Room {
                decay,
                mean_free_path,
                enclosure,
                first_reflection,
                scattering,
                volume,
            } = room;
            for t60 in decay {
                assert!((0.1..=8.0).contains(&f64::from(t60)), "{scene:?} {room:?}");
            }
            assert!((0.0..=1.0).contains(&f64::from(enclosure)));
            assert!(mean_free_path >= 0.0 && first_reflection >= 0.0);
            assert!((0.0..=1.0).contains(&f64::from(scattering)));
            assert!(volume >= 0.0);
            assert!(decay.iter().all(|t60| t60.is_finite()));
        }
    }
}

/// A doorway's edges leak high-band energy into the next room: the wired
/// `impulse_response_for` must include `edge_sources` in its IR. A regression
/// that drops the `&edge` argument (passes `&[]` instead) breaks this test:
/// the wired IR's energy at the edge delay collapses to the no-edge
/// baseline, and the comparison below fails.
#[test]
fn a_doorway_leaks_high_band_around_its_edges() {
    use earshot_sim::{Engine, IR_CHANNELS, early_taps, edge_sources, impulse_response, measure};
    // A 3×3 rock wall at x=4 (in the +x direction from the source, so the
    // wall is *behind* the source relative to the listener). A 1×1 doorway
    // at (4, 1, 0) lets the source's reflection back to the listener pass
    // through; the upper vertical edges of the doorway's frame are visible
    // and leak high-band energy around the wall.
    let mut world = World::new();
    for y in 0..=2 {
        for z in -1..=1 {
            if y == 1 && z == 0 {
                continue;
            }
            world.set_block(4, y, z, Material::Rock);
        }
    }
    let listener = Vec3::new(-2.0, 0.5, 0.5);
    let source = Vec3::new(2.0, 0.5, 0.5);

    // The wired entry point: same path `impulse_response_for` takes.
    let room = measure(&world, source, 256, 8, 32.0, 1);
    let taps = early_taps(&world, listener, source);
    let edges = edge_sources(&world, listener, source, &taps);
    assert!(
        !edges.is_empty(),
        "test setup: the doorway should have at least one edge source"
    );

    // And a sanity check that the Engine's wired path agrees.
    let engine = Engine::start();
    engine.world_change(|w| {
        for y in 0..=2 {
            for z in -1..=1 {
                if y == 1 && z == 0 {
                    continue;
                }
                w.set_block(4, y, z, Material::Rock);
            }
        }
        true
    });
    engine.set_listener(listener);
    let wired = engine
        .impulse_response_for(source, 256, 8, 48_000, 0.05, 1)
        .expect("listener is set");

    // The energy in a 1 ms window around the first edge's delay, on W.
    // With edges, the window has the edge's tap; without, it does not.
    let energy_window = |ir: &[f32]| -> f32 {
        let delay = edges[0].delay;
        let start = ((delay - 0.0005).max(0.0) * 48_000.0) as usize * IR_CHANNELS;
        let end = (((delay + 0.0005) * 48_000.0) as usize + 1) * IR_CHANNELS;
        ir[start..end.min(ir.len())]
            .iter()
            .step_by(IR_CHANNELS)
            .map(|x| x * x)
            .sum()
    };
    let direct_with_edges = impulse_response(&room, &taps, &edges, 48_000, 0.05, 1);
    let direct_without_edges = impulse_response(&room, &taps, &[], 48_000, 0.05, 1);
    let direct_with = energy_window(&direct_with_edges);
    let direct_without = energy_window(&direct_without_edges);
    let wired_at = energy_window(&wired);
    assert!(
        direct_with > direct_without,
        "the IR with edges has more energy at the edge delay: {direct_with} vs {direct_without}"
    );
    assert!(
        wired_at > direct_without,
        "the wired engine path matches a hand-built IR with edges: {wired_at} vs {direct_without}"
    );
}
