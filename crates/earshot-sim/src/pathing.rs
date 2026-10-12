use earshot_world::{Vec3, World, cast};

/// Path lengths are kept in tenths of a block.
const PER_BLOCK: f64 = 10.0;
/// What a step to a neighbour costs: through a face, an edge or a corner
/// (1, √2 and √3 blocks).
const FACE: u16 = 10;
const EDGE: u16 = 14;
const CORNER: u16 = 17;
const UNREACHED: u16 = u16::MAX;

/// Shortest distances through open air from the listener to every block of
/// a cube around it: Dijkstra over the 26 neighbours, never cutting a corner
/// past a solid block. Sound behind a wall gets to the listener the way this
/// says, round through the nearest opening.
#[derive(Debug, Clone)]
pub struct Field {
    listener: Vec3,
    /// The cube's lowest block.
    min: [i32; 3],
    size: i32,
    /// Distance from the listener's block in tenths of a block, or
    /// [`UNREACHED`].
    dist: Vec<u16>,
}

/// How a sound gets to the listener through the air.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Route {
    /// How far it travels, in blocks.
    pub length: f64,
    /// Where it seems to come from: in the direction of the last opening on
    /// its way that the listener can see, as far away as the way is long.
    pub heard_from: Vec3,
    /// How far that direction turns from the straight line to the source, in
    /// radians: 0 when the way is straight.
    pub bend: f64,
}

impl Field {
    /// The field for a listener at `listener`, `half` blocks each way.
    pub fn build(world: &World, listener: Vec3, half: i32) -> Field {
        let size = 2 * half + 1;
        let centre = block_of(listener);
        let min = centre.map(|c| c - half);
        let cells = (size * size * size) as usize;
        let mut open = vec![false; cells];
        for x in 0..size {
            for y in 0..size {
                for z in 0..size {
                    let block = world.block(min[0] + x, min[1] + y, min[2] + z);
                    open[((x * size + y) * size + z) as usize] = !block.is_solid();
                }
            }
        }
        let mut field = Field {
            listener,
            min,
            size,
            dist: vec![UNREACHED; cells],
        };
        field.flood(&open, [half; 3]);
        field
    }

    /// Dial's algorithm: Dijkstra with a ring of buckets, one per distance
    /// modulo the largest step.
    fn flood(&mut self, open: &[bool], start: [i32; 3]) {
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); usize::from(CORNER) + 1];
        let start_index = self.index(start);
        self.dist[start_index] = 0;
        buckets[0].push(start_index as u32);
        let mut queued = 1usize;
        let mut d = 0u32;
        while queued > 0 {
            let bucket = (d % u32::from(CORNER + 1)) as usize;
            while let Some(cell) = buckets[bucket].pop() {
                queued -= 1;
                let cell = cell as usize;
                if u32::from(self.dist[cell]) != d {
                    continue;
                }
                let at = self.coords(cell);
                for (step, cost) in NEIGHBOURS {
                    let next = [at[0] + step[0], at[1] + step[1], at[2] + step[2]];
                    if !self.contains(next) || !self.passable(open, at, step) {
                        continue;
                    }
                    let next_index = self.index(next);
                    let nd = d + u32::from(cost);
                    if nd < u32::from(self.dist[next_index]) && nd < u32::from(UNREACHED) {
                        self.dist[next_index] = nd as u16;
                        buckets[(nd % u32::from(CORNER + 1)) as usize].push(next_index as u32);
                        queued += 1;
                    }
                }
            }
            d += 1;
        }
    }

    /// Whether sound can step from `at` by `step`: into an open block, and
    /// through open blocks on the way when it goes diagonally.
    fn passable(&self, open: &[bool], at: [i32; 3], step: [i32; 3]) -> bool {
        let to = [at[0] + step[0], at[1] + step[1], at[2] + step[2]];
        if !open[self.index(to)] {
            return false;
        }
        for axis in 0..3 {
            if step[axis] != 0 && step.iter().filter(|&&s| s != 0).count() > 1 {
                let mut side = at;
                side[axis] += step[axis];
                if !open[self.index(side)] {
                    return false;
                }
            }
        }
        true
    }

    pub fn listener(&self) -> Vec3 {
        self.listener
    }

    /// How far sound travels through the air from the listener to `block`,
    /// in blocks, if it gets there at all.
    pub fn distance(&self, block: [i32; 3]) -> Option<f64> {
        let local = self.local(block)?;
        let dist = self.dist[self.index(local)];
        (dist != UNREACHED).then(|| f64::from(dist) / PER_BLOCK)
    }

    /// The way a sound at `source` gets to the listener, if the field
    /// reaches it. A source inside a solid block (a chest, a note block) is
    /// heard from its nearest open neighbour.
    pub fn route(&self, world: &World, source: Vec3) -> Option<Route> {
        let mut cell = self.local(block_of(source))?;
        if self.dist[self.index(cell)] == UNREACHED {
            cell = FACES
                .iter()
                .map(|step| [cell[0] + step[0], cell[1] + step[1], cell[2] + step[2]])
                .filter(|&next| self.contains(next))
                .min_by_key(|&next| self.dist[self.index(next)])?;
            if self.dist[self.index(cell)] == UNREACHED {
                return None;
            }
        }
        let length = f64::from(self.dist[self.index(cell)]) / PER_BLOCK
            + (source - self.centre(cell)).length();
        // Walk back towards the listener until the listener can see the way.
        let mut opening = self.centre(cell);
        while !self.visible(world, opening) {
            let next = NEIGHBOURS
                .iter()
                .map(|(step, _)| [cell[0] + step[0], cell[1] + step[1], cell[2] + step[2]])
                .filter(|&next| self.contains(next))
                .min_by_key(|&next| self.dist[self.index(next)])?;
            if self.dist[self.index(next)] >= self.dist[self.index(cell)] {
                break;
            }
            cell = next;
            opening = self.centre(cell);
        }
        let towards = (opening - self.listener).normalized();
        let straight = (source - self.listener).normalized();
        if towards == Vec3::ZERO || straight == Vec3::ZERO {
            return Some(Route {
                length,
                heard_from: source,
                bend: 0.0,
            });
        }
        Some(Route {
            length,
            heard_from: self.listener + towards * length,
            bend: towards.dot(straight).clamp(-1.0, 1.0).acos(),
        })
    }

    /// Whether nothing solid lies between the listener and `point`.
    fn visible(&self, world: &World, point: Vec3) -> bool {
        let delta = point - self.listener;
        let distance = delta.length();
        distance < 1e-9 || cast(world, self.listener, delta * (1.0 / distance), distance).is_none()
    }

    fn local(&self, block: [i32; 3]) -> Option<[i32; 3]> {
        let local = [
            block[0] - self.min[0],
            block[1] - self.min[1],
            block[2] - self.min[2],
        ];
        self.contains(local).then_some(local)
    }

    fn contains(&self, local: [i32; 3]) -> bool {
        local.iter().all(|&c| (0..self.size).contains(&c))
    }

    fn index(&self, [x, y, z]: [i32; 3]) -> usize {
        ((x * self.size + y) * self.size + z) as usize
    }

    fn coords(&self, index: usize) -> [i32; 3] {
        let index = index as i32;
        [
            index / (self.size * self.size),
            index / self.size % self.size,
            index % self.size,
        ]
    }

    /// The middle of local block `local`, in world coordinates.
    fn centre(&self, local: [i32; 3]) -> Vec3 {
        Vec3::new(
            f64::from(self.min[0] + local[0]) + 0.5,
            f64::from(self.min[1] + local[1]) + 0.5,
            f64::from(self.min[2] + local[2]) + 0.5,
        )
    }
}

fn block_of(point: Vec3) -> [i32; 3] {
    [
        point.x.floor() as i32,
        point.y.floor() as i32,
        point.z.floor() as i32,
    ]
}

const FACES: [[i32; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

/// The 26 neighbours and what a step to each costs.
const NEIGHBOURS: [([i32; 3], u16); 26] = {
    let mut out = [([0; 3], 0); 26];
    let mut i = 0;
    let mut x = -1;
    while x <= 1 {
        let mut y = -1;
        while y <= 1 {
            let mut z = -1;
            while z <= 1 {
                let axes = (x != 0) as u8 + (y != 0) as u8 + (z != 0) as u8;
                if axes > 0 {
                    let cost = match axes {
                        1 => FACE,
                        2 => EDGE,
                        _ => CORNER,
                    };
                    out[i] = ([x, y, z], cost);
                    i += 1;
                }
                z += 1;
            }
            y += 1;
        }
        x += 1;
    }
    out
};

#[cfg(test)]
mod tests {
    use earshot_world::Material;

    use super::*;

    /// Rock ground at y = -1, and a rock wall on it at x = 5 from z = -10
    /// to 10 and y = 0 to 9 (too high to go over), with a doorway two high at
    /// z = `door` if any.
    fn wall(door: Option<i32>) -> World {
        let mut world = World::new();
        for x in -16..=16 {
            for z in -16..=16 {
                world.set_block(x, -1, z, Material::Rock);
            }
        }
        for y in 0..=9 {
            for z in -10..=10 {
                if door == Some(z) && y < 2 {
                    continue;
                }
                world.set_block(5, y, z, Material::Rock);
            }
        }
        world
    }

    const LISTENER: Vec3 = Vec3::new(0.5, 0.5, 0.5);
    const SOURCE: Vec3 = Vec3::new(9.5, 0.5, 0.5);

    #[test]
    fn open_air_is_a_straight_line() {
        let world = World::new();
        let field = Field::build(&world, LISTENER, 16);
        let route = field.route(&world, SOURCE).unwrap();
        assert!((route.length - 9.0).abs() < 0.6, "{route:?}");
        assert!(route.bend < 1e-9);
        assert!((route.heard_from - SOURCE).length() < 0.6);
        assert_eq!(field.distance([0, 0, 0]), Some(0.0));
    }

    #[test]
    fn sound_comes_round_through_the_doorway() {
        let world = wall(Some(6));
        let field = Field::build(&world, LISTENER, 16);
        let route = field.route(&world, SOURCE).unwrap();
        // Out to the doorway at z = 6 and back: longer than straight.
        assert!(route.length > 12.0 && route.length < 18.0, "{route:?}");
        // It is heard from the doorway's side (+z), not straight ahead.
        assert!(route.heard_from.z > 3.0, "{route:?}");
        assert!(route.bend > 0.4, "{route:?}");
    }

    #[test]
    fn a_sealed_wall_cuts_the_way_round() {
        let mut world = wall(None);
        // Close the ends and the top so there is no way round.
        for y in -1..=5 {
            for x in -10..=20 {
                for z in [-11, 11] {
                    world.set_block(x, y, z, Material::Rock);
                }
            }
            for z in -11..=11 {
                world.set_block(-10, y, z, Material::Rock);
                world.set_block(20, y, z, Material::Rock);
            }
        }
        for x in -10..=20 {
            for z in -11..=11 {
                world.set_block(x, 5, z, Material::Rock);
                world.set_block(x, -1, z, Material::Rock);
            }
        }
        let field = Field::build(&world, LISTENER, 16);
        assert_eq!(field.route(&world, SOURCE), None);
    }

    #[test]
    fn a_source_inside_a_block_is_heard_from_beside_it() {
        let mut world = World::new();
        world.set_block(9, 0, 0, Material::Wood);
        let field = Field::build(&world, LISTENER, 16);
        let route = field.route(&world, SOURCE).unwrap();
        assert!((route.length - 9.0).abs() < 1.5, "{route:?}");
    }

    #[test]
    fn diagonal_steps_dont_slip_between_blocks_touching_at_an_edge() {
        let mut world = World::new();
        // Two blocks meeting only at an edge, between (0,0,0) and (1,0,1).
        world.set_block(1, 0, 0, Material::Rock);
        world.set_block(0, 0, 1, Material::Rock);
        let field = Field::build(&world, LISTENER, 4);
        // Not 1.4 straight through the crack: round, at least 2.
        assert!(field.distance([1, 0, 1]).unwrap() >= 2.0);
    }
}
