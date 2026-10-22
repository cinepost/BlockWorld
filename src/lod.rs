//! Sparse chunk-addressed SVO forests. CPU preparation never touches Vulkan.
//! Cache keys include the generator version, seed and chunk coordinates; replacing
//! the bounded RAM cache with a disk-backed store does not change the page format.
use crate::{
    config::*,
    renderer::{Buffer, ContextVk},
    world::{Block, Chunk},
};
use anyhow::Result;
use ash::vk;
use glam::{IVec3, Vec3};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
};

pub const PAGE_SIDE: usize = (MAX_VIEW_CHUNKS as usize * 2 + 3).next_power_of_two();
const MERGE_BASE: usize = 8 + PAGE_SIDE * PAGE_SIDE * 8;
pub const TABLE_WORDS: usize = MERGE_BASE + PAGE_SIDE * PAGE_SIDE * SUBCHUNKS;
pub const POOL_BYTES: usize = 256 * 1024 * 1024;
const UPLOAD_BYTES: usize = 1024 * 1024;
const CACHE_BYTES: usize = 64 * 1024 * 1024;
pub const GENERATOR_VERSION: u32 = 3;
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct CacheKey {
    pub version: u32,
    pub seed: u32,
    pub x: i32,
    pub z: i32,
}
#[derive(Clone)]
struct Level {
    scale: usize,
    low: usize,
    high: usize,
    data: Vec<u8>,
}
// Each node is two words: relative first-child index, then child mask (8 bits)
// and aggregate material palette index (8 bits). Positions follow the octant path.
#[derive(Default,Clone)]
struct SvoNode {children:Vec<(usize,SvoNode)>,material:u8}
impl Level {
    fn encode_svo(mut self)->Self {
        fn build(level:&Level,low:[usize;3],size:usize)->SvoNode {
            if low[1]+size<=level.low*level.scale || low[1]>=level.high*level.scale {return SvoNode::default();}
            if size>level.scale && low[1]>=level.low*level.scale && low[1]+size<=level.high*level.scale {
                let q=low.map(|v| v/level.scale);let side=16/level.scale;let span=size/level.scale;
                let first=level.data[q[0]+side*(q[2]+side*(q[1]-level.low))];
                let uniform=(0..span).all(|dy| (0..span).all(|dz| {
                    let start=q[0]+side*(q[2]+dz+side*(q[1]+dy-level.low));
                    level.data[start..start+span].iter().all(|v| *v==first)
                }));
                if uniform {return SvoNode{material:first,children:vec![]};}
            }
            if size==level.scale {
                let q=low.map(|v| v/level.scale);let side=EDGE/level.scale;
                let material=if q[1]>=level.low && q[1]<level.high {level.data[q[0]+side*(q[2]+side*(q[1]-level.low))]} else {0};
                return SvoNode{material,children:vec![]};
            }
            let half=size/2;let mut all=Vec::new();
            for octant in 0..8 {
                let offset=[octant&1,(octant>>1)&1,(octant>>2)&1];
                all.push(build(level,std::array::from_fn(|a| low[a]+offset[a]*half),half));
            }
            if all.iter().all(|n| n.children.is_empty() && n.material==all[0].material) {
                return SvoNode{material:all[0].material,children:vec![]};
            }
            let mut counts=[0u8;MATERIAL_COUNT+1];
            for n in &all {counts[n.material as usize]+=1;}
            let mut material=(1..=MATERIAL_COUNT).max_by_key(|i| counts[*i]).unwrap();
            if counts[material]==0 {material=0;}
            if material==1 || material==3 {for surface in [2,4,12,18,19] {if counts[surface]>0 {material=surface;break;}}}
            SvoNode{material:material as u8,children:all.into_iter().enumerate().filter(|(_,n)| n.material!=0).collect()}
        }
        fn write(node:SvoNode,index:usize,nodes:&mut Vec<[u32;2]>) {
            let mask=node.children.iter().fold(0u32,|v,(i,_)|v|(1<<i));
            let base=nodes.len();nodes[index]=[if mask==0 {0} else {base as u32},mask|((node.material as u32)<<8)];
            nodes.resize(base+node.children.len(),[0;2]);
            for (i,(_,child)) in node.children.into_iter().enumerate() {write(child,base+i,nodes);}
        }
        let mut roots=Vec::new();let mut mask=0u32;
        for y in 0..SUBCHUNKS {
            if (y+1)*EDGE<=self.low*self.scale || y*EDGE>=self.high*self.scale {continue;}
            let node=build(&self,[0,y*EDGE,0],EDGE);
            if node.material!=0 {mask|=1<<y;roots.push(node);}
        }
        let mut nodes=vec![[0u32;2];roots.len()];
        for (index,node) in roots.into_iter().enumerate() {write(node,index,&mut nodes);}
        self.data=mask.to_le_bytes().to_vec();self.data.extend_from_slice(bytemuck::cast_slice(&nodes));
        self.low=self.low*self.scale/EDGE*EDGE/self.scale;
        self.high=(self.high*self.scale).div_ceil(EDGE)*EDGE/self.scale;
        self
    }
    #[cfg(test)]
    fn sample_svo(&self,x:usize,y:usize,z:usize)->u8 {
        let word=|i:usize| u32::from_le_bytes(self.data[i*4..i*4+4].try_into().unwrap());
        let mask=word(0);let section=y/16;if mask&(1<<section)==0 {return 0;}let mut node=(mask&((1<<section)-1)).count_ones() as usize;let mut size=16;
        loop {
            let base=1+node*2;let bits=word(base+1);let mask=bits&255;
            if mask==0 {return ((bits>>8)&255) as u8;}
            size/=2;let octant=usize::from(x&size!=0)|(usize::from(y&size!=0)<<1)|(usize::from(z&size!=0)<<2);
            if mask&(1<<octant)==0 {return 0;}
            node=word(base) as usize+(mask&((1<<octant)-1)).count_ones() as usize;
        }
    }
}
fn uniform_roots(level:&Level)->[u8;SUBCHUNKS] {
    // Majority-reduced coarse leaves are not proof of homogeneous source data.
    if level.scale!=1 {return [255;SUBCHUNKS];}
    let word=|i:usize|u32::from_le_bytes(level.data[i*4..i*4+4].try_into().unwrap());
    let roots=word(0);
    std::array::from_fn(|y| {
        if roots&(1<<y)==0 {return 0;}
        let node=(roots&((1<<y)-1)).count_ones() as usize;
        let bits=word(2+node*2);
        if bits&255==0 {(bits>>8) as u8} else {255}
    })
}
// Complete homogeneous 32/64m parents only. Unknown pages cannot become air.
fn merged_cell(pages:&HashMap<(i32,i32),Resident>,c:(i32,i32),section:usize)->u32 {
    let mut result=0;
    for span in [2i32,4] {
        let x=c.0.div_euclid(span)*span;let z=c.1.div_euclid(span)*span;
        let y=section/span as usize*span as usize;
        let Some(first)=pages.get(&(x,z)) else {continue;};
        let material=first.uniform[y];if material==255 {continue;}
        let uniform=(z..z+span).all(|z| (x..x+span).all(|x| pages.get(&(x,z)).is_some_and(|p|
            p.uniform[y..y+span as usize].iter().all(|m| *m==material))));
        if uniform {result=((span as u32*16)<<8)|material as u32;}
    }
    result
}
struct Pyramid {
    levels: Vec<Level>,
}
impl Pyramid {
    fn build(chunk: &Chunk) -> Self {
        let mut data = vec![0; EDGE * EDGE * HEIGHT];
        for y in 0..HEIGHT {
            for z in 0..EDGE {
                for x in 0..EDGE {
                    data[x + EDGE * (z + EDGE * y)] = chunk.get(x, y, z) as u8;
                }
            }
        }
        let mut levels = Vec::new();
        let mut scale = 1;
        loop {
            let side = EDGE / scale;
            let height = HEIGHT / scale;
            let slab = side * side;
            let low = (0..height)
                .find(|y| data[y * slab..(y + 1) * slab].iter().any(|b| *b != 0))
                .unwrap_or(0);
            let high = (0..height)
                .rfind(|y| data[y * slab..(y + 1) * slab].iter().any(|b| *b != 0))
                .map_or(low, |y| y + 1);
            levels.push(Level {
                scale,
                low,
                high,
                data: data[low * slab..high * slab].to_vec(),
            }.encode_svo());
            if scale == EDGE {
                break;
            }
            let ns = side / 2;
            let mut next = vec![0; ns * ns * (height / 2)];
            for y in 0..height / 2 {
                for z in 0..ns {
                    for x in 0..ns {
                        let mut counts = [0u8; MATERIAL_COUNT+1];
                        for dy in 0..2 {
                            for dz in 0..2 {
                                for dx in 0..2 {
                                    counts[data
                                        [x * 2 + dx + side * (z * 2 + dz + side * (y * 2 + dy))]
                                        as usize] += 1;
                                }
                            }
                        }
                        // Keep a surface palette instead of turning every mixed ground cell
                        // into stone. Air is excluded: distant silhouettes are conservative.
                        let mut material = (1..=MATERIAL_COUNT).max_by_key(|i| counts[*i]).unwrap();
                        if counts[material] == 0 {
                            material = 0;
                        }
                        if material == 1 || material == 3 {
                            for surface in [2, 4, 12, 18, 19] {
                                if counts[surface] > 0 {
                                    material = surface;
                                    break;
                                }
                            }
                        }
                        next[x + ns * (z + ns * y)] = material as u8;
                    }
                }
            }
            data = next;
            scale *= 2;
        }
        Self { levels }
    }
    fn bytes(&self) -> usize {
        self.levels.iter().map(|l| l.data.len()).sum()
    }
}
/// Conservative distance to a chunk AABB; the entire chunk uses its finest
/// required level. Thus no section can become coarser than the pixel criterion.
pub fn level_for(eye: Vec3, chunk: (i32, i32), height: u32, pixels: f32) -> usize {
    let low = Vec3::new((chunk.0 * 16) as f32, MIN_Y as f32, (chunk.1 * 16) as f32);
    let high = low + Vec3::new(16.0, HEIGHT as f32, 16.0);
    let distance = (eye - eye.clamp(low, high)).length().max(0.01);
    let focal = height as f32 / (2.0 * (FOV_Y_DEGREES.to_radians() * 0.5).tan());
    let mut level = 0;
    while level < 4 && ((1usize << (level + 1)) as f32 * focal / distance) <= pixels {
        level += 1;
    }
    level
}
/// Track the edit journal by chunk, including journal removals/restores. The
/// version of an untouched chunk survives unrelated edits and view changes.
#[derive(Default)]
struct EditVersions {
    journal: HashMap<IVec3, Block>,
    chunks: HashMap<(i32, i32), u64>,
    epoch: u64,
}
impl EditVersions {
    fn update(&mut self, edits: &HashMap<IVec3, Block>) {
        if self.journal == *edits {
            return;
        }
        let mut changed = std::collections::HashSet::new();
        for p in self.journal.keys().chain(edits.keys()) {
            if self.journal.get(p) != edits.get(p) {
                changed.insert((p.x.div_euclid(16), p.z.div_euclid(16)));
            }
        }
        self.epoch += 1;
        for coord in changed {
            self.chunks.insert(coord, self.epoch);
        }
        self.journal = edits.clone();
    }
    fn version(&self, coord: (i32, i32)) -> u64 {
        self.chunks.get(&coord).copied().unwrap_or(0)
    }
}
#[derive(Clone)]
struct Request {
    eye: Vec3,
    center: (i32, i32),
    view: i32,
    height: u32,
    pixels: f32,
    versions: HashMap<(i32, i32), u64>,
    resident:HashMap<(i32,i32),(u64,usize)>,
    full_detail:bool,
    include_near:bool,
    edits: HashMap<IVec3, Block>,
}
struct Page {
    coord: (i32, i32),
    version: u64,
    level: Level,
}
struct CpuWorker {
    tx: mpsc::Sender<Request>,
    rx: mpsc::Receiver<Page>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl CpuWorker {
    fn new(seed: u32) -> Result<Self> {
        let (tx, requests) = mpsc::channel::<Request>();
        let (done, rx) = mpsc::sync_channel(16);
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = stop.clone();
        let thread = thread::Builder::new()
            .name("terrain-lod".into())
            .spawn(move || {
                let mut cache = HashMap::<CacheKey, (Pyramid, u64)>::new();
                let mut bytes = 0;
                let mut clock = 0;
                let mut sent = HashMap::<(i32, i32), (u64, usize)>::new();
                let mut versions = HashMap::<(i32, i32), u64>::new();
                let mut last_pixels=None;
                let mut pending = None;
                while !cancel.load(Ordering::Relaxed) {
                    let mut r = match pending.take().or_else(|| requests.recv().ok()) {
                        Some(r) => r,
                        None => break,
                    };
                    while let Ok(new) = requests.try_recv() {
                        r = new;
                    }
                    // Actual VRAM pressure can have rejected queued pages. Re-offer
                    // them when the quality budget changes; the GPU side skips
                    // already-resident identical versions/scales.
                    if last_pixels.is_some_and(|p| p!=(r.pixels.to_bits(),r.full_detail,r.include_near)) {sent=r.resident.clone();}
                    last_pixels=Some((r.pixels.to_bits(),r.full_detail,r.include_near));
                    // A content change invalidates only this chunk's cached
                    // pyramid. Published GPU pages remain owned by the renderer.
                    cache.retain(|key, (pyramid, _)| {
                        let coord = (key.x, key.z);
                        if versions.get(&coord) != r.versions.get(&coord) {
                            bytes -= pyramid.bytes();
                            false
                        } else {
                            true
                        }
                    });
                    versions = r.versions.clone();
                    sent.retain(|&(x, z), _| {
                        (x - r.center.0).abs() <= r.view && (z - r.center.1).abs() <= r.view
                    });
                    let mut chunks = Vec::new();
                    for dz in -r.view..=r.view {
                        for dx in -r.view..=r.view {
                            // Include the near window as a handoff fallback. Rendering always
                            // gives exact resident voxels priority over a coarse page.
                            if !r.include_near && dx.abs().max(dz.abs()) <= NEAR_VIEW_CHUNKS {
                                continue;
                            }
                            chunks.push((dx * dx + dz * dz, (r.center.0 + dx, r.center.1 + dz)));
                        }
                    }
                    chunks.sort_unstable_by_key(|v| {
                        let version = r.versions.get(&v.1).copied().unwrap_or(0);
                        let dirty = sent.get(&v.1).is_some_and(|old| old.0 != version);
                        (!dirty, v.0)
                    });
                    for (_, coord) in chunks {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Ok(new) = requests.try_recv() {
                            pending = Some(new);
                            break;
                        }
                        let level = if r.full_detail {0} else {level_for(r.eye, coord, r.height, r.pixels*0.5)};
                        let version = r.versions.get(&coord).copied().unwrap_or(0);
                        if sent.get(&coord) == Some(&(version, level)) {
                            continue;
                        }
                        let key = CacheKey {
                            version: GENERATOR_VERSION | (u32::from(crate::world::NOISY_WORLD.load(Ordering::Relaxed)) << 31),
                            seed,
                            x: coord.0,
                            z: coord.1,
                        };
                        clock += 1;
                        if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(key) {
                            let mut chunk = Chunk::generate(coord.0, coord.1, seed);
                            for (p, b) in &r.edits {
                                if p.x.div_euclid(16) == coord.0
                                    && p.z.div_euclid(16) == coord.1
                                    && (MIN_Y..MAX_Y).contains(&p.y)
                                {
                                    chunk.set(
                                        p.x.rem_euclid(16) as usize,
                                        (p.y - MIN_Y) as usize,
                                        p.z.rem_euclid(16) as usize,
                                        *b,
                                    );
                                }
                            }
                            let pyramid = Pyramid::build(&chunk);
                            bytes += pyramid.bytes();
                            entry.insert((pyramid, clock));
                        }
                        let entry = cache.get_mut(&key).unwrap();
                        entry.1 = clock;
                        let page = Page {
                            coord,
                            version,
                            level: entry.0.levels[level].clone(),
                        };
                        // Bounded result queue plus cancellable backpressure.
                        let mut page = page;
                        loop {
                            match done.try_send(page) {
                                Ok(()) => break,
                                Err(mpsc::TrySendError::Disconnected(_)) => return,
                                Err(mpsc::TrySendError::Full(p)) => {
                                    page = p;
                                    if cancel.load(Ordering::Relaxed) {
                                        return;
                                    }
                                    thread::park_timeout(std::time::Duration::from_millis(2));
                                }
                            }
                        }
                        sent.insert(coord, (version, level));
                        while bytes > CACHE_BYTES {
                            let Some(key) = cache.iter().min_by_key(|(_, v)| v.1).map(|(k, _)| *k)
                            else {
                                break;
                            };
                            bytes -= cache.remove(&key).unwrap().0.bytes();
                        }
                    }
                }
            })?;
        Ok(Self {
            tx,
            rx,
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for CpuWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Request {
            eye: Vec3::ZERO,
            center: (0, 0),
            view: 0,
            height: 1,
            pixels: 1.0,
            versions: HashMap::new(),
                resident:HashMap::new(),
                full_detail:true,
                include_near:false,
            edits: HashMap::new(),
        });
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Default)]
struct Allocator {
    free: BTreeMap<usize, usize>,
}
impl Allocator {
    fn new() -> Self {
        Self {
            free: BTreeMap::from([(TABLE_WORDS * 4, POOL_BYTES - TABLE_WORDS * 4)]),
        }
    }
    fn alloc(&mut self, size: usize) -> Option<usize> {
        let (&offset, &len) = self.free.iter().find(|(_, n)| **n >= size)?;
        self.free.remove(&offset);
        if len > size {
            self.free.insert(offset + size, len - size);
        }
        Some(offset)
    }
    fn release(&mut self, mut offset: usize, mut len: usize) {
        if let Some((&a, &n)) = self.free.range(..offset).next_back() {
            if a + n == offset {
                self.free.remove(&a);
                offset = a;
                len += n;
            }
        }
        if let Some((&a, &n)) = self.free.range(offset..).next() {
            if offset + len == a {
                self.free.remove(&a);
                len += n;
            }
        }
        self.free.insert(offset, len);
    }
}
struct Resident {
    offset: usize,
    size: usize,
    scale: usize,
    low: usize,
    high: usize,
    version:u64,
    uniform:[u8;SUBCHUNKS],
}
type RequestKey = ((i32, i32), i32, u32, u32, u64, IVec3, bool);
pub struct LodGpu {
    pub buffer: Buffer,
    pub gi:bool,
    staging: Buffer,
    worker: Option<CpuWorker>,
    allocator: Allocator,
    pages: HashMap<(i32, i32), Resident>,
    updates: Vec<(usize, [u32; 8])>,
    copies: Vec<vk::BufferCopy>,
    request_key: Option<RequestKey>,
    initial: bool,
    edit_versions: EditVersions,
    pending_page: Option<Page>,
    merge_dirty:std::collections::HashSet<(i32,i32)>,
    pub effective_pixels: f32,
    pressure_pixels:f32,
    pressure_pending:bool,
    last_pressure:Option<std::time::Instant>,
    pub uploaded: u64,
    pub epoch: u32,
    pub levels: [usize; 5],
    pub bytes: usize,
}
impl LodGpu {
    pub fn new(ctx: Arc<ContextVk>) -> Result<Self> {
        Ok(Self {
            buffer: Buffer::new(
                ctx.clone(),
                POOL_BYTES,
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
                false,
            )?,
            gi:false,
            staging: Buffer::new(ctx, UPLOAD_BYTES, vk::BufferUsageFlags::TRANSFER_SRC, true)?,
            worker: None,
            allocator: Allocator::new(),
            pages: HashMap::new(),
            updates: Vec::new(),
            copies: Vec::new(),
            request_key: None,
            initial: true,
            edit_versions: EditVersions::default(),
            pending_page: None,
            merge_dirty:Default::default(),
            effective_pixels: 1.0,
            pressure_pixels:1.0,
            pressure_pending:false,
            last_pressure:None,
            uploaded: 0,
            epoch: 0,
            levels: [0; 5],
            bytes: 0,
        })
    }
    fn slot(coord: (i32, i32)) -> usize {
        8 + (coord.0.rem_euclid(PAGE_SIDE as i32) as usize
            + PAGE_SIDE * coord.1.rem_euclid(PAGE_SIDE as i32) as usize)
            * 8
    }
    fn remove(&mut self, c: (i32, i32)) {
        self.merge_dirty.insert((c.0.div_euclid(4)*4,c.1.div_euclid(4)*4));
        if let Some(r) = self.pages.remove(&c) {
            self.epoch = self.epoch.wrapping_add(1);
            self.allocator.release(r.offset, r.size);
            self.copies.retain(|c| c.dst_offset != r.offset as u64);
            self.updates.push((Self::slot(c), [0; 8]));
        }
    }
    pub fn prepare(
        &mut self,
        eye: Vec3,
        view: i32,
        height: u32,
        pixels: f32,
        world: &crate::world::World,
    ) -> Result<()> {
        if !self.copies.is_empty() {
            return Ok(());
        }
        self.edit_versions.update(&world.edits);
        let center = (
            (eye.x.floor() as i32).div_euclid(16),
            (eye.z.floor() as i32).div_euclid(16),
        );
        let key = (
            center,
            view,
            height,
            pixels.to_bits(),
            self.edit_versions.epoch,
            (eye / 4.0).floor().as_ivec3(),
            self.gi,
        );
        if self.request_key.is_some_and(|old| old.3!=pixels.to_bits() || view<old.1) {
            self.pressure_pixels=pixels;self.pressure_pending=false;self.last_pressure=None;
        }
        if self.pressure_pending && self.last_pressure.is_none_or(|t| t.elapsed().as_secs_f32()>=1.0) {
            self.pressure_pixels=(self.effective_pixels*1.15).min(128.0).max(pixels);
            self.last_pressure=Some(std::time::Instant::now());self.pressure_pending=false;self.request_key=None;
        }
        if self.request_key != Some(key) {
            // SVOs compress uniform terrain; dense full-height estimates were
            // forcing ~7px detail even with ample VRAM. Only measured pressure
            // may relax the requested residency target now.
            self.effective_pixels=pixels.max(self.pressure_pixels);
            let stale: Vec<_> = self
                .pages
                .keys()
                .filter(|&&(x, z)| (x - center.0).abs() > view || (z - center.1).abs() > view || (!self.gi && (x-center.0).abs().max((z-center.1).abs())<=NEAR_VIEW_CHUNKS))
                .copied()
                .collect();
            for c in stale {
                self.remove(c);
            }
            self.request_key = Some(key);
            if (view > NEAR_VIEW_CHUNKS || self.gi) && self.worker.is_none() {
                self.worker = Some(CpuWorker::new(world.seed)?);
            }
            if let Some(worker) = &self.worker {
                worker.tx.send(Request {
                    eye,
                    center,
                    view,
                    height,
                    pixels: self.effective_pixels,
                    versions: self.edit_versions.chunks.clone(),
                    full_detail:self.effective_pixels<=pixels,
                    include_near:self.gi,
                    resident:self.pages.iter().map(|(&c,p)|(c,(p.version,p.scale.trailing_zeros() as usize))).collect(),
                    edits: world.edits.clone(),
                })?;
            }
        }
        let mut payload = Vec::new();
        let mut resident_bytes=self.pages.values().map(|p|p.size).sum::<usize>();
        for _ in 0..8 {
            let Some(page) = self
                .pending_page
                .take()
                .or_else(|| self.worker.as_ref().and_then(|w| w.rx.try_recv().ok()))
            else {
                break;
            };
            if (!self.gi && (page.coord.0-center.0).abs().max((page.coord.1-center.1).abs())<=NEAR_VIEW_CHUNKS)
                || page.version != self.edit_versions.version(page.coord)
                || (page.coord.0 - center.0).abs() > view
                || (page.coord.1 - center.1).abs() > view
            {
                continue;
            }
            if self.pages.get(&page.coord).is_some_and(|old| old.scale==page.level.scale && old.version==page.version) {continue;}
            let size = (page.level.data.len().max(4) + 3) & !3;
            if payload.len() + size > UPLOAD_BYTES {
                self.pending_page=Some(page);
                break;
            }
            let previous=self.pages.get(&page.coord).map_or(0,|p| p.size);
            // Keep real headroom for staging/replacements. Coarser replacements
            // are always allowed to shrink the working set; rejected new/finer
            // pages are re-offered after a pressure-driven quality request.
            if resident_bytes+size-previous>(POOL_BYTES-TABLE_WORDS*4)*9/10 && size>=previous {
                self.pressure_pending=true;
                continue;
            }
            // Allocate the replacement first: allocation pressure must never
            // remove a still-visible old page. Retry later if the pool is full.
            let Some(offset) = self.allocator.alloc(size) else {
                self.pressure_pending=true;
                continue;
            };
            resident_bytes=resident_bytes+size-previous;
            self.remove(page.coord);
            let start = payload.len();
            payload.extend(&page.level.data);
            payload.resize(start + size, 0);
            self.copies.push(
                vk::BufferCopy::default()
                    .src_offset(start as u64)
                    .dst_offset(offset as u64)
                    .size(size as u64),
            );
            self.updates.push((
                Self::slot(page.coord),
                [
                    page.coord.0 as u32,
                    page.coord.1 as u32,
                    (offset / 4) as u32,
                    page.level.scale as u32,
                    page.level.low as u32,
                    page.level.high as u32,
                    2,
                    0,
                ],
            ));
            self.pages.insert(
                page.coord,
                Resident {
                    offset,
                    size,
                    scale: page.level.scale,
                    low: page.level.low,
                    high: page.level.high,
                    version:page.version,
                    uniform:uniform_roots(&page.level),
                },
            );
            self.uploaded += 1;
            self.epoch = self.epoch.wrapping_add(1);
        }
        if !payload.is_empty() {
            self.staging.write(&payload);
        }
        self.levels = [0; 5];
        self.bytes = 0;
        for p in self.pages.values() {
            self.levels[p.scale.trailing_zeros() as usize] += 1;
            self.bytes += p.size;
        }
        for (gx,gz) in self.merge_dirty.drain().collect::<Vec<_>>() {
            for z in gz..gz+4 {for x in gx..gx+4 {
                if !self.pages.contains_key(&(x,z)) {continue;}
                let values:[u32;SUBCHUNKS]=std::array::from_fn(|y|merged_cell(&self.pages,(x,z),y));
                let index=(Self::slot((x,z))-8)/8;
                for i in 0..3 {self.updates.push((MERGE_BASE+index*SUBCHUNKS+i*8,values[i*8..i*8+8].try_into().unwrap()));}
            }}
        }
        self.updates.push((0, self.header()));
        Ok(())
    }
    fn header(&self) -> [u32; 8] {
        let mut low = IVec3::splat(i32::MAX);
        let mut high = IVec3::splat(i32::MIN);
        for (&(x, z), p) in &self.pages {
            if p.high == p.low {
                continue;
            }
            low = low.min(IVec3::new(x * 16, MIN_Y + (p.low * p.scale) as i32, z * 16));
            high = high.max(IVec3::new(
                x * 16 + 16,
                MIN_Y + (p.high * p.scale) as i32,
                z * 16 + 16,
            ));
        }
        if low.x == i32::MAX {
            return [0; 8];
        }
        [
            1,
            low.x as u32,
            low.z as u32,
            high.x as u32,
            high.z as u32,
            low.y as u32,
            high.y as u32,
            0,
        ]
    }
    pub fn active(&self) -> bool {
        !self.pages.is_empty()
    }
    pub fn visible(&self, frustum: &crate::spatial::Frustum) -> bool {
        self.pages.iter().any(|(&(x, z), p)| {
            p.high > p.low
                && frustum.intersects(
                    Vec3::new(
                        (x * 16) as f32,
                        MIN_Y as f32 + (p.low * p.scale) as f32,
                        (z * 16) as f32,
                    ),
                    Vec3::new(
                        (x * 16 + 16) as f32,
                        MIN_Y as f32 + (p.high * p.scale) as f32,
                        (z * 16 + 16) as f32,
                    ),
                )
        })
    }
    /// Deterministic sparse-page fixture, uploaded through the normal transfer path.
    pub(crate) fn fixture(&mut self,coord:(i32,i32),block:IVec3,scale:usize,material:u8) {
        self.fixture_format(coord,block,scale,material,false);
    }
    pub(crate) fn fixture_svo(&mut self,coord:(i32,i32),block:IVec3,scale:usize,material:u8) {
        self.fixture_format(coord,block,scale,material,true);
    }
    pub(crate) fn fixture_format(&mut self, coord: (i32, i32), block: IVec3, scale: usize, material: u8, svo:bool) {
        self.request_key = None;
        let y = (block.y - MIN_Y) as usize / scale;
        let side = 16 / scale;
        let size = (side * side).max(4);
        let mut data = vec![0u8; size];
        data[block.x.rem_euclid(16) as usize / scale
            + side * (block.z.rem_euclid(16) as usize / scale)] = material;
        let level=if svo {Level{scale,low:y,high:y+1,data:data[..side*side].to_vec()}.encode_svo()}
            else {Level{scale,low:y,high:y+1,data}};
        let uniform=if svo {uniform_roots(&level)} else {[255;SUBCHUNKS]};
        let data=level.data;let size=data.len();
        self.remove(coord);
        let offset = self.allocator.alloc(size).unwrap();
        self.staging.write(&data);
        self.copies.push(
            vk::BufferCopy::default()
                .dst_offset(offset as u64)
                .size(size as u64),
        );
        self.updates.push((
            Self::slot(coord),
            [
                coord.0 as u32,
                coord.1 as u32,
                (offset / 4) as u32,
                scale as u32,
                level.low as u32,
                level.high as u32,
                if svo {2} else {1},
                0,
            ],
        ));

        self.pages.insert(
            coord,
            Resident {
                offset,
                size,
                scale,
                low: level.low,
                high: level.high,
                version:0,
                uniform,
            },
        );
        let merge=MERGE_BASE+(Self::slot(coord)-8)/8*SUBCHUNKS;
        for i in 0..3 {self.updates.push((merge+i*8,[0;8]));}
        self.updates.push((0, self.header()));
    }
    /// Called only after the previous render fence, before any compute readers.
    pub unsafe fn record(&mut self, d: &ash::Device, cmd: vk::CommandBuffer) {
        if self.initial {
            d.cmd_fill_buffer(cmd, self.buffer.handle, 0, (TABLE_WORDS * 4) as u64, 0);
            self.initial = false;
            let b = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
            d.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[b],
                &[],
                &[],
            );
        }
        if !self.copies.is_empty() {
            d.cmd_copy_buffer(cmd, self.staging.handle, self.buffer.handle, &self.copies);
            self.copies.clear();
        }
        let updates: HashMap<_, _> = self.updates.drain(..).collect();
        for (offset, record) in updates {
            d.cmd_update_buffer(
                cmd,
                self.buffer.handle,
                (offset * 4) as u64,
                bytemuck::cast_slice(&record),
            );
        }
        let barrier = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ);
        d.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::DependencyFlags::empty(),
            &[barrier],
            &[],
            &[],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_is_monotonic() {
        let eye = Vec3::new(0.0, 50.0, 0.0);
        let levels: Vec<_> = (10..=64)
            .map(|x| level_for(eye, (x, 0), 1080, 8.0))
            .collect();
        assert!(levels.windows(2).all(|v| v[0] <= v[1]));
        assert_eq!(level_for(eye, (1, 0), 1080, 1.0), 0);
        assert!(level_for(eye, (64, 0), 1080, 8.0) >= 2);
    }
    #[test]
    fn pages_do_not_alias_inside_window() {
        let mut slots = std::collections::HashSet::new();
        for z in -64..=64 {
            for x in -64..=64 {
                assert!(slots.insert(LodGpu::slot((x, z))));
            }
        }
    }
    #[test]
    fn worker_publishes_before_finishing_and_cancels_backpressure() {
        let worker = CpuWorker::new(42).unwrap();
        worker
            .tx
            .send(Request {
                eye: Vec3::new(0.5, 80.0, 0.5),
                center: (0, 0),
                view: 64,
                height: 540,
                pixels: 64.0,
                versions: HashMap::new(),
                resident:HashMap::new(),
                full_detail:true,
                include_near:false,
                edits: HashMap::new(),
            })
            .unwrap();
        let first = worker
            .rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert_eq!(first.coord.0.abs().max(first.coord.1.abs()), 9);
        assert_eq!(first.level.scale,1,"GPU LOD requires a full-detail resident tree before actual pressure");
        assert!(first.level.data.len() < EDGE * EDGE * HEIGHT);
        // Dropping while the bounded output queue fills must still join promptly.
        drop(worker);
    }
    #[test]
    fn edit_versions_are_local_and_track_removal_and_cross_chunk_changes() {
        let mut edits = HashMap::new();
        let mut versions = EditVersions::default();
        let a = IVec3::new(-1, 10, 0);
        let b = IVec3::new(16, 20, 0);
        edits.insert(a, Block::Wood);
        versions.update(&edits);
        let first = versions.version((-1, 0));
        assert_ne!(first, 0);
        assert_eq!(versions.version((0, 0)), 0);
        versions.update(&edits);
        assert_eq!(versions.epoch, first);
        edits.insert(b, Block::Stone);
        versions.update(&edits);
        assert_eq!(versions.version((-1, 0)), first);
        assert_ne!(versions.version((1, 0)), 0);
        edits.remove(&a);
        versions.update(&edits);
        assert!(versions.version((-1, 0)) > first);
        let other = versions.version((1, 0));
        edits.insert(a, Block::Air);
        edits.insert(IVec3::new(15, 10, 0), Block::Air);
        versions.update(&edits);
        assert_eq!(versions.version((1, 0)), other);
        assert_ne!(versions.version((0, 0)), 0);
    }
    #[test]
    fn worker_rebuilds_only_modified_chunk_after_unrelated_edit() {
        let worker = CpuWorker::new(42).unwrap();
        let mut r = Request {
            eye: Vec3::new(0.5, 80.0, 0.5),
            center: (0, 0),
            view: 9,
            height: 540,
            pixels: 1.0,
            versions: HashMap::new(),
                resident:HashMap::new(),
                full_detail:true,
                include_near:false,
            edits: HashMap::new(),
        };
        worker.tx.send(r.clone()).unwrap();
        for _ in 0..72 {
            worker
                .rx
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
        }
        r.edits.insert(IVec3::new(0, 200, 0), Block::Wood);
        r.versions.insert((0, 0), 1);
        worker.tx.send(r.clone()).unwrap();
        assert!(matches!(
            worker
                .rx
                .recv_timeout(std::time::Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        let point = IVec3::new(144, 200, 0);
        r.edits.insert(point, Block::Wood);
        r.versions.insert((9, 0), 2);
        worker.tx.send(r.clone()).unwrap();
        let page = worker
            .rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert_eq!(page.coord, (9, 0));
        assert_eq!(page.version, 2);
        assert_eq!(page.level.scale, 1);
        assert_eq!(
            page.level.sample_svo(0,(point.y-MIN_Y) as usize,0),
            Block::Wood as u8
        );
        assert!(matches!(
            worker
                .rx
                .recv_timeout(std::time::Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        // Another edit to the same chunk cannot reuse its old cached pyramid.
        r.edits.insert(point, Block::Air);
        r.versions.insert((9, 0), 3);
        worker.tx.send(r).unwrap();
        let page = worker
            .rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert_eq!(page.coord, (9, 0));
        assert_eq!(page.version, 3);
        assert!(page.level.high < (point.y - MIN_Y) as usize);
    }
    #[test]
    fn allocator_coalesces() {
        let mut a = Allocator::new();
        let initial = a.free.clone();
        let x = a.alloc(100).unwrap();
        let y = a.alloc(200).unwrap();
        a.release(x, 100);
        a.release(y, 200);
        assert_eq!(a.free, initial);
    }
    #[test]
    fn pyramid_preserves_surface_and_dimensions() {
        let c = Chunk::generate(-2, 3, 42);
        let p = Pyramid::build(&c);
        assert_eq!(p.levels.len(), 5);
        for l in p.levels {
            assert!(l.data.len()>=4 && l.data.len()%8==4);
            assert!(l.high<=HEIGHT/l.scale);
            assert!(l.data.iter().any(|v| *v != 0));
        }
    }
}

#[cfg(test)]
mod svo_tests {
    use super::*;
    #[test]
    fn uniform_sections_collapse_and_empty_sections_have_no_nodes() {
        let water=Level{scale:1,low:0,high:16,data:vec![5;4096]}.encode_svo();
        assert_eq!(water.data.len(),4+8);
        assert_eq!(water.sample_svo(15,15,15),5);
        assert_eq!(water.sample_svo(0,16,0),0);
        let air=Level{scale:1,low:0,high:16,data:vec![0;4096]}.encode_svo();
        assert_eq!(air.data.len(),4);
    }
    #[test]
    fn sparse_masks_and_relative_child_ranks_preserve_all_voxels() {
        let dense:Vec<u8>=(0..4096).map(|i| if i%11==0 {12} else if i%7==0 {5} else {0}).collect();
        let tree=Level{scale:1,low:32,high:48,data:dense.clone()}.encode_svo();
        for y in 0..16 {for z in 0..16 {for x in 0..16 {
            assert_eq!(tree.sample_svo(x,y+32,z),dense[x+16*(z+16*y)]);
        }}}
        assert_eq!(tree.sample_svo(0,31,0),0);
        assert_eq!(tree.sample_svo(0,48,0),0);
    }
    #[test]
    fn finest_tree_matches_generated_chunk_and_bounds_enclose_roots() {
        let chunk=Chunk::generate(-17,23,42);let tree=Pyramid::build(&chunk);
        let fine=&tree.levels[0];
        for y in 0..HEIGHT {for z in (0..16).step_by(3) {for x in (0..16).step_by(3) {
            assert_eq!(fine.sample_svo(x,y,z),chunk.get(x,y,z) as u8);
        }}}
        assert!(tree.levels.iter().all(|l| (l.low*l.scale)%16==0 && (l.high*l.scale)%16==0));
    }
}

#[cfg(test)]
mod cross_chunk_tests {
    use super::*;
    fn uniform(material:u8)->Resident {
        Resident{offset:0,size:0,scale:1,low:0,high:HEIGHT,version:0,uniform:[material;SUBCHUNKS]}
    }
    #[test]
    fn complete_negative_coordinate_parents_merge_and_missing_or_edited_children_split() {
        let mut pages=HashMap::new();
        for z in -4..0 {for x in -4..0 {pages.insert((x,z),uniform(5));}}
        assert_eq!(merged_cell(&pages,(-1,-1),3),(64<<8)|5);
        pages.remove(&(-4,-4));
        assert_eq!(merged_cell(&pages,(-1,-1),3),(32<<8)|5);
        pages.get_mut(&(-1,-1)).unwrap().uniform[3]=255;
        assert_eq!(merged_cell(&pages,(-1,-1),3),0);
        pages.insert((-4,-4),uniform(5));
        assert_eq!(merged_cell(&pages,(-1,-1),4),(64<<8)|5);
        pages.remove(&(-4,-4));
        for p in pages.values_mut() {p.uniform=[0;SUBCHUNKS];}
        assert_eq!(merged_cell(&pages,(-1,-1),3),32<<8);
    }
    #[test]
    fn noisy_volume_resists_compression_and_round_trips() {
        let chunk=Chunk::generate_noisy(-2,3,42);
        let fine=&Pyramid::build(&chunk).levels[0];
        // A random 128m slab takes substantially more storage than dense IDs.
        assert!(fine.data.len()>16*16*128);
        for y in (0..128).step_by(7) {for z in 0..16 {for x in 0..16 {
            assert_eq!(fine.sample_svo(x,y,z),chunk.get(x,y,z) as u8);
        }}}
        assert_eq!(uniform_roots(fine)[..8],[255;8]);
        assert_eq!(uniform_roots(fine)[8..],[0;16]);
    }
}

#[cfg(test)]
mod biome_palette_tests {
    use super::*;
    #[test]
    fn new_biome_ids_survive_sparse_tree_and_mip_aggregation() {
        let dense:Vec<_>=(0..4096).map(|i|15+(i%5) as u8).collect();
        let fine=Level{scale:1,low:0,high:16,data:dense.clone()}.encode_svo();
        for y in 0..16 {for z in 0..16 {for x in 0..16 {
            assert_eq!(fine.sample_svo(x,y,z),dense[x+16*(z+16*y)]);
        }}}
        let mut chunk=Chunk::generate_noisy(0,0,42);
        for y in 0..HEIGHT {for z in 0..16 {for x in 0..16 {chunk.set(x,y,z,Block::Podzol);}}}
        let pyramid=Pyramid::build(&chunk);
        for l in pyramid.levels {assert_eq!(l.sample_svo(0,128,0),Block::Podzol as u8);assert_eq!(l.data.len(),4+SUBCHUNKS*8);}
    }
}
