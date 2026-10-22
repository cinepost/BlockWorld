//! Bounded asynchronous world preparation and a three-slot, immutable mesh/AS ring.
//! The render thread only adopts completed generations. Queue work and destruction
//! happen on the worker; no mapped geometry is overwritten while a reader owns it.
use crate::{
    config::*,
    entities::{ActivePlayer, EntitySystem},
    meshes::MeshScene,
    renderer::{Buffer, ContextVk},
    rt::RayTracing,
    spatial::{self, Section},
    world::{Block, CacheStats, World},
};
use anyhow::{anyhow, Result};
use ash::vk;
use glam::IVec3;
use std::{
    collections::{HashMap, VecDeque},
    sync::{mpsc, Arc},
    thread,
    time::Instant,
};

pub const AS_RING_SLOTS: usize = 3;
pub const AS_BUILD_BATCH: usize = 16;
pub struct Ready {
    ctx: Arc<ContextVk>,
    pub semaphore: vk::Semaphore,
}
impl Drop for Ready {
    fn drop(&mut self) {
        unsafe {
            self.ctx.device.destroy_semaphore(self.semaphore, None);
        }
    }
}
pub struct MeshFrame {
    pub objects: Buffer,
    pub triangles: Buffer,
    pub rt: Option<RayTracing>,
    pub scene: MeshScene,
    pub generation: u64,
    pub time: f32,
    pub prepare_ms: f32,
    pub gpu_ms: [f32; 2],
}
pub struct WorldGpu {
    pub voxels: Buffer,
    pub occupancy: Buffer,
    pub mask: Buffer,
    pub sections: Vec<Section>,
    pub section_bounds: Vec<spatial::SectionBounds>,
    pub voxel_y: [u32; 2],
    pub edge: usize,
    pub origin: IVec3,
}
pub struct Request {
    pub generation: u64,
    pub center: (i32, i32),
    pub view: i32,
    pub budget: usize,
    pub edits: HashMap<IVec3, Block>,
    pub time: f32,
    pub players: Vec<ActivePlayer>,
}
pub struct WorldReady {
    pub request: Request,
    pub world: World,
    pub cache: CacheStats,
    pub gpu: WorldGpu,
    pub frame: MeshFrame,
    pub ready: Ready,
}
pub enum Completed {
    World(Box<WorldReady>),
    Animation(Option<Box<(MeshFrame, Ready)>>),
    Error(String),
}
enum Job {
    World(Request),
    Animate {
        generation: u64,
        time: f32,
        rebuild: bool,
        players: Vec<ActivePlayer>,
    },
    Recycle(Box<MeshFrame>),
    RetireWorld(Box<WorldGpu>),
    RetireCpu(World),
    Stop,
}
pub struct Streamer {
    tx: mpsc::Sender<Job>,
    rx: mpsc::Receiver<Completed>,
    thread: Option<thread::JoinHandle<()>>,
    pub animation_pending: bool,
}
impl Streamer {
    pub fn new(ctx: Arc<ContextVk>, world: World) -> Result<Self> {
        let (tx, jobs) = mpsc::channel();
        let (done, rx) = mpsc::sync_channel(2);
        let handle = thread::Builder::new()
            .name("world-upload-as".into())
            .spawn(move || {
                let mut worker = match Worker::new(ctx, world) {
                    Ok(w) => w,
                    Err(e) => {
                        let _ = done.send(Completed::Error(e.to_string()));
                        return;
                    }
                };
                while let Ok(job) = jobs.recv() {
                    let result = match job {
                        Job::World(r) => {
                            worker.world(r).map(|v| Some(Completed::World(Box::new(v))))
                        }
                        Job::Animate {
                            generation,
                            time,
                            rebuild,
                            players,
                        } => worker
                            .animate(generation, time, rebuild, &players)
                            .map(|v| Some(Completed::Animation(v.map(Box::new)))),
                        Job::Recycle(frame) => {
                            if frame.generation == worker.generation {
                                worker.pool.push_back(*frame);
                            }
                            Ok(None)
                        }
                        Job::RetireCpu(world) => {
                            drop(world);
                            Ok(None)
                        }
                        Job::RetireWorld(gpu) => {
                            if worker
                                .world_pool
                                .as_ref()
                                .is_none_or(|old| old.voxels.size <= gpu.voxels.size)
                            {
                                worker.world_pool = Some(*gpu);
                            }
                            Ok(None)
                        }
                        Job::Stop => break,
                    };
                    match result {
                        Ok(Some(v)) => {
                            if done.send(v).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            if done.send(Completed::Error(format!("{e:#}"))).is_err() {
                                break;
                            }
                        }
                    }
                }
            })?;
        Ok(Self {
            tx,
            rx,
            thread: Some(handle),
            animation_pending: false,
        })
    }
    pub fn request(&self, r: Request) -> Result<()> {
        self.tx
            .send(Job::World(r))
            .map_err(|_| anyhow!("Streaming worker stopped"))
    }
    pub fn animate(&mut self, generation: u64, time: f32, rebuild: bool, players: Vec<ActivePlayer>) -> Result<()> {
        if self.animation_pending {
            return Ok(());
        }
        self.tx
            .send(Job::Animate {
                generation,
                time,
                rebuild,
                players,
            })
            .map_err(|_| anyhow!("Streaming worker stopped"))?;
        self.animation_pending = true;
        Ok(())
    }
    pub fn poll(&mut self) -> Option<Completed> {
        let result = match self.rx.try_recv() {
            Ok(result)=>result,
            Err(mpsc::TryRecvError::Empty)=>return None,
            Err(mpsc::TryRecvError::Disconnected)=>return Some(Completed::Error("Streaming worker stopped unexpectedly".into())),
        };
        if matches!(result, Completed::Animation(_)) {
            self.animation_pending = false;
        }
        Some(result)
    }
    pub fn recycle(&self, frame: MeshFrame) {
        let _ = self.tx.send(Job::Recycle(Box::new(frame)));
    }
    pub fn retire_cpu(&self, world: World) {
        let _ = self.tx.send(Job::RetireCpu(world));
    }
    pub fn retire_world(&self, gpu: WorldGpu) {
        let _ = self.tx.send(Job::RetireWorld(Box::new(gpu)));
    }
}
impl Drop for Streamer {
    fn drop(&mut self) {
        let _ = self.tx.send(Job::Stop); // Drain a bounded result channel so shutdown cannot block a sender.
        while self.thread.as_ref().is_some_and(|t| !t.is_finished()) {
            let _ = self.rx.recv_timeout(std::time::Duration::from_millis(10));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
struct Worker {
    ctx: Arc<ContextVk>,
    world: World,
    pool: VecDeque<MeshFrame>,
    template: MeshScene,
    entities: EntitySystem,
    started: Instant,
    world_pool: Option<WorldGpu>,
    staging: Option<Buffer>,
    metadata: Option<Buffer>,
    generation: u64,
    allocated: usize,
    commands: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    queries: vk::QueryPool,
    period: f32,
    bits: u32,
}
impl Worker {
    fn new(ctx: Arc<ContextVk>, world: World) -> Result<Self> {
        unsafe {
            let commands = ctx.device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(ctx.family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            let cmd = ctx.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(commands)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            let fence = ctx
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)?;
            let props = ctx.instance.get_physical_device_properties(ctx.physical);
            let bits = ctx
                .instance
                .get_physical_device_queue_family_properties(ctx.physical)[ctx.family as usize]
                .timestamp_valid_bits;
            let queries = if bits > 0 {
                ctx.device.create_query_pool(
                    &vk::QueryPoolCreateInfo::default()
                        .query_type(vk::QueryType::TIMESTAMP)
                        .query_count(3),
                    None,
                )?
            } else {
                vk::QueryPool::null()
            };
            let period = props.limits.timestamp_period;
            Ok(Self {
                queries,
                period,
                bits,
                ctx,
                world,
                pool: VecDeque::new(),
                template: MeshScene::empty(),
                entities: EntitySystem::default(),
                started: Instant::now(),
                world_pool: None,
                staging: None,
                metadata: None,
                generation: 0,
                allocated: 0,
                commands,
                cmd,
                fence,
            })
        }
    }
    fn begin(&self) -> Result<()> {
        unsafe {
            self.ctx.device.reset_fences(&[self.fence])?;
            self.ctx
                .device
                .reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty())?;
            self.ctx.device.begin_command_buffer(
                self.cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
        }
        Ok(())
    }
    fn finish(&self) -> Result<Ready> {
        unsafe {
            let semaphore = self
                .ctx
                .device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
            let ready = Ready {
                ctx: self.ctx.clone(),
                semaphore,
            };
            self.ctx.device.end_command_buffer(self.cmd)?;
            {
                let _guard = self.ctx.background_lock().lock().unwrap();
                self.ctx.device.queue_submit(
                    self.ctx.background_queue,
                    &[vk::SubmitInfo::default()
                        .command_buffers(&[self.cmd])
                        .signal_semaphores(&[semaphore])],
                    self.fence,
                )?;
            }
            // Only the worker waits. The render loop keeps the previous completed snapshot.
            self.ctx
                .device
                .wait_for_fences(&[self.fence], true, u64::MAX)?;
            Ok(ready)
        }
    }
    fn frame(&self, scene: MeshScene, time: f32) -> Result<MeshFrame> {
        let storage = vk::BufferUsageFlags::STORAGE_BUFFER;
        let mut objects = Buffer::new(
            self.ctx.clone(),
            (scene.objects.len() * 112).max(112),
            storage,
            true,
        )?;
        let mut triangles = Buffer::new(
            self.ctx.clone(),
            (scene.triangles.len() * 80).max(80),
            storage,
            true,
        )?;
        objects.write(bytemuck::cast_slice(&scene.objects));
        triangles.write(bytemuck::cast_slice(&scene.triangles));
        let rt = if self.ctx.ray_query {
            Some(RayTracing::new(self.ctx.clone(), &scene)?)
        } else {
            None
        };
        Ok(MeshFrame {
            objects,
            triangles,
            rt,
            scene,
            time,
            generation: self.generation,
            prepare_ms: 0.0,
            gpu_ms: [0.0; 2],
        })
    }
    fn stamp(&self, index: u32) {
        if self.queries == vk::QueryPool::null() {
            return;
        }
        let stage = if self.ctx.ray_query {
            vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR
        } else {
            vk::PipelineStageFlags::TOP_OF_PIPE
        };
        unsafe {
            if index == 0 {
                self.ctx
                    .device
                    .cmd_reset_query_pool(self.cmd, self.queries, 0, 3);
            }
            self.ctx
                .device
                .cmd_write_timestamp(self.cmd, stage, self.queries, index);
        }
    }
    fn build(&self, frame: &mut MeshFrame, rebuild: bool) -> Result<Ready> {
        frame.gpu_ms = [0.0; 2];
        if let Some(rt) = &mut frame.rt {
            rt.upload(&frame.scene);
            let indices: Vec<_> = frame
                .scene
                .objects
                .iter()
                .enumerate()
                .filter(|(_, o)| !rt.built || (o.info[2] == 0 && o.info[3]&crate::entities::INACTIVE==0))
                .map(|(i, _)| i)
                .collect();
            // Bound driver submission work too, not just GPU build duration.
            for batch in indices.chunks(AS_BUILD_BATCH) {
                self.begin()?;
                self.stamp(0);
                rt.record_blas_batch(self.cmd, &frame.scene, rebuild, batch);
                self.stamp(1);
                self.stamp(2);
                drop(self.finish()?);
                frame.gpu_ms[0] += self.timings()?[0];
            }
        }
        self.begin()?;
        self.stamp(0);
        self.stamp(1);
        if let Some(rt) = &mut frame.rt {
            rt.record_tlas(self.cmd, &frame.scene);
            rt.built = true;
        }
        self.stamp(2);
        let ready = self.finish()?;
        frame.gpu_ms[1] = self.timings()?[1];
        Ok(ready)
    }
    fn timings(&self) -> Result<[f32; 2]> {
        if self.queries == vk::QueryPool::null() || !self.ctx.ray_query {
            return Ok([0.0; 2]);
        }
        let mut times = [0u64; 3];
        unsafe {
            self.ctx.device.get_query_pool_results(
                self.queries,
                0,
                &mut times,
                vk::QueryResultFlags::TYPE_64,
            )?;
        }
        let mask = if self.bits == 64 {
            u64::MAX
        } else {
            (1u64 << self.bits) - 1
        };
        Ok(std::array::from_fn(|i| {
            ((times[i + 1].wrapping_sub(times[i])) & mask) as f32 * self.period / 1_000_000.0
        }))
    }
    fn world(&mut self, r: Request) -> Result<WorldReady> {
        let start = Instant::now();
        self.world.apply_edits(r.edits.clone());
        self.world.load_radius(r.center.0, r.center.1, r.view.min(NEAR_VIEW_CHUNKS) + 1);
        let cache = self.world.trim_cache(r.center, r.view.min(NEAR_VIEW_CHUNKS) + 1, r.budget);
        let (origin, blocks) = self
            .world
            .snapshot_radius(r.center.0, r.center.1, r.view.min(NEAR_VIEW_CHUNKS) + 1);
        let (sections, section_bounds) = spatial::occupancy_with_bounds(&blocks);
        let voxel_y = spatial::voxel_y_bounds(&blocks);
        let edge = window_edge(blocks.len()).unwrap();
        self.template = MeshScene::from_voxels(&blocks, origin);
        self.entities=EntitySystem::from_scene(&self.template);
        self.entities.update(&mut self.template,&r.players,r.time,self.started.elapsed().as_secs_f64());
        self.generation = r.generation;
        self.pool.clear();
        self.allocated = 1;
        let mut frame = self.frame(self.template.clone(), r.time)?;
        let voxel_bytes = blocks.len();
        let section_bytes = sections.len() * 16;
        if self.staging.as_ref().is_none_or(|b| b.size < voxel_bytes) {
            self.staging = Some(Buffer::new(
                self.ctx.clone(),
                voxel_bytes,
                vk::BufferUsageFlags::TRANSFER_SRC,
                true,
            )?);
        }
        if self
            .metadata
            .as_ref()
            .is_none_or(|b| b.size < section_bytes)
        {
            self.metadata = Some(Buffer::new(
                self.ctx.clone(),
                section_bytes,
                vk::BufferUsageFlags::TRANSFER_SRC,
                true,
            )?);
        }
        self.staging
            .as_mut()
            .unwrap()
            .write(bytemuck::cast_slice(&blocks));
        self.metadata
            .as_mut()
            .unwrap()
            .write(bytemuck::cast_slice(&sections));
        let storage = vk::BufferUsageFlags::STORAGE_BUFFER;
        let mut gpu = if self.world_pool.as_ref().is_some_and(|old| {
            old.voxels.size >= voxel_bytes
                && old.occupancy.size >= section_bytes
                && old.mask.size >= sections.len() * 4
        }) {
            self.world_pool.take().unwrap()
        } else {
            WorldGpu {
                voxels: Buffer::new(
                    self.ctx.clone(),
                    voxel_bytes,
                    storage | vk::BufferUsageFlags::TRANSFER_DST,
                    false,
                )?,
                occupancy: Buffer::new(
                    self.ctx.clone(),
                    section_bytes,
                    storage | vk::BufferUsageFlags::TRANSFER_DST,
                    false,
                )?,
                mask: Buffer::new(self.ctx.clone(), sections.len() * 4, storage, true)?,
                sections: Vec::new(),
                section_bounds: Vec::new(),
                voxel_y,
                edge,
                origin,
            }
        };
        gpu.edge = edge;
        gpu.origin = origin;
        gpu.sections = sections;
        gpu.section_bounds = section_bounds;
        gpu.voxel_y = voxel_y;
        let staging = self.staging.as_ref().unwrap();
        let metadata = self.metadata.as_ref().unwrap();
        self.begin()?;
        unsafe {
            let d = &self.ctx.device;
            d.cmd_copy_buffer(
                self.cmd,
                staging.handle,
                gpu.voxels.handle,
                &[vk::BufferCopy::default().size(voxel_bytes as u64)],
            );
            d.cmd_copy_buffer(
                self.cmd,
                metadata.handle,
                gpu.occupancy.handle,
                &[vk::BufferCopy::default().size(section_bytes as u64)],
            );
            let barrier = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ);
            d.cmd_pipeline_barrier(
                self.cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[barrier],
                &[],
                &[],
            );
        }
        drop(self.finish()?);
        drop(blocks);
        let ready = self.build(&mut frame, false)?;
        frame.prepare_ms = start.elapsed().as_secs_f32() * 1000.0;
        Ok(WorldReady {
            request: r,
            world: self.world.clone(),
            cache,
            gpu,
            frame,
            ready,
        })
    }
    fn animate(
        &mut self,
        generation: u64,
        time: f32,
        rebuild: bool,
        players: &[ActivePlayer],
    ) -> Result<Option<(MeshFrame, Ready)>> {
        if generation != self.generation {
            return Ok(None);
        }
        let start = Instant::now();
        let mut frame = if self.allocated < AS_RING_SLOTS {
            self.allocated += 1;
            self.frame(self.template.clone(), time)?
        } else if let Some(frame) = self.pool.pop_front() {
            frame
        } else {
            return Ok(None);
        };
        self.entities.update(&mut frame.scene,players,time,self.started.elapsed().as_secs_f64());
        frame
            .objects
            .write(bytemuck::cast_slice(&frame.scene.objects));
        frame
            .triangles
            .write(bytemuck::cast_slice(&frame.scene.triangles));
        frame.time = time;
        let ready = self.build(&mut frame, rebuild)?;
        frame.prepare_ms = start.elapsed().as_secs_f32() * 1000.0;
        Ok(Some((frame, ready)))
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        unsafe {
            if self.queries != vk::QueryPool::null() {
                self.ctx.device.destroy_query_pool(self.queries, None);
            }
            self.ctx.device.destroy_fence(self.fence, None);
            self.ctx.device.destroy_command_pool(self.commands, None);
        }
    }
}
