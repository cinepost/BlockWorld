// Fixed before workers start; the stress generator is deliberately opt-in.
pub static NOISY_WORLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
use crate::config::*;
use crate::{terrain::{self,Biome,hash},caves::CaveField};
pub use crate::terrain::terrain_height;
use glam::{IVec3, Vec3};
use std::{collections::HashMap, sync::Arc};

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Block {
    #[default]
    Air,
    Stone,
    Grass,
    Dirt,
    Sand,
    Water,
    Wood,
    // IDs 7..=11 belong to polygon materials. Keep the shared material table stable.
    Snow = 12,
    OakLeaves = 13,
    PineLeaves = 14,
    Gravel = 15,
    Sandstone = 16,
    Granite = 17,
    MeadowGrass = 18,
    Podzol = 19,
}
impl Block {
    pub fn solid(self) -> bool {
        self != Self::Air && self != Self::Water
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Air => "air",
            Self::Stone => "stone",
            Self::Grass => "grass",
            Self::Dirt => "dirt",
            Self::Sand => "sand",
            Self::Water => "water",
            Self::Wood => "wood",
            Self::Snow => "snow",
            Self::OakLeaves => "oak leaves",
            Self::PineLeaves => "pine needles",
            Self::Gravel=>"gravel",
            Self::Sandstone=>"sandstone",
            Self::Granite=>"granite",
            Self::MeadowGrass=>"meadow grass",
            Self::Podzol=>"podzol",
        }
    }
}
/// One byte per block; X contiguous, then Z, then Y. Cache-line aligned allocation.
#[derive(Clone)]
#[repr(C, align(64))]
pub struct SubChunk {
    blocks: [Block; SUBCHUNK_VOLUME],
}
impl SubChunk {
    fn empty() -> Box<Self> {
        Box::new(Self {
            blocks: [Block::Air; SUBCHUNK_VOLUME],
        })
    }
    pub const fn index(x: usize, y: usize, z: usize) -> usize {
        x | (z << SUBCHUNK_BITS) | (y << (2 * SUBCHUNK_BITS))
    }
}
/// Stable identity is the world-space tree grid cell, independent of chunk load order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TreeId(pub i32, pub i32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeKind {
    Oak,
    Pine,
}
#[derive(Clone, Debug)]
pub struct Tree {
    pub id: TreeId,
    pub kind: TreeKind,
    pub root: IVec3,
    pub blocks: Vec<(IVec3, Block)>,
}
const TREE_SPACING: i32 = 16;
const TREE_RADIUS: i32 = 3;
impl Tree {
    fn generate(gx: i32, gz: i32, seed: u32) -> Option<Self> {
        let key = hash(gx, gz, seed ^ 0x74726565);
        let x = gx * TREE_SPACING + 2 + ((key >> 8) % 8) as i32;
        let z = gz * TREE_SPACING + 2 + ((key >> 16) % 8) as i32;
        let terrain=terrain::sample(x,z,seed);let h=terrain.height;
        let chance=match terrain.biome {Biome::Forest=>72,Biome::Meadow=>14,Biome::Plains=>12,_=>0};
        if key%100>=chance || terrain::entrance(x,z,seed) {return None;}
        if h <= SEA_LEVEL + 2 || h >= SNOW_LEVEL - 6 {
            return None;
        }
        if [(3, 0), (-3, 0), (0, 3), (0, -3)]
            .iter()
            .any(|&(dx, dz)| (terrain_height(x + dx, z + dz, seed) - h).abs() > 3)
        {
            return None;
        }
        let kind = if terrain.temperature<0.43 || h>70 {
            TreeKind::Pine
        } else {
            TreeKind::Oak
        };
        let height = match kind {
            TreeKind::Oak => 5 + (key % 3) as i32,
            TreeKind::Pine => 8 + (key % 4) as i32,
        };
        let root = IVec3::new(x, h + 1, z);
        let leaf = match kind {
            TreeKind::Oak => Block::OakLeaves,
            TreeKind::Pine => Block::PineLeaves,
        };
        let mut blocks = Vec::new();
        for dy in 0..=height {
            let radius = match kind {
                TreeKind::Oak => {
                    if dy >= height - 3 {
                        3
                    } else {
                        0
                    }
                }
                TreeKind::Pine => {
                    if dy >= 2 {
                        ((height - dy) / 3 + 1).min(3)
                    } else {
                        0
                    }
                }
            };
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    let trunk = dx == 0 && dz == 0 && dy < height;
                    let crown = radius > 0
                        && dx * dx + dz * dz <= radius * radius + 1
                        && (kind == TreeKind::Pine || dy < height || dx * dx + dz * dz <= 4);
                    if !trunk && !crown {
                        continue;
                    }
                    let p = root + IVec3::new(dx, dy, dz);
                    if p.y < MAX_Y && p.y > terrain_height(p.x, p.z, seed) {
                        blocks.push((p, if trunk { Block::Wood } else { leaf }));
                    }
                }
            }
        }
        Some(Self {
            id: TreeId(gx, gz),
            kind,
            root,
            blocks,
        })
    }
}
#[derive(Clone)]
pub struct Chunk {
    biomes:[Biome;EDGE*EDGE],
    sections: [Option<Box<SubChunk>>; SUBCHUNKS],
    trees: Vec<Arc<Tree>>,
    // Local full-height voxel index -> tree descriptor index. Edits detach membership.
    owners: HashMap<u32, usize>,
}
impl Chunk {
    fn empty() -> Self {
        Self {
            biomes:[Biome::Plains;EDGE*EDGE],
            sections: std::array::from_fn(|_| None),
            trees: Vec::new(),
            owners: HashMap::new(),
        }
    }
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.sections.iter().filter(|s| s.is_some()).count() * std::mem::size_of::<SubChunk>()
            + self.owners.capacity() * std::mem::size_of::<(u32, usize)>()
            + self.trees.capacity() * std::mem::size_of::<Arc<Tree>>()
            + self
                .trees
                .iter()
                .map(|t| {
                    std::mem::size_of::<Tree>()
                        + t.blocks.capacity() * std::mem::size_of::<(IVec3, Block)>()
                })
                .sum::<usize>()
    }
    pub(crate) fn get(&self, x: usize, y: usize, z: usize) -> Block {
        self.sections[y / EDGE].as_ref().map_or(Block::Air, |s| {
            s.blocks[SubChunk::index(x, y & (EDGE - 1), z)]
        })
    }
    pub(crate) fn set(&mut self, x: usize, y: usize, z: usize, b: Block) {
        self.owners.remove(&((x + EDGE * (z + EDGE * y)) as u32));
        if b == Block::Air && self.sections[y / EDGE].is_none() {
            return;
        }
        self.sections[y / EDGE]
            .get_or_insert_with(SubChunk::empty)
            .blocks[SubChunk::index(x, y & (EDGE - 1), z)] = b;
    }
    /// Independent per-voxel noise: no coherent surfaces or homogeneous parents.
    /// A clear upper layer allows inspection from a flying camera.
    pub(crate) fn generate_noisy(cx: i32, cz: i32, seed: u32) -> Self {
        let mut c=Self::empty();
        for y in MIN_Y..64 {for z in 0..EDGE {for x in 0..EDGE {
            let h=hash(cx*16+x as i32,cz*16+z as i32,seed ^ (y as u32).wrapping_mul(0x9e3779b9));
            let b=match h%8 {0..=2=>Block::Air,3=>Block::Stone,4=>Block::Dirt,5=>Block::Sand,6=>Block::Wood,_=>Block::Snow};
            c.set(x,(y-MIN_Y) as usize,z,b);
        }}}
        c
    }
    pub(crate) fn generate(cx: i32, cz: i32, seed: u32) -> Self {
        if NOISY_WORLD.load(std::sync::atomic::Ordering::Relaxed) {
            return Self::generate_noisy(cx, cz, seed);
        }
        let mut c=Self::empty();
        let columns:Vec<_>=(0..EDGE).flat_map(|z|(0..EDGE).map(move |x|terrain::sample(cx*16+x as i32,cz*16+z as i32,seed))).collect();
        let highest=columns.iter().map(|t|t.height).max().unwrap();
        let caves=CaveField::new(cx,cz,highest,seed);
        for z in 0..EDGE {for x in 0..EDGE {
            let wx=cx*16+x as i32;let wz=cz*16+z as i32;let t=columns[x+EDGE*z];
            c.biomes[x+EDGE*z]=t.biome;
            let entrance=t.height>SEA_LEVEL+6 && terrain::entrance(wx,wz,seed);
            let wet=crate::caves::aquifer(wx,wz,seed);
            for y in MIN_Y..=t.height.max(SEA_LEVEL) {
                let depth=t.height-y;
                let carve=y>=MIN_Y+4 && y<=t.height && (depth>=4 || entrance) && caves.density(x,(y-MIN_Y) as usize,z)>0.0;
                let b=if carve {if wet && y< -24 {Block::Water}else{Block::Air}} else {terrain::layer(t,y)};
                if b!=Block::Air {c.set(x,(y-MIN_Y) as usize,z,b);}
            }
        }}
        // Enumerate neighboring tree cells too, so canopies crossing chunk/region borders
        // have identical blocks and identity regardless of which chunk is generated first.
        let low_x = cx * EDGE as i32;
        let low_z = cz * EDGE as i32;
        for gz in (low_z - TREE_RADIUS).div_euclid(TREE_SPACING)
            ..=(low_z + EDGE as i32 - 1 + TREE_RADIUS).div_euclid(TREE_SPACING)
        {
            for gx in (low_x - TREE_RADIUS).div_euclid(TREE_SPACING)
                ..=(low_x + EDGE as i32 - 1 + TREE_RADIUS).div_euclid(TREE_SPACING)
            {
                let Some(tree) = Tree::generate(gx, gz, seed) else {
                    continue;
                };
                let index = c.trees.len();
                for &(p, b) in &tree.blocks {
                    let x = p.x - low_x;
                    let z = p.z - low_z;
                    if !(0..EDGE as i32).contains(&x) || !(0..EDGE as i32).contains(&z) {
                        continue;
                    }
                    let y = (p.y - MIN_Y) as usize;
                    c.set(x as usize, y, z as usize, b);
                    c.owners.insert(
                        x as u32 + EDGE as u32 * (z as u32 + EDGE as u32 * y as u32),
                        index,
                    );
                }
                if c.owners.values().any(|&v| v == index) {
                    c.trees.push(Arc::new(tree));
                }
            }
        }
        c
    }
}
/// Fixed region grid of chunk slots. Only nearby chunks and non-air sections allocate.
#[derive(Clone)]
pub struct Region {
    chunks: Box<[Option<Arc<Chunk>>; REGION_CHUNKS]>,
}
impl Region {
    fn new() -> Self {
        Self {
            chunks: (0..REGION_CHUNKS)
                .map(|_| None)
                .collect::<Vec<_>>()
                .into_boxed_slice()
                .try_into()
                .ok()
                .unwrap(),
        }
    }
}
#[derive(Clone)]
pub struct World {
    clock: u64,
    touched: HashMap<(i32, i32), u64>,
    pub edits: HashMap<IVec3, Block>,
    regions: HashMap<(i32, i32), Region>,
    pub seed: u32,
}
impl World {
    pub fn new(seed: u32) -> Self {
        Self {
            regions: HashMap::new(),
            clock: 0,
            touched: HashMap::new(),
            edits: HashMap::new(),
            seed,
        }
    }
    /// Empty resident chunks for physics fixtures, independent of procedural terrain.
    #[cfg(test)]
    pub fn fixture() -> Self {
        let mut w = Self::new(42);
        for z in -2..2 {
            for x in -2..2 {
                let (r, i) = Self::address(x, z);
                w.regions.entry(r).or_insert_with(Region::new).chunks[i] =
                    Some(Arc::new(Chunk::empty()));
                w.touched.insert((x, z), 0);
            }
        }
        w
    }
    fn address(cx: i32, cz: i32) -> ((i32, i32), usize) {
        let r = REGION_EDGE as i32;
        (
            (cx.div_euclid(r), cz.div_euclid(r)),
            cz.rem_euclid(r) as usize * REGION_EDGE + cx.rem_euclid(r) as usize,
        )
    }
    fn chunk(&self, cx: i32, cz: i32) -> Option<&Chunk> {
        let (r, i) = Self::address(cx, cz);
        self.regions.get(&r)?.chunks[i].as_deref()
    }
    fn ensure(&mut self, cx: i32, cz: i32) -> &mut Chunk {
        self.clock += 1;
        self.touched.insert((cx, cz), self.clock);
        let (r, i) = Self::address(cx, cz);
        if self.chunk(cx, cz).is_none() {
            let mut chunk = Chunk::generate(cx, cz, self.seed);
            for (p, &block) in &self.edits {
                if p.x.div_euclid(EDGE as i32) == cx && p.z.div_euclid(EDGE as i32) == cz {
                    chunk.set(
                        p.x.rem_euclid(EDGE as i32) as usize,
                        (p.y - MIN_Y) as usize,
                        p.z.rem_euclid(EDGE as i32) as usize,
                        block,
                    );
                }
            }
            self.regions.entry(r).or_insert_with(Region::new).chunks[i] = Some(Arc::new(chunk));
        }
        Arc::make_mut(
            self.regions.get_mut(&r).unwrap().chunks[i]
                .as_mut()
                .unwrap(),
        )
    }
    /// Stable generated biome identity survives edits; unloaded queries stay cheap.
    pub fn biome_at(&self,x:i32,z:i32)->Biome {
        self.chunk(x.div_euclid(16),z.div_euclid(16)).map_or_else(||terrain::sample(x,z,self.seed).biome,|c|c.biomes[x.rem_euclid(16) as usize+EDGE*z.rem_euclid(16) as usize])
    }
    pub fn resident(&self, cx: i32, cz: i32) -> bool {
        self.chunk(cx, cz).is_some()
    }
    pub fn apply_edits(&mut self, edits: HashMap<IVec3, Block>) {
        for (&p, &block) in &edits {
            if self.edits.get(&p) == Some(&block) {
                continue;
            }
            if self.resident(p.x.div_euclid(16), p.z.div_euclid(16)) {
                self.set(p, block);
            }
        }
        self.edits = edits;
    }
    pub fn trim_cache(&mut self, center: (i32, i32), radius: i32, budget: usize) -> CacheStats {
        let bytes = Chunk::bytes;
        let mut used = 0;
        let mut pinned = 0;
        let mut chunks = 0;
        let mut candidates = Vec::new();
        for (&(cx, cz), &age) in &self.touched {
            if let Some(c) = self.chunk(cx, cz) {
                let size = bytes(c);
                used += size;
                chunks += 1;
                if (cx - center.0).abs() <= radius && (cz - center.1).abs() <= radius {
                    pinned += size;
                } else {
                    candidates.push((age, cx, cz, size));
                }
            }
        }
        candidates.sort_unstable();
        let mut evicted = 0;
        for (_, cx, cz, size) in candidates {
            if used <= budget {
                break;
            }
            let (r, i) = Self::address(cx, cz);
            self.regions.get_mut(&r).unwrap().chunks[i] = None;
            self.touched.remove(&(cx, cz));
            used -= size;
            chunks -= 1;
            evicted += 1;
        }
        self.regions
            .retain(|_, r| r.chunks.iter().any(Option::is_some));
        CacheStats {
            bytes: used,
            pinned,
            chunks,
            evicted,
            budget,
        }
    }
    pub fn load_around(&mut self, cx: i32, cz: i32) {
        self.load_radius(cx, cz, LOAD_RADIUS);
    }
    pub fn load_radius(&mut self, cx: i32, cz: i32, radius: i32) {
        self.load_radius_progress(cx,cz,radius,|_,_| true);
    }
    pub fn load_radius_progress(&mut self, cx:i32, cz:i32, radius:i32, mut progress:impl FnMut(usize,usize)->bool) -> bool {
        let mut done=0;
        let total=((radius*2+1) as usize).pow(2);
        for z in cz - radius..=cz + radius {
            for x in cx - radius..=cx + radius {
                if self.resident(x, z) {
                    self.clock += 1;
                    self.touched.insert((x, z), self.clock);
                } else {
                    self.ensure(x, z);
                }
                done+=1; if !progress(done,total) {return false;}
            }
        }
        true
    }
    pub fn get(&self, p: IVec3) -> Block {
        if !(MIN_Y..MAX_Y).contains(&p.y) {
            return Block::Air;
        }
        let e = EDGE as i32;
        self.chunk(p.x.div_euclid(e), p.z.div_euclid(e))
            .map_or(Block::Air, |c| {
                c.get(
                    p.x.rem_euclid(e) as usize,
                    (p.y - MIN_Y) as usize,
                    p.z.rem_euclid(e) as usize,
                )
            })
    }
    pub fn set(&mut self, p: IVec3, b: Block) -> bool {
        if !(MIN_Y..MAX_Y).contains(&p.y) {
            return false;
        }
        self.edits.insert(p, b);
        let e = EDGE as i32;
        self.ensure(p.x.div_euclid(e), p.z.div_euclid(e)).set(
            p.x.rem_euclid(e) as usize,
            (p.y - MIN_Y) as usize,
            p.z.rem_euclid(e) as usize,
            b,
        );
        true
    }
    pub fn tree_at(&self, p: IVec3) -> Option<Arc<Tree>> {
        if !(MIN_Y..MAX_Y).contains(&p.y) {
            return None;
        }
        let c = self.chunk(p.x.div_euclid(16), p.z.div_euclid(16))?;
        let index = p.x.rem_euclid(16) as u32
            + 16 * (p.z.rem_euclid(16) as u32 + 16 * (p.y - MIN_Y) as u32);
        Some(c.trees[*c.owners.get(&index)?].clone())
    }
    /// Destroy a generated collection atomically in the edit journal. Replaced/placed
    /// blocks have detached membership and survive even when touching the old tree.
    pub fn destroy(&mut self, p: IVec3) -> usize {
        if let Some(tree) = self.tree_at(p) {
            let mut count = 0;
            for &(cell, _) in &tree.blocks {
                if !self.edits.contains_key(&cell) {
                    count += self.set(cell, Block::Air) as usize;
                }
            }
            count
        } else if self.get(p) != Block::Air {
            self.set(p, Block::Air) as usize
        } else {
            0
        }
    }
    /// Dense upload staging window. CPU world retains the region/chunk/section hierarchy.
    #[cfg(test)]
    pub fn snapshot(&self, cx: i32, cz: i32) -> (IVec3, Vec<u8>) {
        self.snapshot_radius(cx, cz, LOAD_RADIUS)
    }
    pub fn snapshot_radius(&self, cx: i32, cz: i32, radius: i32) -> (IVec3, Vec<u8>) {
        assert!(radius >= 0);
        let side = (radius * 2 + 1) as usize;
        let edge = side * EDGE;
        let origin = IVec3::new(
            (cx - radius) * EDGE as i32,
            MIN_Y,
            (cz - radius) * EDGE as i32,
        );
        let mut out = vec![0; edge * edge * HEIGHT];
        // Resolve the region hash once per chunk, then copy contiguous X rows.
        // Missing sections remain zero without visiting every air voxel.
        for cz_local in 0..side {
            for cx_local in 0..side {
                let Some(chunk) =
                    self.chunk(cx - radius + cx_local as i32, cz - radius + cz_local as i32)
                else {
                    continue;
                };
                for (sy, section) in chunk.sections.iter().enumerate() {
                    let Some(section) = section else {
                        continue;
                    };
                    for y in 0..EDGE {
                        for z in 0..EDGE {
                            let src = SubChunk::index(0, y, z);
                            let dst = cx_local * EDGE
                                + edge * (cz_local * EDGE + z + edge * (sy * EDGE + y));
                            for x in 0..EDGE {
                                out[dst + x] = section.blocks[src + x] as u8;
                            }
                        }
                    }
                }
            }
        }
        (origin, out)
    }
    /// Amanatides-Woo traversal; returns hit and adjacent cell for placement.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, reach: f32) -> Option<(IVec3, IVec3)> {
        let dir = dir.normalize_or_zero();
        if dir == Vec3::ZERO {
            return None;
        }
        let mut cell = origin.floor().as_ivec3();
        let mut previous = cell;
        let step = dir.signum().as_ivec3();
        let delta = dir.recip().abs();
        let mut side = Vec3::splat(f32::INFINITY);
        for a in 0..3 {
            if dir[a] != 0.0 {
                side[a] = if dir[a] > 0.0 {
                    (cell[a] as f32 + 1.0 - origin[a]) / dir[a]
                } else {
                    (cell[a] as f32 - origin[a]) / dir[a]
                };
            }
        }
        let mut t = 0.0;
        while t <= reach {
            if self.get(cell) != Block::Air {
                return Some((cell, previous));
            }
            let a = if side.x <= side.y && side.x <= side.z {
                0
            } else if side.y <= side.z {
                1
            } else {
                2
            };
            t = side[a];
            side[a] += delta[a];
            previous = cell;
            cell[a] += step[a];
        }
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn example_tree(kind: TreeKind, boundary: bool) -> Tree {
        (-64..64)
            .flat_map(|z| (-64..64).map(move |x| (x, z)))
            .filter_map(|(x, z)| Tree::generate(x, z, 42))
            .find(|t| {
                t.kind == kind
                    && (!boundary
                        || t.blocks.iter().any(|(p, _)| {
                            p.x.div_euclid((EDGE * REGION_EDGE) as i32)
                                != t.root.x.div_euclid((EDGE * REGION_EDGE) as i32)
                                || p.z.div_euclid((EDGE * REGION_EDGE) as i32)
                                    != t.root.z.div_euclid((EDGE * REGION_EDGE) as i32)
                        }))
            })
            .expect("Generated tree fixture")
    }
    #[test]
    fn tree_collections_cross_boundaries_and_survive_eviction() {
        for kind in [TreeKind::Oak, TreeKind::Pine] {
            let tree = example_tree(kind, true);
            let mut w = World::new(42);
            let middle = tree.root + IVec3::Y * 2;
            w.load_radius(middle.x.div_euclid(16), middle.z.div_euclid(16), 0);
            assert_eq!(w.tree_at(middle).unwrap().id, tree.id);
            // A manually replaced leaf is now independent, even if its material is wood.
            let replacement = tree
                .blocks
                .iter()
                .find(|(_, b)| *b != Block::Wood)
                .unwrap()
                .0;
            w.set(replacement, Block::Wood);
            assert!(w.tree_at(replacement).is_none());
            assert_eq!(w.destroy(middle), tree.blocks.len() - 1);
            let journal = w.edits.clone();
            let mut worker = World::new(42);
            worker.apply_edits(journal);
            w.trim_cache((999, 999), 0, 1);
            assert!(!w.resident(middle.x.div_euclid(16), middle.z.div_euclid(16)));
            for &(p, _) in &tree.blocks {
                for world in [&mut w, &mut worker] {
                    world.load_radius(p.x.div_euclid(16), p.z.div_euclid(16), 0);
                    assert_eq!(
                        world.get(p),
                        if p == replacement {
                            Block::Wood
                        } else {
                            Block::Air
                        }
                    );
                    assert!(world.tree_at(p).is_none());
                }
            }
        }
    }
    #[test]
    fn neighboring_trees_do_not_share_blocks() {
        let mut cells = HashMap::new();
        for z in -12..12 {
            for x in -12..12 {
                if let Some(t) = Tree::generate(x, z, 42) {
                    for (p, _) in t.blocks {
                        assert!(
                            cells.insert(p, t.id).is_none(),
                            "Overlapping generated collections"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn terrain_has_broad_valleys_and_snowy_mountains() {
        let mut low = MAX_Y;
        let mut high = MIN_Y;
        let mut summit = (0, 0);
        for z in (-4096..4096).step_by(32) {
            for x in (-4096..4096).step_by(32) {
                let h = terrain_height(x, z, 42);
                low = low.min(h);
                if h > high {
                    high = h;
                    summit = (x, z);
                }
            }
        }
        assert!(
            low < SEA_LEVEL && high > SNOW_LEVEL + 20,
            "range {low}..{high}"
        );
        let (x, z) = summit;
        let c = Chunk::generate(x.div_euclid(16), z.div_euclid(16), 42);
        assert_eq!(
            c.get(
                x.rem_euclid(16) as usize,
                (high - MIN_Y) as usize,
                z.rem_euclid(16) as usize
            ),
            Block::Snow
        );
        assert!((terrain_height(x + 1, z, 42) - high).abs() <= 4);
    }
    #[test]
    fn lru_evicts_distant_chunks_and_replays_edits() {
        let mut w = World::new(42);
        w.load_radius(0, 0, 1);
        let edit = IVec3::new(0, 100, 0);
        w.set(edit, Block::Wood);
        let shared = w.clone();
        // Loading an existing chunk must not clone its section allocations.
        let before = Arc::as_ptr(w.regions.get(&(0, 0)).unwrap().chunks[0].as_ref().unwrap());
        w.load_radius(0, 0, 1);
        assert_eq!(
            before,
            Arc::as_ptr(w.regions.get(&(0, 0)).unwrap().chunks[0].as_ref().unwrap())
        );
        w.load_radius(10, 0, 1);
        let stats = w.trim_cache((10, 0), 1, 1);
        assert_eq!(stats.chunks, 9);
        assert_eq!(stats.evicted, 9);
        assert_eq!(stats.bytes, stats.pinned);
        assert!(!w.resident(0, 0));
        assert_eq!(shared.get(edit), Block::Wood);
        w.load_radius(0, 0, 0);
        assert_eq!(w.get(edit), Block::Wood);
        let stats = w.trim_cache((0, 0), 0, 1);
        assert_eq!(stats.chunks, 1);
        assert_eq!(
            w.regions.len(),
            1,
            "Empty region tables must also be evicted"
        );
    }
    #[test]
    fn least_recently_used_unpinned_chunk_is_removed_first() {
        let mut w = World::new(42);
        w.load_radius(0, 0, 0);
        w.load_radius(1, 0, 0);
        w.load_radius(0, 0, 0);
        w.load_radius(2, 0, 0);
        let bytes = Chunk::bytes;
        let budget = bytes(w.chunk(0, 0).unwrap()) + bytes(w.chunk(2, 0).unwrap());
        let stats = w.trim_cache((2, 0), 0, budget);
        assert_eq!(stats.evicted, 1);
        assert!(w.resident(0, 0));
        assert!(!w.resident(1, 0));
        assert!(w.resident(2, 0));
    }
    #[test]
    fn layout() {
        assert_eq!(std::mem::size_of::<SubChunk>(), 4096);
        assert_eq!(std::mem::align_of::<SubChunk>(), 64);
        assert_eq!(SubChunk::index(15, 15, 15), 4095);
        assert_eq!((MIN_Y, MAX_Y), (-64, 320));
    }
    #[test]
    fn negative_boundaries() {
        let mut w = World::new(1);
        for p in [
            IVec3::new(-257, -64, -257),
            IVec3::new(-1, -1, -1),
            IVec3::new(256, 319, 256),
        ] {
            assert!(w.set(p, Block::Wood));
            assert_eq!(w.get(p), Block::Wood);
            w.set(p, Block::Air);
            assert_eq!(w.get(p), Block::Air);
        }
        assert!(!w.set(IVec3::new(0, 320, 0), Block::Stone));
    }
    #[test]
    fn ray_face_and_range() {
        let mut w = World::new(7);
        let p = IVec3::new(0, 100, 0);
        w.set(p, Block::Stone);
        let o = Vec3::new(0.5, 100.5, 3.5);
        assert_eq!(w.raycast(o, -Vec3::Z, 7.0), Some((p, p + IVec3::Z)));
        assert!(w.raycast(o, -Vec3::Z, 1.0).is_none());
        assert_eq!(
            w.raycast(Vec3::new(-2.5, 100.5, 0.5), Vec3::X, 7.0),
            Some((p, p - IVec3::X))
        );
    }
    #[test]
    fn upload_window_matches_world_across_sections() {
        let mut w = World::new(42);
        w.set(IVec3::new(-64, -64, -48), Block::Wood);
        w.set(IVec3::new(-1, 319, 0), Block::Sand);
        let (origin, data) = w.snapshot(-1, 0);
        for y in [0, 63, 64, 80, HEIGHT - 1] {
            for z in (0..GPU_EDGE).step_by(7) {
                for x in (0..GPU_EDGE).step_by(11) {
                    assert_eq!(
                        data[x + GPU_EDGE * (z + GPU_EDGE * y)],
                        w.get(origin + IVec3::new(x as i32, y as i32, z as i32)) as u8
                    );
                }
            }
        }
        let q = IVec3::new(-64, -64, -48) - origin;
        assert_eq!(
            data[q.x as usize + GPU_EDGE * (q.z as usize + GPU_EDGE * q.y as usize)],
            Block::Wood as u8
        );
        let q = IVec3::new(-1, 319, 0) - origin;
        assert_eq!(
            data[q.x as usize + GPU_EDGE * (q.z as usize + GPU_EDGE * q.y as usize)],
            Block::Sand as u8
        );
    }
    #[test]
    fn deterministic_generation() {
        let a = Chunk::generate(-1, 2, 42);
        let b = Chunk::generate(-1, 2, 42);
        for y in 0..HEIGHT {
            for z in 0..EDGE {
                for x in 0..EDGE {
                    assert_eq!(a.get(x, y, z), b.get(x, y, z));
                }
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct CacheStats {
    pub bytes: usize,
    pub pinned: usize,
    pub chunks: usize,
    pub evicted: usize,
    pub budget: usize,
}

#[cfg(test)]
mod biome_world_tests {
    use super::*;
    #[test]
    fn biome_metadata_is_stable_across_edits_and_eviction_and_caves_keep_floor() {
        let mut w=World::new(42);let p=IVec3::new(-9,10,21);
        let expected=w.biome_at(p.x,p.z);w.load_radius(-1,1,0);
        assert_eq!(w.biome_at(p.x,p.z),expected);
        w.set(p,Block::Sandstone);assert_eq!(w.biome_at(p.x,p.z),expected);
        w.trim_cache((40,40),0,0);assert_eq!(w.biome_at(p.x,p.z),expected);
        w.load_radius(-1,1,0);assert_eq!(w.biome_at(p.x,p.z),expected);
        assert_eq!(w.get(p),Block::Sandstone);
        let chunk=Chunk::generate(-1,1,42);
        for z in 0..16 {for x in 0..16 {for y in 0..4 {assert!(chunk.get(x,y,z).solid());}}}
    }
}

#[cfg(test)]
mod cave_generation_tests {
    use super::*;
    #[test]
    fn carved_caves_exist_below_land_and_covered_surfaces_stay_solid() {
        let (x,z)=(-2048i32..2048).step_by(32).flat_map(|z|(-2048i32..2048).step_by(32).map(move |x|(x,z)))
            .find(|&(x,z)|terrain_height(x,z,42)>40).expect("Land fixture");
        let cx=x.div_euclid(16);let cz=z.div_euclid(16);let chunk=Chunk::generate(cx,cz,42);let mut carved=0;
        for z in 0..16 {for x in 0..16 {
            let wx=cx*16+x as i32;let wz=cz*16+z as i32;let h=terrain_height(wx,wz,42);
            for y in MIN_Y+4..h-4 {if !chunk.get(x,(y-MIN_Y) as usize,z).solid() {carved+=1;}}
            if !terrain::entrance(wx,wz,42) {
                for depth in 0..4 {assert!(chunk.get(x,(h-depth-MIN_Y) as usize,z).solid());}
            }
        }}
        assert!(carved>32,"Cave field was not carved into runtime chunks");
    }
}
