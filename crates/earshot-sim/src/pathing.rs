use earshot_world::{SectionPos, Vec3, World, cast};

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
    /// How big the narrowest opening on the way is, in square blocks, across
    /// the way the sound goes through it: a door is 2, an arch 9 or so, and
    /// anything over [`MAX_APERTURE`] is open.
    pub aperture: f32,
}

/// Openings are measured up to this size, in square blocks.
pub const MAX_APERTURE: f32 = 64.0;
/// And this far across from where the way passes, in blocks.
const APERTURE_REACH: i32 = 6;

impl Field {
    /// How far the field reaches each way from the listener's block.
    pub fn half(&self) -> i32 {
        self.size / 2
    }

    /// The field for a listener at `listener`, `half` blocks each way.
    pub fn build(world: &World, listener: Vec3, half: i32) -> Field {
        debug_assert!(
            half >= 0,
            "field half-extent must not be negative, got {half}"
        );
        let size = 2 * half + 1;
        let centre = block_of(listener);
        let min = centre.map(|c| c - half);
        let cells = (size * size * size) as usize;
        // Flooded in a grid one cell bigger all round, its rim shut, so no
        // step needs a bounds check.
        let padded = size + 2;
        let mut open = vec![false; (padded * padded * padded) as usize];
        for x in 0..size {
            for y in 0..size {
                let (wx, wy) = (min[0] + x, min[1] + y);
                let row = (((x + 1) * padded + y + 1) * padded + 1) as usize;
                let mut z = 0;
                while z < size {
                    // A section at a time along the row: one lookup per 16.
                    let wz = min[2] + z;
                    let run = (16 - (wz & 15)).min(size - z);
                    if let Some(section) = world.section(SectionPos::of_block(wx, wy, wz)) {
                        for i in 0..run {
                            open[row + (z + i) as usize] = !section.get(wx, wy, wz + i).is_solid();
                        }
                    } else {
                        open[row + z as usize..row + (z + run) as usize].fill(true);
                    }
                    z += run;
                }
            }
        }
        let mut field = Field {
            listener,
            min,
            size,
            dist: vec![UNREACHED; cells],
        };
        let flooded = flood(&open, padded, [half + 1; 3]);
        for x in 0..size {
            for y in 0..size {
                let from = (((x + 1) * padded + y + 1) * padded + 1) as usize;
                let to = ((x * size + y) * size) as usize;
                field.dist[to..to + size as usize]
                    .copy_from_slice(&flooded[from..from + size as usize]);
            }
        }
        field
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
        // Walk back towards the listener until the listener can see the way,
        // noting the narrowest opening the way squeezes through.
        let mut opening = self.centre(cell);
        let mut narrowest = MAX_APERTURE;
        while !self.visible(world, opening) {
            let next = NEIGHBOURS
                .iter()
                .map(|(step, _)| [cell[0] + step[0], cell[1] + step[1], cell[2] + step[2]])
                .filter(|&next| self.contains(next))
                .min_by_key(|&next| self.dist[self.index(next)])?;
            if self.dist[self.index(next)] >= self.dist[self.index(cell)] {
                break;
            }
            let step = self.centre(next) - self.centre(cell);
            narrowest = narrowest.min(aperture(world, self.origin(next), step));
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
                aperture: MAX_APERTURE,
            });
        }
        Some(Route {
            length,
            heard_from: self.listener + towards * length,
            bend: towards.dot(straight).clamp(-1.0, 1.0).acos(),
            aperture: narrowest,
        })
    }

    /// The world block of local cell `local`.
    fn origin(&self, local: [i32; 3]) -> [i32; 3] {
        [
            local[0] + self.min[0],
            local[1] + self.min[1],
            local[2] + self.min[2],
        ]
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

    /// The middle of local block `local`, in world coordinates.
    fn centre(&self, local: [i32; 3]) -> Vec3 {
        Vec3::new(
            f64::from(self.min[0] + local[0]) + 0.5,
            f64::from(self.min[1] + local[1]) + 0.5,
            f64::from(self.min[2] + local[2]) + 0.5,
        )
    }
}

/// The size of the opening at block `at`, in square blocks: the open blocks
/// joined to it (through faces) across the plane facing `through`, the way
/// the sound passes, out to [`APERTURE_REACH`], at most [`MAX_APERTURE`].
fn aperture(world: &World, at: [i32; 3], through: Vec3) -> f32 {
    let d = [through.x.abs(), through.y.abs(), through.z.abs()];
    let axis = if d[0] >= d[1] && d[0] >= d[2] {
        0
    } else if d[1] >= d[2] {
        1
    } else {
        2
    };
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let open = |a: i32, b: i32| {
        let mut cell = at;
        cell[u] += a;
        cell[v] += b;
        !world.block(cell[0], cell[1], cell[2]).is_solid()
    };
    if !open(0, 0) {
        return MAX_APERTURE;
    }
    let mut seen = std::collections::HashSet::from([(0i32, 0i32)]);
    let mut queue = std::collections::VecDeque::from([(0i32, 0i32)]);
    while let Some((a, b)) = queue.pop_front() {
        if seen.len() as f32 >= MAX_APERTURE {
            break;
        }
        for (na, nb) in [(a + 1, b), (a - 1, b), (a, b + 1), (a, b - 1)] {
            if na.abs() <= APERTURE_REACH
                && nb.abs() <= APERTURE_REACH
                && !seen.contains(&(na, nb))
                && open(na, nb)
            {
                seen.insert((na, nb));
                queue.push_back((na, nb));
            }
        }
    }
    (seen.len() as f32).min(MAX_APERTURE)
}

fn block_of(point: Vec3) -> [i32; 3] {
    [
        point.x.floor() as i32,
        point.y.floor() as i32,
        point.z.floor() as i32,
    ]
}

/// Dial's algorithm: Dijkstra with a ring of buckets, one per distance
/// modulo the largest step, over a `padded`³ grid whose rim is shut. A
/// diagonal step needs every block sharing the edge or corner it crosses
/// open too: it never slips past a solid block.
///
/// Invariant: `start` is always the in-field centre (`half + 1`, with
/// `half >= 0` asserted in `Field::build`) and the padded rim is never open,
/// so the signed strides below never reach an out-of-range index; a negative
/// `half` (size <= -1) is the caller breaking `Field::build`'s contract, not
/// a bounds case for `flood` to handle.
fn flood(open: &[bool], padded: i32, start: [i32; 3]) -> Vec<u16> {
    let stride = |[x, y, z]: [i32; 3]| ((x * padded + y) * padded + z) as isize;
    // Each neighbour: its offset, its cost, and the offsets of the blocks it
    // passes beside (none for a face step). An edge step passes beside the
    // two blocks on its other axes; a corner step also grazes the three
    // blocks diagonal across its pairs of axes, so all six must be open.
    let steps: Vec<(isize, u16, [isize; 6], usize)> = NEIGHBOURS
        .iter()
        .map(|&(step, cost)| {
            let axes: Vec<usize> = (0..3).filter(|&axis| step[axis] != 0).collect();
            let mut sides = [0; 6];
            let mut beside = 0;
            // Every non-empty proper subset of the axes crossed: the blocks
            // between this one and the neighbour that the step would clip.
            for mask in 1..(1usize << axes.len()) - 1 {
                let mut side = [0; 3];
                for (bit, &axis) in axes.iter().enumerate() {
                    if mask & (1 << bit) != 0 {
                        side[axis] = step[axis];
                    }
                }
                sides[beside] = stride(side);
                beside += 1;
            }
            (stride(step), cost, sides, beside)
        })
        .collect();
    let mut dist = vec![UNREACHED; open.len()];
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); usize::from(CORNER) + 1];
    let first = stride(start) as usize;
    dist[first] = 0;
    buckets[0].push(first as u32);
    let mut queued = 1usize;
    let mut d = 0u32;
    while queued > 0 {
        let bucket = (d % u32::from(CORNER + 1)) as usize;
        while let Some(cell) = buckets[bucket].pop() {
            queued -= 1;
            let cell = cell as usize;
            if u32::from(dist[cell]) != d {
                continue;
            }
            for (offset, cost, sides, beside) in &steps {
                let next = cell.wrapping_add_signed(*offset);
                let nd = d + u32::from(*cost);
                // Cheapest test first: most neighbours are already nearer.
                if nd >= u32::from(dist[next]) || nd >= u32::from(UNREACHED) {
                    continue;
                }
                if !open[next]
                    || !sides[..*beside]
                        .iter()
                        .all(|&side| open[cell.wrapping_add_signed(side)])
                {
                    continue;
                }
                dist[next] = nd as u16;
                buckets[(nd % u32::from(CORNER + 1)) as usize].push(next as u32);
                queued += 1;
            }
        }
        d += 1;
    }
    dist
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

    /// Plain Dijkstra over the 26 neighbours with every check spelt out,
    /// to hold the fast flood to.
    fn reference(world: &World, listener: Vec3, half: i32) -> Vec<u16> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let size = 2 * half + 1;
        let min = block_of(listener).map(|c| c - half);
        let inside = |p: [i32; 3]| p.iter().all(|&c| (0..size).contains(&c));
        let open = |p: [i32; 3]| {
            inside(p)
                && !world
                    .block(min[0] + p[0], min[1] + p[1], min[2] + p[2])
                    .is_solid()
        };
        let index = |[x, y, z]: [i32; 3]| ((x * size + y) * size + z) as usize;
        let mut dist = vec![UNREACHED; (size * size * size) as usize];
        let mut heap = BinaryHeap::new();
        dist[index([half; 3])] = 0;
        heap.push(Reverse((0u32, [half; 3])));
        while let Some(Reverse((d, at))) = heap.pop() {
            if u32::from(dist[index(at)]) != d {
                continue;
            }
            for (step, cost) in NEIGHBOURS {
                let to = [at[0] + step[0], at[1] + step[1], at[2] + step[2]];
                let axes: Vec<usize> = (0..3).filter(|&a| step[a] != 0).collect();
                // Every non-empty proper subset of the axes crossed: the
                // blocks the step would clip along its edge or corner.
                let beside_open = (1..(1usize << axes.len()) - 1).all(|mask| {
                    let mut side = at;
                    for (bit, &axis) in axes.iter().enumerate() {
                        if mask & (1 << bit) != 0 {
                            side[axis] += step[axis];
                        }
                    }
                    open(side)
                });
                let nd = d + u32::from(cost);
                if open(to) && beside_open && nd < u32::from(dist[index(to)]) {
                    dist[index(to)] = nd as u16;
                    heap.push(Reverse((nd, to)));
                }
            }
        }
        dist
    }

    #[test]
    fn the_flood_matches_plain_dijkstra() {
        let mut world = World::new();
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        // Walls, pillars and rubble across several sections.
        for _ in 0..3000 {
            let [x, y, z] = [next(), next(), next()].map(|v| (v % 40) as i32 - 20);
            world.set_block(x, y, z, Material::Rock);
        }
        for y in -20..20 {
            for z in -20..20 {
                if (y, z) != (0, 3) && (y, z) != (1, 3) {
                    world.set_block(6, y, z, Material::Rock);
                }
            }
        }
        for listener in [Vec3::new(0.5, 0.5, 0.5), Vec3::new(-7.2, 3.9, 11.1)] {
            world.set_block(
                listener.x.floor() as i32,
                listener.y.floor() as i32,
                listener.z.floor() as i32,
                Material::Air,
            );
            let field = Field::build(&world, listener, 12);
            assert_eq!(field.dist, reference(&world, listener, 12));
            assert!(field.dist.iter().filter(|&&d| d != UNREACHED).count() > 1000);
        }
    }

    #[test]
    fn a_door_is_a_small_opening_and_a_gap_a_wide_one() {
        // A wall at x = 4 across y, z in -8..8, with a 1 × 2 door at z = 0.
        let mut world = World::new();
        for y in -8..8 {
            for z in -8..8 {
                world.set_block(4, y, z, earshot_world::Material::Rock);
            }
        }
        world.set_block(4, 0, 0, earshot_world::Material::Air);
        world.set_block(4, 1, 0, earshot_world::Material::Air);
        assert_eq!(aperture(&world, [4, 0, 0], Vec3::new(1.0, 0.0, 0.0)), 2.0);
        // Knock out a 3 × 3 arch round it (the door's blocks among them).
        for y in -1..=1 {
            for z in -1..=1 {
                world.set_block(4, y, z, earshot_world::Material::Air);
            }
        }
        assert_eq!(aperture(&world, [4, 0, 0], Vec3::new(1.0, 0.0, 0.0)), 9.0);
        // With no wall at all, it's open.
        assert_eq!(
            aperture(&World::new(), [4, 0, 0], Vec3::new(1.0, 0.0, 0.0)),
            MAX_APERTURE
        );
    }

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

    #[test]
    fn diagonal_steps_dont_cut_past_a_block_at_a_corner() {
        let mut world = World::new();
        // (1, 1, 0) touches the line from the listener's block to (1, 1, 1)
        // at its corner. The three face neighbours are open, so a step that
        // checked only those would squeeze through the corner.
        world.set_block(1, 1, 0, Material::Rock);
        let field = Field::build(&world, LISTENER, 2);
        // Not 1.7 straight through the corner: round, at least 2.
        assert!(field.distance([1, 1, 1]).unwrap() >= 2.0);
    }

    #[test]
    fn distances_never_undersell_a_straight_line_or_skip_a_step() {
        let mut rng = crate::Rng::new(5);
        let mut world = World::new();
        for _ in 0..800 {
            let at =
                [rng.next_f64(), rng.next_f64(), rng.next_f64()].map(|v| (v * 28.0 - 14.0) as i32);
            world.set_block(at[0], at[1], at[2], Material::Rock);
        }
        let listener = Vec3::new(0.5, 0.5, 0.5);
        world.set_block(0, 0, 0, Material::Air);
        let field = Field::build(&world, listener, 12);
        let size = 25;
        for x in 0..size {
            for y in 0..size {
                for z in 0..size {
                    let local = [x, y, z];
                    if field.dist[field.index(local)] == UNREACHED {
                        assert_eq!(field.distance(field.origin(local)), None);
                        continue;
                    }
                    let way = field.distance(field.origin(local)).unwrap();
                    // Grid steps round their lengths down (1.7 against √3),
                    // so a way may undercut the crow-flies distance slightly
                    // — never past what those roundings allow. The field
                    // measures block to block, so compare block centres.
                    let block = field.origin(local);
                    let centre = block.map(|c| f64::from(c) + 0.5);
                    let straight = ((centre[0] - listener.x).powi(2)
                        + (centre[1] - listener.y).powi(2)
                        + (centre[2] - listener.z).powi(2))
                    .sqrt();
                    assert!(
                        way + 1e-9 >= straight * (1.7 / 3f64.sqrt()),
                        "({x},{y},{z}): {way} against {straight}"
                    );
                    // Every way but the listener's own block arrives over a
                    // step from a neighbour at least a face's cost nearer.
                    if local != [12, 12, 12] {
                        let nearer = NEIGHBOURS.iter().any(|(step, _)| {
                            let next = [local[0] + step[0], local[1] + step[1], local[2] + step[2]];
                            field.contains(next)
                                && field.dist[field.index(next)] != UNREACHED
                                && field.dist[field.index(next)] + FACE
                                    <= field.dist[field.index(local)]
                        });
                        assert!(nearer, "({x},{y},{z}) is reached from nowhere");
                    }
                }
            }
        }
    }

    #[test]
    fn a_one_block_field_is_only_the_listeners_own_block() {
        let world = World::new();
        let field = Field::build(&world, LISTENER, 0);
        assert_eq!(field.half(), 0);
        assert_eq!(field.distance([0, 0, 0]), Some(0.0));
        // The cube is 1³: a step in any direction is already outside it.
        for step in FACES {
            let outside = [step[0], step[1], step[2]];
            assert_eq!(field.distance(outside), None, "{outside:?}");
        }
        // The listener hears itself from itself, with no way to travel.
        let route = field.route(&world, LISTENER).unwrap();
        assert_eq!(route.length, 0.0);
        assert_eq!(route.heard_from, LISTENER);
        assert_eq!(route.bend, 0.0);
    }

    #[test]
    fn a_three_block_field_steps_onto_each_of_its_neighbours() {
        let world = World::new();
        let field = Field::build(&world, LISTENER, 1);
        assert_eq!(field.half(), 1);
        // The listener's own block, then a face, an edge and a corner step
        // across the 3³ cube (face, edge and corner costs).
        assert_eq!(field.distance([0, 0, 0]), Some(0.0));
        assert_eq!(field.distance([1, 0, 0]), Some(1.0));
        assert_eq!(field.distance([-1, 0, 0]), Some(1.0));
        assert_eq!(field.distance([1, 1, 0]), Some(1.4));
        assert_eq!(field.distance([1, 1, 1]), Some(1.7));
        assert_eq!(field.distance([-1, -1, -1]), Some(1.7));
        // One block past the rim: out of the field.
        assert_eq!(field.distance([2, 0, 0]), None);
        assert_eq!(field.distance([0, -2, 0]), None);
        // A source in the neighbouring block is heard one block away, straight.
        let route = field.route(&world, Vec3::new(1.5, 0.5, 0.5)).unwrap();
        assert!((route.length - 1.0).abs() < 1e-9, "{route:?}");
        assert!(route.bend < 1e-9, "{route:?}");
    }

    #[test]
    fn sources_beyond_the_field_or_buried_in_rock_have_no_way_round() {
        let world = wall(Some(6));
        let small = Field::build(&world, LISTENER, 8);
        assert!(small.route(&world, Vec3::new(30.5, 0.5, 0.5)).is_none());

        // A source sealed in a 3 × 3 × 3 boulder: no face of its block is open.
        let mut buried = wall(None);
        for dx in 8..11 {
            for dy in -1..2 {
                for dz in -1..2 {
                    buried.set_block(dx, dy, dz, Material::Rock);
                }
            }
        }
        let field = Field::build(&buried, LISTENER, 12);
        assert!(field.route(&buried, Vec3::new(9.5, 0.5, 0.5)).is_none());
    }

    #[test]
    fn a_doorway_is_heard_through_two_square_blocks() {
        let world = wall(Some(4));
        let field = Field::build(&world, LISTENER, 16);
        assert_eq!(field.route(&world, SOURCE).unwrap().aperture, 2.0);
        let heard = crate::hear(
            &world,
            Some(&field),
            LISTENER,
            SOURCE,
            5,
            1.0,
            crate::quality::DiffractionStandard::Iso9613_2,
        );
        assert_eq!(heard.aperture, 2.0, "{heard:?}");
    }
}
