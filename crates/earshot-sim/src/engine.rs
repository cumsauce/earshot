use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use earshot_world::{Vec3, World};

use crate::{Field, Heard, Room, hear, measure};

/// How far the pathing field reaches each way from the listener, in blocks.
pub const FIELD_HALF: i32 = 24;
/// The field is rebuilt once the listener moves this far from where it was
/// built.
const REBUILD_DISTANCE: f64 = 0.5;
/// Or, after blocks change, once it is this old.
const REBUILD_AFTER_CHANGES: Duration = Duration::from_millis(200);
/// How often the worker looks.
const WORKER_TICK: Duration = Duration::from_millis(25);
/// How often the room around the listener is measured, and with what trace.
const ROOM_EVERY: Duration = Duration::from_millis(200);
pub const ROOM_RAYS: u32 = 1024;
pub const ROOM_BOUNCES: u32 = 12;
pub const ROOM_REACH: f64 = 64.0;
/// Each measurement moves the room this much of the way to what it measured:
/// about half a second to cross a doorway, and no flicker from ray noise.
const ROOM_SMOOTHING: f32 = 0.35;

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
    field: RwLock<Option<Arc<Built>>>,
    room: RwLock<Option<Room>>,
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

    /// The current pathing field, if one has been built.
    pub fn field(&self) -> Option<Field> {
        self.built().map(|built| built.field.clone())
    }

    /// How each of `sources` reaches `listener`, into `out` (one per source).
    pub fn hear(&self, listener: Vec3, sources: &[Vec3], out: &mut [Heard]) {
        let built = self.built();
        let field = built.as_ref().map(|built| &built.field);
        self.world(|world| {
            for (source, heard) in sources.iter().zip(out) {
                *heard = hear(world, field, listener, *source);
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

/// Rebuilds the field when the listener has moved, or the world has changed
/// and the field is old enough.
fn worker(shared: &Shared) {
    let mut room_at: Option<Instant> = None;
    let mut seed = 0u64;
    while !shared.stop.load(Ordering::Relaxed) {
        let listener = *shared
            .listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(listener) = listener {
            let version = shared.world_version.load(Ordering::Acquire);
            let current = shared
                .field
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let due = current.is_none_or(|built| {
                (built.field.listener() - listener).length() >= REBUILD_DISTANCE
                    || (built.world_version != version
                        && built.at.elapsed() >= REBUILD_AFTER_CHANGES)
            });
            if due {
                let field = {
                    let world = shared.world.read().unwrap_or_else(PoisonError::into_inner);
                    Field::build(&world, listener, FIELD_HALF)
                };
                *shared.field.write().unwrap_or_else(PoisonError::into_inner) =
                    Some(Arc::new(Built {
                        field,
                        world_version: version,
                        at: Instant::now(),
                    }));
            }
        }
        if let Some(listener) = listener
            && room_at.is_none_or(|at| at.elapsed() >= ROOM_EVERY)
        {
            room_at = Some(Instant::now());
            seed += 1;
            let measured = {
                let world = shared.world.read().unwrap_or_else(PoisonError::into_inner);
                measure(&world, listener, ROOM_RAYS, ROOM_BOUNCES, ROOM_REACH, seed)
            };
            let mut room = shared.room.write().unwrap_or_else(PoisonError::into_inner);
            *room = Some(match *room {
                Some(old) => blend(&old, &measured, ROOM_SMOOTHING),
                None => measured,
            });
        }
        std::thread::sleep(WORKER_TICK);
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
