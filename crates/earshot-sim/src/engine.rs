use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use earshot_world::{Vec3, World};

use crate::{
    Field, Heard, Quality, Reflection, Room, Stats, early_taps, echoes, edge_sources, hear,
    impulse_response, measure, measure_threads, measure_with_early,
};

/// The field is rebuilt once the listener moves this far from where it was
/// built.
const REBUILD_DISTANCE: f64 = 0.5;
/// Or, after blocks change, once it is this old.
const REBUILD_AFTER_CHANGES: Duration = Duration::from_millis(200);
/// After a build, the next waits at least this many times as long as the
/// build took, so pathing takes at most about a quarter of a core even while
/// the listener runs through big fields; but never longer than
/// [`MAX_REBUILD_WAIT`], and not at all when the field's reach changes.
const REBUILD_SPACING: u32 = 3;
const MAX_REBUILD_WAIT: Duration = Duration::from_millis(250);
/// How often the worker looks.
const WORKER_TICK: Duration = Duration::from_millis(25);
/// The room follows its measurements with this time constant: about half a
/// second to cross a doorway, and no flicker from ray noise.
const ROOM_TIME_CONSTANT: f32 = 0.46;
/// The budget scales the room's rays down to this share at the least, and
/// never below this many.
const MIN_RAY_SCALE: f32 = 1.0 / 16.0;
const MIN_ROOM_RAYS: u32 = 64;
/// Once the listener has stayed within this many blocks, with the world
/// unchanged, for [`SETTLED`] measurements in a row, the room has converged
/// on what's there: it isn't traced again until something moves or changes.
const STILL: f64 = 0.1;
const SETTLED: u32 = 16;

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
    /// Changes made while the world was being read, applied once it isn't.
    queued: Mutex<Vec<Change>>,
    /// Bumped on every change to the world.
    world_version: AtomicU64,
    listener: Mutex<Option<Vec3>>,
    quality: RwLock<Quality>,
    /// The air's temperature, sampled from the listener's biome.
    air: RwLock<Air>,
    stats: Mutex<Stats>,
    field: RwLock<Option<Arc<Built>>>,
    room: RwLock<Option<Room>>,
    /// The early reflections from the latest measurement.
    early: RwLock<Vec<Reflection>>,
    stop: AtomicBool,
}

/// The air's temperature in °C, clamped to the band real weather stays in
/// (−60 to 60). The default 20 °C gives `speed_of_sound(20.0)` = 343.42 m/s,
/// the fixed `SPEED_OF_SOUND` within a tenth of a percent, so the timing
/// paths do not move until a climate actually differs.
#[derive(Debug, Clone, Copy)]
struct Air(f64);

impl Default for Air {
    fn default() -> Air {
        Air(20.0)
    }
}

/// A change to the world, waiting for the lock.
struct Change(Box<dyn FnOnce(&mut World) -> bool + Send>);

impl std::fmt::Debug for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Change")
    }
}

/// Applies the queued changes to `world`, in order; whether any changed it.
fn apply_queued(shared: &Shared, world: &mut World) -> bool {
    let queued = std::mem::take(&mut *shared.queued.lock().unwrap_or_else(PoisonError::into_inner));
    let mut changed = false;
    for change in queued {
        changed |= (change.0)(world);
    }
    changed
}

/// Applies any queued changes. The worker `wait`s for the lock; a reader
/// (the sound thread) only takes it if it's free, else reads the world as it
/// was a moment ago rather than wait out a measurement.
fn catch_up(shared: &Shared, wait: bool) {
    if shared
        .queued
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_empty()
    {
        return;
    }
    let mut world = if wait {
        shared.world.write().unwrap_or_else(PoisonError::into_inner)
    } else {
        match shared.world.try_write() {
            Ok(world) => world,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return,
        }
    };
    if apply_queued(shared, &mut world) {
        shared.world_version.fetch_add(1, Ordering::Release);
    }
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

    /// Changes the world if `change` says it did: the field and the room are
    /// only measured again for a real change.
    pub fn world_change(&self, change: impl FnOnce(&mut World) -> bool) -> bool {
        let changed = change(
            &mut self
                .shared
                .world
                .write()
                .unwrap_or_else(PoisonError::into_inner),
        );
        if changed {
            self.shared.world_version.fetch_add(1, Ordering::Release);
        }
        changed
    }

    /// Changes the world without waiting: now if no one is reading it,
    /// otherwise queued, for the next reader or the worker to apply. So the
    /// game's thread never waits on a trace or a measurement.
    pub fn world_change_soon(&self, change: impl FnOnce(&mut World) -> bool + Send + 'static) {
        match self.shared.world.try_write() {
            Ok(mut world) => {
                let mut changed = apply_queued(&self.shared, &mut world);
                changed |= change(&mut world);
                if changed {
                    self.shared.world_version.fetch_add(1, Ordering::Release);
                }
            }
            Err(_) => self
                .shared
                .queued
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Change(Box::new(change))),
        }
    }

    /// Reads the world, with every change made so far.
    pub fn world<T>(&self, read: impl FnOnce(&World) -> T) -> T {
        catch_up(&self.shared, true);
        self.world_as_is(read)
    }

    /// Reads the world without waiting: changes queued while the worker is
    /// reading it land a moment later. For the sound thread's every tick.
    fn world_now<T>(&self, read: impl FnOnce(&World) -> T) -> T {
        catch_up(&self.shared, false);
        self.world_as_is(read)
    }

    fn world_as_is<T>(&self, read: impl FnOnce(&World) -> T) -> T {
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

    /// How warm the air is, in °C, for the timing paths (see
    /// [`crate::speed_of_sound`]), sampled from the listener's biome. A
    /// reading outside the band real weather stays in (−60 to 60) is
    /// clamped to it, and a non-finite one refused outright, so the engine
    /// can never hold a temperature the speed maths would turn silly; the
    /// last real reading stands instead.
    pub fn set_air(&self, celsius: f64) {
        if !celsius.is_finite() {
            return;
        }
        *self
            .shared
            .air
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Air(celsius.clamp(-60.0, 60.0));
    }

    /// The air's temperature in °C: 20 by default (see [`Self::set_air`]).
    pub fn air_celsius(&self) -> f64 {
        self.shared
            .air
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .0
    }

    /// What the engine is spending.
    pub fn stats(&self) -> Stats {
        *self
            .shared
            .stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Where the listener is now; the pathing field follows. A listener the
    /// sim can't carry (not finite, or out of [`Vec3::is_usable`] range) is
    /// refused rather than overflowing the field's block arithmetic on the
    /// worker.
    pub fn set_listener(&self, listener: Vec3) {
        let listener = listener.is_usable().then_some(listener);
        *self
            .shared
            .listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = listener;
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
            &[],
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
        let reach = self.quality().room_reach;
        self.world(|world| measure(world, open_near(world, at), rays, bounces, reach, seed))
    }

    /// An impulse response for a sound at `source` as the listener hears it
    /// (see [`impulse_response`]): the room measured round the sound with
    /// `rays` × `bounces`, and its first reflections off the walls round it
    /// as they reach the listener, timed after its direct sound. For the
    /// loudest sounds' own reverb, in place of the listener's. None before
    /// the listener is known.
    #[allow(clippy::too_many_arguments)]
    pub fn impulse_response_for(
        &self,
        source: Vec3,
        rays: u32,
        bounces: u32,
        sample_rate: u32,
        max_seconds: f32,
        seed: u64,
    ) -> Option<Vec<f32>> {
        let listener = (*self
            .shared
            .listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner))?;
        let room = self.room_at(source, rays, bounces, seed);
        let (taps, edge) = self.world(|world| {
            let taps = early_taps(world, listener, source);
            let edge = edge_sources(world, listener, source, &taps);
            (taps, edge)
        });
        Some(impulse_response(
            &room,
            &taps,
            &edge,
            sample_rate,
            max_seconds,
            seed,
        ))
    }

    /// The current pathing field, if one has been built.
    pub fn field(&self) -> Option<Field> {
        self.built().map(|built| built.field.clone())
    }

    /// How each of `sources` reaches `listener`, into `out` (one per source).
    pub fn hear(&self, listener: Vec3, sources: &[Vec3], out: &mut [Heard]) {
        let built = self.built();
        let field = built.as_ref().map(|built| &built.field);
        let quality = self.quality();
        let rays = quality.occlusion_rays;
        // How sound gets round corners depends on how shut in the listener is.
        let enclosure = self.room().map_or(0.0, |room| room.enclosure);
        let diffraction = quality.diffraction;
        self.world_now(|world| {
            for (source, heard) in sources.iter().zip(out) {
                *heard = hear(
                    world,
                    field,
                    listener,
                    *source,
                    rays,
                    enclosure,
                    diffraction,
                );
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
        catch_up(shared, true);
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
            } else if let Some(resized) = field_due(shared, listener, quality)
                && (resized || Instant::now() >= next_build)
            {
                let took = build_field(shared, listener, quality.field_half);
                next_build = Instant::now() + (took * REBUILD_SPACING).min(MAX_REBUILD_WAIT);
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

/// Whether the field needs building: `Some(true)` when it is missing or the
/// wrong size, `Some(false)` when it is left behind by the listener or older
/// than a change to the world, else `None`.
fn field_due(shared: &Shared, listener: Vec3, quality: Quality) -> Option<bool> {
    let version = shared.world_version.load(Ordering::Acquire);
    let current = shared
        .field
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let Some(built) = current else {
        return Some(true);
    };
    if built.field.half() != quality.field_half {
        return Some(true);
    }
    let stale = (built.field.listener() - listener).length() >= REBUILD_DISTANCE
        || (built.world_version != version && built.at.elapsed() >= REBUILD_AFTER_CHANGES);
    stale.then_some(false)
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
    /// Where the last measurement was made, and of which world.
    last: Option<(Vec3, u64)>,
    /// Measurements in a row made there, of that world.
    still: u32,
}

impl Default for RoomTrace {
    fn default() -> Self {
        RoomTrace {
            at: None,
            seed: 0,
            scale: 1.0,
            cost: None,
            last: None,
            still: 0,
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
        let version = shared.world_version.load(Ordering::Acquire);
        let same = self
            .last
            .is_some_and(|(at, world)| world == version && (at - listener).length() < STILL);
        self.still = if same { self.still + 1 } else { 0 };
        if !same {
            self.last = Some((listener, version));
        }
        if self.still >= SETTLED {
            // Settled: nothing new to hear, and nothing spent.
            self.at = Some(start);
            shared
                .stats
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .room_cost = 0.0;
            return;
        }
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
            let (measured, mut early) = measure_with_early(
                &world,
                listener,
                rays,
                quality.room_bounces,
                quality.room_reach,
                self.seed,
            );
            // Echoes off walls farther than the trace reaches, and back and
            // forth between facing ones.
            early.extend(echoes(&world, listener, quality.room_reach));
            (measured, early)
        };
        // Spread over threads, it takes their time together.
        let share = start.elapsed().as_secs_f32() * measure_threads(rays) as f32
            / quality.room_every.as_secs_f32();
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
        self.last = None;
        self.still = 0;
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
        early_decay: [0, 1, 2].map(|band| geo(from.early_decay[band], to.early_decay[band])),
        mean_free_path: lerp64(from.mean_free_path, to.mean_free_path),
        enclosure: lerp(from.enclosure, to.enclosure),
        first_reflection: lerp64(from.first_reflection, to.first_reflection),
        scattering: lerp(from.scattering, to.scattering),
        volume: lerp64(from.volume, to.volume),
    }
}

#[cfg(test)]
mod tests {
    use earshot_world::{BANDS, Material};

    use super::*;

    #[test]
    fn the_field_follows_the_listener_and_the_world() {
        let engine = Engine::start();
        assert!(engine.field().is_none());
        engine.set_listener(Vec3::new(0.5, 0.5, 0.5));
        let deadline = Instant::now() + Duration::from_secs(20);
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
        let deadline = Instant::now() + Duration::from_secs(20);
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
        let deadline = Instant::now() + Duration::from_secs(20);
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
    fn a_sound_gets_a_response_of_its_own_room() {
        let engine = Engine::start();
        // A closed stone cell round the sound, the listener outside it.
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
        });
        assert!(
            engine
                .impulse_response_for(Vec3::new(3.0, 3.0, 3.0), 512, 8, 48_000, 1.5, 1)
                .is_none(),
            "no listener yet"
        );
        engine.set_listener(Vec3::new(20.0, 3.0, 3.0));
        let ir = engine
            .impulse_response_for(Vec3::new(3.0, 3.0, 3.0), 512, 8, 48_000, 1.5, 1)
            .unwrap();
        // The cell rings: its response runs for its decay, not open air's tenth of a second.
        assert!(ir.len() / 4 > 48_000 / 5, "{} samples", ir.len() / 4);
    }

    #[test]
    fn hearing_uses_the_field() {
        let engine = Engine::start();
        let listener = Vec3::new(0.5, 0.5, 0.5);
        let mut out = [Heard {
            aperture: 0.0,
            gains: [0.0; 3],
            heard_from: Vec3::ZERO,
            path_length: 0.0,
        }];
        engine.hear(listener, &[Vec3::new(5.5, 0.5, 0.5)], &mut out);
        assert_eq!(out[0].gains, [1.0; 3]);
    }

    #[test]
    fn a_change_made_while_the_world_is_read_lands_before_the_next_read() {
        let engine = Engine::start();
        engine.set_listener(Vec3::new(0.5, 0.5, 0.5));
        // Keep the worker reading and building so `world_change_soon` finds
        // the world busy and queues.
        engine.set_quality(Quality {
            field_half: 24,
            room_rays: 4096,
            ..Quality::DEFAULT
        });
        for i in 0..200 {
            engine.world_change_soon(move |world| world.set_block(i, 0, 100, Material::Rock));
        }
        // Every queued block is there by the time a waiting read runs; the
        // calls themselves must never block on a trace.
        engine.world(|world| {
            for i in 0..200 {
                assert_eq!(world.block(i, 0, 100), Material::Rock, "block {i}");
            }
        });
    }

    #[test]
    fn rooms_blend_across_time_constant_and_decays_geometrically() {
        let (from, to) = (
            Room {
                decay: [1.0, 1.0, 1.0],
                early_decay: [4.0, 4.0, 4.0],
                mean_free_path: 4.0,
                enclosure: 0.0,
                first_reflection: 2.0,
                scattering: 0.0,
                volume: 100.0,
            },
            Room {
                decay: [2.0, 4.0, 8.0],
                early_decay: [16.0, 16.0, 16.0],
                mean_free_path: 12.0,
                enclosure: 1.0,
                first_reflection: 6.0,
                scattering: 1.0,
                volume: 300.0,
            },
        );
        let whole = blend(&from, &to, 1.0);
        assert_eq!(whole, to);
        let none = blend(&from, &to, 0.0);
        assert_eq!(none, from);
        let half = blend(&from, &to, 0.5);
        // Straight fields take half the difference.
        assert!((half.enclosure - 0.5).abs() < 1e-6);
        assert!((half.mean_free_path - 8.0).abs() < 1e-9);
        // Decays take half of it in log space: 1 → 2 halves to √2, 1 → 4 to
        // 2, 1 → 8 to √8.
        assert!(
            (half.decay[0] - std::f32::consts::SQRT_2).abs() < 1e-6,
            "{half:?}"
        );
        assert!((half.decay[1] - 2.0).abs() < 1e-6, "{half:?}");
        assert!((half.decay[2] - 8.0f32.sqrt()).abs() < 1e-6, "{half:?}");
        // The early decay blends geometrically too, off its own values:
        // 4 → 16 halves to 8, which no blend of the decays above would give.
        for band in 0..BANDS {
            assert!((half.early_decay[band] - 8.0).abs() < 1e-6, "{half:?}");
        }
    }

    #[test]
    fn an_absurd_listener_is_refused_and_the_worker_survives() {
        let engine = Engine::start();
        engine.set_listener(Vec3::new(f64::NAN, 0.0, 0.0));
        engine.set_listener(Vec3::new(-9e18, 0.5, 0.5));
        engine.set_listener(Vec3::new(0.5, 0.5, 0.5));
        let deadline = Instant::now() + Duration::from_secs(20);
        while engine.field().is_none() {
            assert!(
                Instant::now() < deadline,
                "no field: the worker never recovered"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_air_starts_at_room_temperature_and_clamps_to_weather() {
        let engine = Engine::start();
        assert_eq!(engine.air_celsius(), 20.0, "comfortable room air");
        engine.set_air(40.0);
        assert_eq!(engine.air_celsius(), 40.0);
        engine.set_air(1e9);
        assert_eq!(engine.air_celsius(), 60.0, "the warm end of the band");
        engine.set_air(-1e9);
        assert_eq!(engine.air_celsius(), -60.0, "the cold end of the band");
        engine.set_air(f64::NAN);
        assert_eq!(
            engine.air_celsius(),
            -60.0,
            "a non-finite reading is no climate: the last real one stands"
        );
    }
}
