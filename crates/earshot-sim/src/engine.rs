use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use earshot_world::{Vec3, World};

use crate::{
    Field, Heard, Quality, Reflection, Room, Stats, hear, impulse_response, measure,
    measure_with_early,
};

/// The field is rebuilt once the listener moves this far from where it was
/// built.
const REBUILD_DISTANCE: f64 = 0.5;
/// Or, after blocks change, once it is this old.
const REBUILD_AFTER_CHANGES: Duration = Duration::from_millis(200);
/// After a build, the next waits at least this many times as long as the
/// build took, so pathing takes at most about a quarter of a core even while
/// the listener runs through big fields.
const REBUILD_SPACING: u32 = 3;
/// How often the worker looks.
const WORKER_TICK: Duration = Duration::from_millis(25);
/// How far room rays reach, in blocks.
pub const ROOM_REACH: f64 = 64.0;
/// The room follows its measurements with this time constant: about half a
/// second to cross a doorway, and no flicker from ray noise.
const ROOM_TIME_CONSTANT: f32 = 0.46;
/// The budget scales the room's rays down to this share at the least, and
/// never below this many.
const MIN_RAY_SCALE: f32 = 1.0 / 16.0;
const MIN_ROOM_RAYS: u32 = 64;

/// The acoustics engine: the mirrored world, the listener, and a worker
/// thread that keeps the pathing field around the listener current. Every
/// method may be called from any thread.
#[derive(Debug)]
pub struct Engine {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Debug, Default)]
struct Shared {
    world: RwLock<World>,
    /// Bumped on every change to the world.
    world_version: AtomicU64,
    listener: Mutex<Option<Vec3>>,
    quality: RwLock<Quality>,
    stats: Mutex<Stats>,
    field: RwLock<Option<Arc<Built>>>,
    room: RwLock<Option<Room>>,
    /// The early reflections from the latest measurement.
    early: RwLock<Vec<Reflection>>,
    stop: AtomicBool,
}

/// A field and what it was built from.
#[derive(Debug)]
struct Built {
    field: Field,
    world_version: u64,
    at: Instant,
}

impl Engine {
    pub fn start() -> Engine {
        let shared = Arc::new(Shared::default());
        let worker = std::thread::Builder::new()
            .name("earshot-paths".into())
            .spawn({
                let shared = shared.clone();
                move || worker(&shared)
            })
            .expect("spawn the pathing thread");
        Engine {
            shared,
            worker: Some(worker),
        }
    }

    /// Changes the world.
    pub fn world_mut<T>(&self, change: impl FnOnce(&mut World) -> T) -> T {
        let result = change(
            &mut self
                .shared
                .world
                .write()
                .unwrap_or_else(PoisonError::into_inner),
        );
        self.shared.world_version.fetch_add(1, Ordering::Release);
        result
    }

    /// Reads the world.
    pub fn world<T>(&self, read: impl FnOnce(&World) -> T) -> T {
        read(
            &self
                .shared
                .world
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// How much to trace from now on. The worker picks it up within a tick.
    pub fn set_quality(&self, quality: Quality) {
        *self
            .shared
            .quality
            .write()
            .unwrap_or_else(PoisonError::into_inner) = quality;
    }

    pub fn quality(&self) -> Quality {
        *self
            .shared
            .quality
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// What the engine is spending.
    pub fn stats(&self) -> Stats {
        *self
            .shared
            .stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Where the listener is now; the pathing field follows.
    pub fn set_listener(&self, listener: Vec3) {
        *self
            .shared
            .listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(listener);
    }

    /// The room around the listener, smoothed, once it has been measured.
    pub fn room(&self) -> Option<Room> {
        *self
            .shared
            .room
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// An impulse response for the room around the listener (see
    /// [`impulse_response`]), or none before the room has been measured.
    pub fn impulse_response(
        &self,
        sample_rate: u32,
        max_seconds: f32,
        seed: u64,
    ) -> Option<Vec<f32>> {
        let room = self.room()?;
        let early = self
            .shared
            .early
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        Some(impulse_response(
            &room,
            &early,
            sample_rate,
            max_seconds,
            seed,
        ))
    }

    /// Measures the room around `at` now, on the calling thread, with a
    /// trace of `rays` × `bounces`: for the reverb of a space a sound is in
    /// when the listener is elsewhere. A point inside a solid block is
    /// measured from the open block next to it nearest the point, if any.
    pub fn room_at(&self, at: Vec3, rays: u32, bounces: u32, seed: u64) -> Room {
        self.world(|world| measure(world, open_near(world, at), rays, bounces, ROOM_REACH, seed))
    }

    /// The current pathing field, if one has been built.
    pub fn field(&self) -> Option<Field> {
        self.built().map(|built| built.field.clone())
    }

    /// How each of `sources` reaches `listener`, into `out` (one per source).
    pub fn hear(&self, listener: Vec3, sources: &[Vec3], out: &mut [Heard]) {
        let built = self.built();
        let field = built.as_ref().map(|built| &built.field);
        let rays = self.quality().occlusion_rays;
        self.world(|world| {
            for (source, heard) in sources.iter().zip(out) {
                *heard = hear(world, field, listener, *source, rays);
            }
        });
    }

    fn built(&self) -> Option<Arc<Built>> {
        self.shared
            .field
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Keeps the pathing field and the room around the listener current, as
/// much as the quality asks for and the budget allows.
fn worker(shared: &Shared) {
    let mut room = RoomTrace::default();
    let mut next_build = Instant::now();
    while !shared.stop.load(Ordering::Relaxed) {
        let quality = *shared
            .quality
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let listener = *shared
            .listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(listener) = listener {
            if quality.field_half == 0 {
                drop_field(shared);
            } else if Instant::now() >= next_build && field_due(shared, listener, quality) {
                let took = build_field(shared, listener, quality.field_half);
                next_build = Instant::now() + took * REBUILD_SPACING;
            }
            if quality.room_rays == 0 {
                room.forget(shared);
            } else if room.due(quality) {
                room.measure(shared, listener, quality);
            }
        }
        std::thread::sleep(WORKER_TICK);
    }
}

/// `at`, or if it is inside a solid block, the centre of the open block next
/// to it (across a face) nearest `at`, so a sound played by a block measures
/// the room it sounds into.
fn open_near(world: &World, at: Vec3) -> Vec3 {
    let block = [at.x, at.y, at.z].map(|v| v.floor() as i32);
    if !world.block(block[0], block[1], block[2]).is_solid() {
        return at;
    }
    let centre =
        |[x, y, z]: [i32; 3]| Vec3::new(f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5);
    [
        [1, 0, 0],
        [-1, 0, 0],
        [0, 1, 0],
        [0, -1, 0],
        [0, 0, 1],
        [0, 0, -1],
    ]
    .map(|[dx, dy, dz]| [block[0] + dx, block[1] + dy, block[2] + dz])
    .into_iter()
    .filter(|&[x, y, z]| !world.block(x, y, z).is_solid())
    .map(centre)
    .min_by(|a, b| (*a - at).length().total_cmp(&(*b - at).length()))
    .unwrap_or(at)
}

/// Whether the field is missing, the wrong size, left behind by the
/// listener, or older than a change to the world.
fn field_due(shared: &Shared, listener: Vec3, quality: Quality) -> bool {
    let version = shared.world_version.load(Ordering::Acquire);
    let current = shared
        .field
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    current.is_none_or(|built| {
        built.field.half() != quality.field_half
            || (built.field.listener() - listener).length() >= REBUILD_DISTANCE
            || (built.world_version != version && built.at.elapsed() >= REBUILD_AFTER_CHANGES)
    })
}

/// Builds the field around `listener` and returns how long it took.
fn build_field(shared: &Shared, listener: Vec3, half: i32) -> Duration {
    let start = Instant::now();
    let version = shared.world_version.load(Ordering::Acquire);
    let field = {
        let world = shared.world.read().unwrap_or_else(PoisonError::into_inner);
        Field::build(&world, listener, half)
    };
    let took = start.elapsed();
    *shared.field.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(Built {
        field,
        world_version: version,
        at: Instant::now(),
    }));
    let mut stats = shared.stats.lock().unwrap_or_else(PoisonError::into_inner);
    stats.field_build = took;
    stats.field_half = half;
    took
}

fn drop_field(shared: &Shared) {
    let mut field = shared.field.write().unwrap_or_else(PoisonError::into_inner);
    if field.take().is_some() {
        shared
            .stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .field_half = 0;
    }
}

/// The room measurement's schedule and budget.
#[derive(Debug)]
struct RoomTrace {
    at: Option<Instant>,
    seed: u64,
    /// The share of the quality's rays the budget allows.
    scale: f32,
    /// The measurement's share of one core, smoothed.
    cost: Option<f32>,
}

impl Default for RoomTrace {
    fn default() -> Self {
        RoomTrace {
            at: None,
            seed: 0,
            scale: 1.0,
            cost: None,
        }
    }
}

impl RoomTrace {
    fn due(&self, quality: Quality) -> bool {
        self.at.is_none_or(|at| at.elapsed() >= quality.room_every)
    }

    /// Measures the room around `listener` and moves the engine's room
    /// towards it, then fits the next measurement's rays to the budget.
    fn measure(&mut self, shared: &Shared, listener: Vec3, quality: Quality) {
        let start = Instant::now();
        let every = match self.at {
            Some(at) => start.duration_since(at).max(quality.room_every),
            None => quality.room_every,
        };
        self.at = Some(start);
        self.seed += 1;
        let rays = ((quality.room_rays as f32 * self.scale).round() as u32)
            .max(quality.room_rays.min(MIN_ROOM_RAYS));
        let (measured, early) = {
            let world = shared.world.read().unwrap_or_else(PoisonError::into_inner);
            measure_with_early(
                &world,
                listener,
                rays,
                quality.room_bounces,
                ROOM_REACH,
                self.seed,
            )
        };
        let share = start.elapsed().as_secs_f32() / quality.room_every.as_secs_f32();
        let cost = self.cost.map_or(share, |cost| cost + (share - cost) * 0.3);
        self.cost = Some(cost);
        self.scale = adjust_scale(self.scale, cost, quality.budget);

        *shared.early.write().unwrap_or_else(PoisonError::into_inner) = early;
        let t = 1.0 - (-every.as_secs_f32() / ROOM_TIME_CONSTANT).exp();
        let mut room = shared.room.write().unwrap_or_else(PoisonError::into_inner);
        *room = Some(match *room {
            Some(old) => blend(&old, &measured, t),
            None => measured,
        });
        let mut stats = shared.stats.lock().unwrap_or_else(PoisonError::into_inner);
        stats.room_rays = rays;
        stats.room_cost = cost;
    }

    /// No room is measured at this quality: there is none, and no reverb.
    fn forget(&mut self, shared: &Shared) {
        if self.at.take().is_none() {
            return;
        }
        *shared.room.write().unwrap_or_else(PoisonError::into_inner) = None;
        shared
            .early
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        let mut stats = shared.stats.lock().unwrap_or_else(PoisonError::into_inner);
        stats.room_rays = 0;
        stats.room_cost = 0.0;
    }
}

/// Fewer rays when the measurement costs more than the budget, more (back
/// up to all of them) when it costs well under.
fn adjust_scale(scale: f32, cost: f32, budget: f32) -> f32 {
    if cost > budget {
        (scale * 0.8).max(MIN_RAY_SCALE)
    } else if cost < 0.6 * budget {
        (scale * 1.1).min(1.0)
    } else {
        scale
    }
}

/// `t` of the way from `from` to `to`; decays blend geometrically, since the
/// ear hears their ratios.
fn blend(from: &Room, to: &Room, t: f32) -> Room {
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    let geo = |a: f32, b: f32| (a.ln() + (b.ln() - a.ln()) * t).exp();
    let lerp64 = |a: f64, b: f64| a + (b - a) * f64::from(t);
    Room {
        decay: [0, 1, 2].map(|band| geo(from.decay[band], to.decay[band])),
        mean_free_path: lerp64(from.mean_free_path, to.mean_free_path),
        enclosure: lerp(from.enclosure, to.enclosure),
        first_reflection: lerp64(from.first_reflection, to.first_reflection),
        scattering: lerp(from.scattering, to.scattering),
    }
}

#[cfg(test)]
mod tests {
    use earshot_world::Material;

    use super::*;

    #[test]
    fn the_field_follows_the_listener_and_the_world() {
        let engine = Engine::start();
        assert!(engine.field().is_none());
        engine.set_listener(Vec3::new(0.5, 0.5, 0.5));
        let deadline = Instant::now() + Duration::from_secs(5);
        while engine.field().is_none() {
            assert!(Instant::now() < deadline, "no field");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(engine.field().unwrap().distance([3, 0, 0]), Some(3.0));

        // Walling the listener in shows up once the field is rebuilt.
        engine.world_mut(|world| {
            for (x, y, z) in [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 1, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
            ] {
                world.set_block(x, y, z, Material::Rock);
            }
        });
        while engine.field().unwrap().distance([3, 0, 0]).is_some() {
            assert!(Instant::now() < deadline, "the field never saw the walls");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_room_is_measured_and_settles() {
        let engine = Engine::start();
        engine.world_mut(|world| {
            for a in -1..=8 {
                for b in -1..=8 {
                    for wall in [-1, 8] {
                        world.set_block(wall, a, b, Material::Rock);
                        world.set_block(a, wall, b, Material::Rock);
                        world.set_block(a, b, wall, Material::Rock);
                    }
                }
            }
        });
        engine.set_listener(Vec3::new(4.0, 4.0, 4.0));
        let deadline = Instant::now() + Duration::from_secs(5);
        while engine.room().is_none_or(|room| room.enclosure < 0.99) {
            assert!(Instant::now() < deadline, "no room: {:?}", engine.room());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(engine.room().unwrap().decay[1] > 1.0);
    }

    #[test]
    fn the_quality_turns_pathing_and_the_room_off_and_on() {
        let engine = Engine::start();
        engine.set_listener(Vec3::new(0.5, 0.5, 0.5));
        let deadline = Instant::now() + Duration::from_secs(5);
        while engine.field().is_none() || engine.room().is_none() {
            assert!(Instant::now() < deadline, "no field or room");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(engine.stats().field_half, Quality::DEFAULT.field_half);
        engine.set_quality(Quality {
            room_rays: 0,
            field_half: 0,
            ..Quality::DEFAULT
        });
        while engine.field().is_some() || engine.room().is_some() {
            assert!(
                Instant::now() < deadline,
                "still tracing: {:?}",
                engine.stats()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        engine.set_quality(Quality {
            field_half: 8,
            ..Quality::DEFAULT
        });
        while engine.field().is_none_or(|field| field.half() != 8) || engine.room().is_none() {
            assert!(Instant::now() < deadline, "not back: {:?}", engine.stats());
            std::thread::sleep(Duration::from_millis(5));
        }
        let stats = engine.stats();
        assert!(stats.room_rays > 0 && stats.room_cost > 0.0, "{stats:?}");
    }

    #[test]
    fn the_budget_trades_rays_for_time() {
        let mut scale = 1.0;
        for _ in 0..100 {
            scale = adjust_scale(scale, 0.5, 0.25);
        }
        assert_eq!(scale, MIN_RAY_SCALE);
        scale = adjust_scale(scale, 0.2, 0.25);
        assert_eq!(scale, MIN_RAY_SCALE, "inside the budget: no change");
        for _ in 0..100 {
            scale = adjust_scale(scale, 0.01, 0.25);
        }
        assert_eq!(scale, 1.0);
        assert!(Quality::DEFAULT.is_valid());
        assert!(
            !Quality {
                occlusion_rays: 3,
                ..Quality::DEFAULT
            }
            .is_valid()
        );
    }

    #[test]
    fn a_room_is_measured_where_a_sound_is_even_from_inside_a_block() {
        let engine = Engine::start();
        // A closed stone cell, 6 blocks inside, with a block set in its floor.
        engine.world_mut(|world| {
            for a in -1..=6 {
                for b in -1..=6 {
                    for wall in [-1, 6] {
                        world.set_block(wall, a, b, Material::Rock);
                        world.set_block(a, wall, b, Material::Rock);
                        world.set_block(a, b, wall, Material::Rock);
                    }
                }
            }
            world.set_block(2, 0, 2, Material::Wood);
        });
        let inside = engine.room_at(Vec3::new(3.0, 3.0, 3.0), 512, 8, 1);
        assert!(inside.enclosure > 0.99, "{inside:?}");
        let block = engine.room_at(Vec3::new(2.5, 0.5, 2.5), 512, 8, 1);
        assert!(
            block.enclosure > 0.99 && block.decay[1] > 0.3,
            "from the block's top: {block:?}"
        );
        let outside = engine.room_at(Vec3::new(30.0, 3.0, 3.0), 512, 8, 1);
        assert!(outside.enclosure < 0.5, "{outside:?}");
    }

    #[test]
    fn hearing_uses_the_field() {
        let engine = Engine::start();
        let listener = Vec3::new(0.5, 0.5, 0.5);
        let mut out = [Heard {
            gains: [0.0; 3],
            heard_from: Vec3::ZERO,
            path_length: 0.0,
        }];
        engine.hear(listener, &[Vec3::new(5.5, 0.5, 0.5)], &mut out);
        assert_eq!(out[0].gains, [1.0; 3]);
    }
}
