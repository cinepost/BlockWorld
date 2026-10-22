//! Direct ash Vulkan bindings: buffers, descriptors, pipelines, barriers and fences.
//! Render frames have explicit fences; streaming/AS snapshots have independent ownership.
use crate::meshes::{self, MeshHit, MeshScene, MATERIALS, MAX_OBJECTS, MAX_TRIANGLES};
use crate::presentation::Presentation;
use crate::rt::RayTracing;
use crate::spatial::{self, Frustum, Section};
use crate::streaming::{MeshFrame, Ready, WorldGpu};
use crate::waves::WaveTexture;
use crate::{atlas_gpu::AtlasTexture, resource_pack::{Pack, DESCRIPTORS, METADATA_WORDS, RECORD_WORDS}};
use crate::{config::*, player::Player};
use anyhow::{anyhow, bail, Context, Result};
use ash::{vk, Entry};
use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Mat4, Vec3};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::time::Instant;
use std::{
    ffi::{CStr, CString},
    io::Cursor,
    sync::{Arc, Mutex},
};
use winit::window::Window;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    eye: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    origin: [i32; 4],
    dims: [u32; 4],
    screen: [u32; 4],
    sun: [f32; 4],
    effects: [f32; 4],
    flags: [u32; 4],
    meshes: [u32; 4],
    water: [f32; 4],
    optics: [f32; 4],
    scene_min: [f32; 4],
    scene_max: [f32; 4],
    acceleration: [u32; 4],
    light_right: [f32; 4],
    light_up: [f32; 4],
    cascade_center: [f32; 4],
    shadow_controls: [f32; 4],
    soft: [f32; 4],
    history: [u32; 4],
    previous_vp: [[f32; 4]; 4],
    sky_light: [f32; 4],
    sky_build: [f32; 4],
    sky_state: [u32; 4],
    display: [f32; 4],
    lod: [f32; 4],
    gi: [f32; 4],
    wave_detail:[f32;4],
    caustics:[f32;4],
    caustic_map:[f32;4],
}
/// Matches WGSL Hit: normal xyz + depth, UV, material ID, valid flag.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Visibility {
    pub normal_depth: [f32; 4],
    pub uv: [f32; 2],
    pub material: u32,
    pub valid: u32,
    pub water_normal_depth: [f32; 4],
    pub water: [f32; 4],
}
const _: () = assert!(std::mem::size_of::<Visibility>() == 64);
const _: () = assert!(std::mem::size_of::<Params>() == 560);
pub(crate) struct ContextVk {
    pub(crate) family: u32,
    pub(crate) background_queue: vk::Queue,
    pub(crate) queue_lock: Mutex<()>,
    background_lock: Mutex<()>,
    pub(crate) independent_queue: bool,
    _entry: Entry,
    pub(crate) instance: ash::Instance,
    pub(crate) device: ash::Device,
    pub(crate) physical: vk::PhysicalDevice,
    pub(crate) ray_query: bool,
    pub(crate) anisotropy: bool,
    pub(crate) bindless: bool,
    pub(crate) surface: Option<(ash::khr::surface::Instance, vk::SurfaceKHR)>,
}
impl ContextVk {
    pub(crate) fn background_lock(&self) -> &Mutex<()> {
        if self.independent_queue {
            &self.background_lock
        } else {
            &self.queue_lock
        }
    }
    pub(crate) fn wait_idle(&self) -> Result<()> {
        let _render = self.queue_lock.lock().unwrap();
        let _background = self.background_lock.lock().unwrap();
        unsafe {
            self.device.device_wait_idle()?;
        }
        Ok(())
    }
}
impl Drop for ContextVk {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_device(None);
            if let Some((loader, surface)) = &self.surface {
                loader.destroy_surface(*surface, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}
pub(crate) struct Buffer {
    ctx: Arc<ContextVk>,
    pub(crate) handle: vk::Buffer,
    memory: vk::DeviceMemory,
    ptr: *mut u8,
    pub(crate) size: usize,
}
// SAFETY: moving transfers exclusive ownership of the mapped allocation. Workers
// never share mapped pointers; channels and GPU completion transfer ownership.
unsafe impl Send for Buffer {}
impl Buffer {
    pub(crate) fn new(
        ctx: Arc<ContextVk>,
        size: usize,
        usage: vk::BufferUsageFlags,
        host: bool,
    ) -> Result<Self> {
        let d = &ctx.device;
        // SAFETY: valid device; all created objects below are cleaned on each error path.
        unsafe {
            let handle = d.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size as u64)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )?;
            let req = d.get_buffer_memory_requirements(handle);
            let props = ctx
                .instance
                .get_physical_device_memory_properties(ctx.physical);
            let flags = if host {
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT
            } else {
                vk::MemoryPropertyFlags::DEVICE_LOCAL
            };
            let ty = (0..props.memory_type_count).find(|&i| {
                req.memory_type_bits & (1 << i) != 0
                    && props.memory_types[i as usize]
                        .property_flags
                        .contains(flags)
            });
            let Some(ty) = ty else {
                d.destroy_buffer(handle, None);
                bail!("No compatible Vulkan memory type with flags {flags:?}");
            };
            let mut address_flags = vk::MemoryAllocateFlagsInfo::default()
                .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
            let mut allocate = vk::MemoryAllocateInfo::default()
                .allocation_size(req.size)
                .memory_type_index(ty);
            if usage.contains(vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS) {
                allocate = allocate.push_next(&mut address_flags);
            }
            let memory = match d.allocate_memory(&allocate, None) {
                Ok(v) => v,
                Err(e) => {
                    d.destroy_buffer(handle, None);
                    return Err(e.into());
                }
            };
            if let Err(e) = d.bind_buffer_memory(handle, memory, 0) {
                d.destroy_buffer(handle, None);
                d.free_memory(memory, None);
                return Err(e.into());
            }
            let ptr = if !host {
                std::ptr::null_mut()
            } else {
                match d.map_memory(memory, 0, req.size, vk::MemoryMapFlags::empty()) {
                    Ok(p) => p.cast(),
                    Err(e) => {
                        d.destroy_buffer(handle, None);
                        d.free_memory(memory, None);
                        return Err(e.into());
                    }
                }
            };
            Ok(Self {
                ctx,
                handle,
                memory,
                ptr,
                size,
            })
        }
    }
    pub(crate) fn write(&mut self, bytes: &[u8]) {
        assert!(!self.ptr.is_null() && bytes.len() <= self.size);
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr, bytes.len());
        }
    }
    pub(crate) fn address(&self) -> u64 {
        unsafe {
            self.ctx.device.get_buffer_device_address(
                &vk::BufferDeviceAddressInfo::default().buffer(self.handle),
            )
        }
    }
    pub(crate) fn info(&self) -> vk::DescriptorBufferInfo {
        vk::DescriptorBufferInfo::default()
            .buffer(self.handle)
            .offset(0)
            .range(self.size as u64)
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            let d = &self.ctx.device;
            if !self.ptr.is_null() {
                d.unmap_memory(self.memory);
            }
            d.destroy_buffer(self.handle, None);
            d.free_memory(self.memory, None);
        }
    }
}
#[derive(Clone, Copy)]
pub struct RenderSettings {
    pub view_chunks: i32,
    pub lod_pixels: f32,
    pub lod_merging: bool,
    pub hierarchy: bool,
    pub frustum: bool,
    pub ssao: bool,
    pub vxgi:bool,
    pub gi_range:f32,
    pub gi_angle:f32,
    pub gi_temporal:bool,
    pub depth_pyramid: bool,
    pub csm: bool,
    pub far_shadows: bool,
    pub shadow_ray_distance: f32,
    pub csm_resolution: u32,
    pub texture_linear: bool,
    pub shadow_type: usize,
    pub soft_shadows: bool,
    pub disk_radius: f32,
    pub penumbra_samples: i32,
    pub atmosphere: bool,
    pub day_cycle: bool,
    pub day_seconds: f32,
    pub water: bool,
    pub ssr: bool,
    pub waves: bool,
    pub wave_strength: f32,
    pub stochastic_waves:bool,
    pub multiscale_waves:bool,
    pub wave_warp:f32,
    pub wave_distance:f32,
    pub caustics:bool,
    pub caustic_strength:f32,
    pub caustic_near:bool,
    pub caustic_adaptive:bool,
    pub caustic_shallow:bool,
    pub caustic_distance:f32,
    pub shadows: bool,
    pub ao_strength: f32,
    pub ao_radius: f32,
    pub tone_mapping: bool,
    pub exposure_ev: f32,
    pub saturation: f32,
    pub ground_bounce: bool,
    pub water_density: f32,
    pub underwater_density: f32,
    pub water_ior: f32,
    pub sun_azimuth: f32,
    pub sun_elevation: f32,
    pub sun_intensity: f32,
    pub meshes: bool,
    pub fish: bool,
    pub foliage: bool,
    pub alpha_test: bool,
    pub hardware: bool,
    pub animate: bool,
    pub rebuild_blas: bool,
}
impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            view_chunks: DEFAULT_VIEW_CHUNKS,
            lod_pixels: 1.0,
            lod_merging: true,
            hierarchy: true,
            frustum: true,
            ssao: true,
            vxgi:false,
            gi_range:96.0,
            gi_angle:30.0,
            gi_temporal:true,
            depth_pyramid: true,
            csm: true,
            far_shadows: true,
            shadow_ray_distance: 48.0,
            csm_resolution: 512,
            texture_linear: true,
            shadow_type: 1,
            soft_shadows: true,
            disk_radius: 0.266,
            penumbra_samples: 3,
            atmosphere: true,
            day_cycle: false,
            day_seconds: 600.0,
            water: true,
            ssr: true,
            waves: true,
            wave_strength: 0.22,
            stochastic_waves:true,
            multiscale_waves:true,
            wave_warp:1.5,
            wave_distance:128.0,
            caustics:true,
            caustic_strength:1.0,
            caustic_near:true,
            caustic_adaptive:true,
            caustic_shallow:true,
            caustic_distance:64.0,
            shadows: true,
            ao_strength: 1.8,
            ao_radius: 1.0,
            tone_mapping: true,
            exposure_ev: 0.0,
            saturation: 1.15,
            ground_bounce: true,
            water_density: 1.0,
            underwater_density: 0.35,
            water_ior: 1.333,
            sun_azimuth: 35.0,
            sun_elevation: 55.0,
            sun_intensity: 1.1,
            meshes: true,
            fish: true,
            foliage: true,
            alpha_test: true,
            hardware: true,
            animate: true,
            rebuild_blas: false,
        }
    }
}
#[derive(Clone, Copy, Default)]
pub struct MeshTimings {
    pub animation_ms: f32,
    pub upload_ms: f32,
    pub setup_ms: f32,
    pub gpu: [f32; 4],
    pub initial_build_ms: f32,
}
pub struct Renderer {
    ctx: Arc<ContextVk>,
    queue: vk::Queue,
    pool: vk::CommandPool,
    command: vk::CommandBuffer,
    descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    layout: vk::PipelineLayout,
    pipelines: Vec<vk::Pipeline>,
    fence: vk::Fence,
    buffers: Vec<Buffer>,
    pub(crate) lod: Option<crate::lod::LodGpu>,
    wave_texture: Option<WaveTexture>,
    atlas_gpu: Option<AtlasTexture>,
    pub resource_pack: Option<Pack>,
    texture_metadata: Vec<u32>,
    pub water_time: Option<f32>,
    pub water_ms: f32,
    pub acceleration_ms: [f32; 2],
    pub sky_ms: f32,
    pub caustic_ms: f32,
    pub gi_ms:f32,
    pub denoise_ms: f32,
    pub cascade_updates: [u64; 2],
    pub far_depth_updates: [u64; 2],
    pub device_name: String,
    pub presentation: Option<Presentation>,
    headless: bool,
    queries: vk::QueryPool,
    timestamp_bits: u32,
    timestamp_period: f32,
    pub gpu_ms: Option<[f32; 4]>,
    pub settings: RenderSettings,
    pub reference_validation: bool,
    previous_vp: Mat4,
    history_key: Vec<u32>,
    clear_history: bool,
    pub sky_cache: crate::sky::SkyCache,
    shadow_cache_key: Vec<u32>,
    shadow_mesh_bounds: Vec<[Vec3; 2]>,
    world_revision: u32,
    temporal_revision: u32,
    shadow_mesh_time: f32,
    sections: Vec<Section>,
    section_bounds: Vec<spatial::SectionBounds>,
    voxel_y: [u32; 2],
    pub sky_only: bool,
    pub mesh_epoch: u32,
    pub sparse_mesh_validation: bool,
    clear_mesh_epochs: bool,
    pub section_counts: [usize; 2],
    pub chunk_counts: [usize; 2],
    pub scene: MeshScene,
    pub active_players: Vec<crate::entities::ActivePlayer>,
    entities: crate::entities::EntitySystem,
    pub animation_time: Option<f32>,
    rt: Option<RayTracing>,
    pub mesh_timings: MeshTimings,
    started: Instant,
    mesh_voxels: Vec<u8>,
    scene_origin: Option<glam::IVec3>,
    fixture_scene: bool,
    entity_snapshot: bool,
    pub async_generation: Option<u64>,
    pub snapshot_time: f32,
    pub background_ms: f32,
    pub cpu_stage_max: [f32; 4],
    ready_waits: Vec<Ready>,
    pub world_edge: usize,
    pub width: u32,
    pub height: u32,
}
impl Renderer {
    pub fn new(width: u32, height: u32, window: Option<Arc<Window>>) -> Result<Self> {
        let ctx = ContextVk::create(window.clone())?;
        let mut renderer = Self::from_context(width, height, ctx, window.is_none())?;
        if let Some(w) = window {
            renderer.presentation = Some(Presentation::new(renderer.ctx.clone(), renderer.queue, renderer.pool, w)?);
        }
        Ok(renderer)
    }
    pub(crate) fn from_context(width: u32, height: u32, ctx: Arc<ContextVk>, headless: bool) -> Result<Self> {
        let family = ctx.family;
        let queue = unsafe { ctx.device.get_device_queue(family, 0) };
        let properties = unsafe {ctx.instance.get_physical_device_properties(ctx.physical)};
        let device_name = unsafe {CStr::from_ptr(properties.device_name.as_ptr())}.to_string_lossy().into_owned();
        let mut r = Self {
            ctx,
            queue,
            pool: vk::CommandPool::null(),
            command: vk::CommandBuffer::null(),
            descriptor_layout: vk::DescriptorSetLayout::null(),
            descriptor_pool: vk::DescriptorPool::null(),
            set: vk::DescriptorSet::null(),
            layout: vk::PipelineLayout::null(),
            pipelines: vec![],
            fence: vk::Fence::null(),
            buffers: vec![],
            lod: None,
            wave_texture: None,
            atlas_gpu: None,
            resource_pack: None,
            texture_metadata: Vec::new(),
            water_time: None,
            water_ms: 0.0,
            acceleration_ms: [0.0; 2],
            sky_ms: 0.0,
            caustic_ms: 0.0,
            gi_ms:0.0,
            denoise_ms: 0.0,
            cascade_updates: [0; 2],
            far_depth_updates: [0; 2],
            device_name,
            presentation: None,
            headless,
            queries: vk::QueryPool::null(),
            timestamp_bits: 0,
            timestamp_period: 0.0,
            gpu_ms: None,
            settings: RenderSettings::default(),
            reference_validation: false,
            previous_vp: Mat4::IDENTITY,
            history_key: Vec::new(),
            clear_history: true,
            sky_cache: crate::sky::SkyCache::default(),
            shadow_cache_key: Vec::new(),
            shadow_mesh_bounds: Vec::new(),
            world_revision: 0,
            temporal_revision: 0,
            shadow_mesh_time: f32::NEG_INFINITY,
            sections: vec![Section::default(); GPU_SECTIONS],
            section_bounds: vec![spatial::SectionBounds::default(); GPU_SECTIONS],
            voxel_y: [0, HEIGHT as u32],
            sky_only: false,
            mesh_epoch: 0,
            sparse_mesh_validation: false,
            clear_mesh_epochs: true,
            world_edge: GPU_EDGE,
            async_generation: None,
            snapshot_time: 0.0,
            background_ms: 0.0,
            cpu_stage_max: [0.0; 4],
            ready_waits: Vec::new(),
            section_counts: [0; 2],
            chunk_counts: [0; 2],
            scene: MeshScene::empty(),
            active_players: vec![],
            entities: crate::entities::EntitySystem::default(),
            animation_time: None,
            rt: None,
            mesh_timings: MeshTimings::default(),
            started: Instant::now(),
            mesh_voxels: vec![],
            scene_origin: None,
            fixture_scene: false,
            entity_snapshot: false,
            width,
            height,
        };
        r.init(family)?;
        Ok(r)
    }
    fn init(&mut self, family: u32) -> Result<()> {
        let properties = unsafe {
            self.ctx
                .instance
                .get_physical_device_properties(self.ctx.physical)
        };
        self.timestamp_period = properties.limits.timestamp_period;
        self.timestamp_bits = unsafe {
            self.ctx
                .instance
                .get_physical_device_queue_family_properties(self.ctx.physical)
        }[family as usize]
            .timestamp_valid_bits;
        if self.timestamp_bits > 0 {
            self.queries = unsafe {
                self.ctx.device.create_query_pool(
                    &vk::QueryPoolCreateInfo::default()
                        .query_type(vk::QueryType::TIMESTAMP)
                        .query_count(22),
                    None,
                )
            }?;
        }
        let pixels = self.width as usize * self.height as usize;
        for (size, usage, host) in [
            (
                GPU_VOXELS,
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
                false,
            ),
            (
                pixels * std::mem::size_of::<Visibility>(),
                vk::BufferUsageFlags::STORAGE_BUFFER,
                self.headless,
            ),
            (
                pixels * 4,
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
                self.headless,
            ),
            (
                METADATA_WORDS * 4,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
            ),
            (
                std::mem::size_of::<Params>(),
                vk::BufferUsageFlags::UNIFORM_BUFFER,
                true,
            ),
            (GPU_VOXELS, vk::BufferUsageFlags::TRANSFER_SRC, true),
            (
                GPU_SECTIONS * 16,
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
                false,
            ),
            (GPU_SECTIONS * 16, vk::BufferUsageFlags::TRANSFER_SRC, true),
            (GPU_SECTIONS * 4, vk::BufferUsageFlags::STORAGE_BUFFER, true),
            (
                pixels * 4,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                self.headless,
            ),
            (
                MAX_OBJECTS * 112,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
            ),
            (
                MAX_TRIANGLES * 80,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
            ),
            (
                pixels * 48,
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
                self.headless,
            ),
            (
                pixels * 8,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                self.headless,
            ),
            (MATERIALS * 16, vk::BufferUsageFlags::STORAGE_BUFFER, true),
            (
                pixels * 16,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                self.headless,
            ),
        ] {
            self.buffers
                .push(Buffer::new(self.ctx.clone(), size, usage, host)?);
        }
        self.buffers.push(Buffer::new(
            self.ctx.clone(),
            pixels * 8,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            false,
        )?);
        self.buffers.push(Buffer::new(
            self.ctx.clone(),
            pyramid_bytes(self.width, self.height),
            vk::BufferUsageFlags::STORAGE_BUFFER,
            self.headless,
        )?);
        self.buffers.push(Buffer::new(
            self.ctx.clone(),
            4 * 128 * 128 * 2 * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            false,
        )?);
        self.buffers.push(Buffer::new(
            self.ctx.clone(),
            4 * 512 * 512 * 2 * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            false,
        )?);
        self.buffers.push(Buffer::new(
            self.ctx.clone(),
            pixels * 64,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            self.headless,
        )?);
        self.buffers.push(Buffer::new(
            self.ctx.clone(),
            crate::sky::CACHE_BYTES,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            self.headless,
        )?);
        self.buffers.push(Buffer::new(
            self.ctx.clone(),
            pixels * 16,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            self.headless,
        )?);
        self.buffers.push(Buffer::new(self.ctx.clone(),16*65,vk::BufferUsageFlags::STORAGE_BUFFER,true)?);
        self.buffers.push(Buffer::new(self.ctx.clone(),16,vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,self.headless)?);
        self.buffers.push(Buffer::new(self.ctx.clone(),256*256*11*4,vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,self.headless)?);
        let mut pack=Pack::generated(&textures())?;
        self.texture_metadata=pack.metadata(0.0,self.settings.texture_linear);
        self.buffers[3].write(bytemuck::cast_slice(&self.texture_metadata));
        self.buffers[14].write(bytemuck::cast_slice(&meshes::materials()));
        let d = &self.ctx.device;
        // SAFETY: all resources owned by self; Drop handles partial initialization too.
        unsafe {
            self.pool = d.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            self.command = d.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            self.fence = d.create_fence(&vk::FenceCreateInfo::default(), None)?;
            self.wave_texture = Some(WaveTexture::new(self.ctx.clone(), self.queue, self.pool)?);
            self.atlas_gpu=Some(AtlasTexture::new(self.ctx.clone(),self.queue,self.pool,pack.width,pack.height,&pack.rgba)?);
            pack.rgba.clear();pack.rgba.shrink_to_fit();self.resource_pack=Some(pack);
            self.lod=Some(crate::lod::LodGpu::new(self.ctx.clone())?);
            let bindings: Vec<_> = (0..30)
                .filter(|&i| i != 12 || self.ctx.ray_query)
                .map(|i| {
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(i)
                        .descriptor_type(if i == 15 || i == 24 {
                            vk::DescriptorType::SAMPLED_IMAGE
                        } else if i == 16 || i == 25 {
                            vk::DescriptorType::SAMPLER
                        } else if i == 12 {
                            vk::DescriptorType::ACCELERATION_STRUCTURE_KHR
                        } else if i == 4 {
                            vk::DescriptorType::UNIFORM_BUFFER
                        } else {
                            vk::DescriptorType::STORAGE_BUFFER
                        })
                        .descriptor_count(if i==24 && self.ctx.bindless {DESCRIPTORS} else {1})
                        .stage_flags(vk::ShaderStageFlags::COMPUTE)
                })
                .collect();
            self.descriptor_layout = d.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            let mut sizes = vec![
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(24),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(1),
            ];
            sizes.push(
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1+if self.ctx.bindless{DESCRIPTORS}else{1}),
            );
            sizes.push(
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLER)
                    .descriptor_count(2),
            );
            if self.ctx.ray_query {
                sizes.push(
                    vk::DescriptorPoolSize::default()
                        .ty(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                        .descriptor_count(1),
                );
            }
            self.descriptor_pool = d.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&sizes),
                None,
            )?;
            let layouts = [self.descriptor_layout];
            self.set = d.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool)
                    .set_layouts(&layouts),
            )?[0];
            let infos: Vec<_> = [0, 1, 2, 3, 4, 6, 8, 9, 10, 11, 12, 13, 14, 15]
                .iter()
                .map(|&i| [self.buffers[i].info()])
                .collect();
            let writes: Vec<_> = infos
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.set)
                        .dst_binding(if i >= 12 { i as u32 + 1 } else { i as u32 })
                        .descriptor_type(if i == 4 {
                            vk::DescriptorType::UNIFORM_BUFFER
                        } else {
                            vk::DescriptorType::STORAGE_BUFFER
                        })
                        .buffer_info(b)
                })
                .collect();
            d.update_descriptor_sets(&writes, &[]);
            let caustic_info=[self.buffers[25].info()];
            d.update_descriptor_sets(&[vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(29).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&caustic_info)],&[]);
            let gi_info=[self.buffers[24].info()];
            d.update_descriptor_sets(&[vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(28).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&gi_info)],&[]);
            let players_info=[self.buffers[23].info()];
            d.update_descriptor_sets(&[vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(27).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&players_info)],&[]);
            let epoch_info = [self.buffers[16].info()];
            d.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(17)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&epoch_info)],
                &[],
            );
            for binding in [18u32, 19, 20, 21, 22, 23] {
                let info = [self.buffers[binding as usize - 1].info()];
                d.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(self.set)
                        .dst_binding(binding)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .buffer_info(&info)],
                    &[],
                );
            }
            let lod_info=[self.lod.as_ref().unwrap().buffer.info()];
            d.update_descriptor_sets(&[vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(26).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&lod_info)],&[]);
            self.update_atlas_descriptors();
            let wave = self.wave_texture.as_ref().unwrap();
            let image = [vk::DescriptorImageInfo::default()
                .image_view(wave.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let sampler = [vk::DescriptorImageInfo::default().sampler(wave.sampler)];
            d.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.set)
                        .dst_binding(15)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&image),
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.set)
                        .dst_binding(16)
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(&sampler),
                ],
                &[],
            );
            self.layout = d.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&layouts)
                    .push_constant_ranges(&[vk::PushConstantRange::default()
                        .stage_flags(vk::ShaderStageFlags::COMPUTE)
                        .offset(0)
                        .size(16)]),
                None,
            )?;
        }
        macro_rules! shader {($name:literal)=>{if self.ctx.bindless {
            include_bytes!(concat!(env!("OUT_DIR"),"/",$name,".spv")).as_slice()
        } else {include_bytes!(concat!(env!("OUT_DIR"),"/",$name,"_single.spv")).as_slice()}};}
        let mut shaders = vec![
            shader!("visibility"),
            shader!("ao"),
            shader!("shade"),
            shader!("mesh_visibility"),
            shader!("mesh_shadow"),
        ];
        if self.ctx.ray_query {
            shaders.push(shader!("mesh_query"));
            shaders.push(
                shader!("mesh_query_shadow"),
            );
        }
        shaders.push(shader!("mesh_resolve"));
        shaders.push(shader!("water"));
        shaders.push(shader!("sky"));
        shaders.push(shader!("depth_pyramid"));
        shaders.push(shader!("csm"));
        shaders.push(shader!("csm_depth"));
        if self.ctx.ray_query {
            shaders
                .push(shader!("mesh_query_csm"));
        }
        shaders.push(shader!("shadow_temporal"));
        shaders.push(shader!("shadow_filter"));
        shaders.push(shader!("atmosphere"));
        shaders.push(shader!("sky_importance"));
        shaders.push(shader!("vxgi"));
        shaders.push(shader!("caustics"));
        for bytes in shaders {
            let code = ash::util::read_spv(&mut Cursor::new(bytes))?;
            unsafe {
                let module = d.create_shader_module(
                    &vk::ShaderModuleCreateInfo::default().code(&code),
                    None,
                )?;
                let stage = vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::COMPUTE)
                    .module(module)
                    .name(c"main");
                let result = d.create_compute_pipelines(
                    vk::PipelineCache::null(),
                    &[vk::ComputePipelineCreateInfo::default()
                        .stage(stage)
                        .layout(self.layout)],
                    None,
                );
                d.destroy_shader_module(module, None);
                match result {
                    Ok(p) => self.pipelines.extend(p),
                    Err((partial, e)) => {
                        for p in partial {
                            d.destroy_pipeline(p, None);
                        }
                        return Err(e.into());
                    }
                }
            }
        }
        Ok(())
    }
    /// Reallocate only resolution-dependent resources, keeping world/pipelines intact.
    pub fn resize_targets(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 || (width, height) == (self.width, self.height) {
            return Ok(());
        }
        let pixels = (width as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| anyhow!("Render extent overflow"))?;
        let bytes = pixels
            .checked_mul(std::mem::size_of::<Visibility>())
            .ok_or_else(|| anyhow!("Visibility buffer size overflow"))?;
        let limits = unsafe {
            self.ctx
                .instance
                .get_physical_device_properties(self.ctx.physical)
        }
        .limits;
        anyhow::ensure!(
            bytes.max(pixels.checked_mul(64).ok_or_else(|| anyhow!("Temporal buffer size overflow"))?) <= limits.max_storage_buffer_range as usize,
            "Render extent {width}x{height} exceeds device storage-buffer range"
        );
        anyhow::ensure!(
            width.div_ceil(8) <= limits.max_compute_work_group_count[0]
                && height.div_ceil(8) <= limits.max_compute_work_group_count[1],
            "Render extent exceeds device dispatch limits"
        );
        self.ctx.wait_idle()?;
        // Allocate before replacing, so allocation failure leaves the previous targets intact.
        let visibility = Buffer::new(
            self.ctx.clone(),
            bytes,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            self.headless,
        )?;
        let output = Buffer::new(
            self.ctx.clone(),
            pixels * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
            self.headless,
        )?;
        let ao = Buffer::new(
            self.ctx.clone(),
            pixels * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            self.headless,
        )?;
        let mesh_hits = Buffer::new(
            self.ctx.clone(),
            pixels * 48,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            self.headless,
        )?;
        let shadows = Buffer::new(
            self.ctx.clone(),
            pixels * 8,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            self.headless,
        )?;
        let scene = Buffer::new(
            self.ctx.clone(),
            pixels * 16,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            self.headless,
        )?;
        let pyramid = Buffer::new(
            self.ctx.clone(),
            pyramid_bytes(width, height),
            vk::BufferUsageFlags::STORAGE_BUFFER,
            self.headless,
        )?;
        let epochs = Buffer::new(
            self.ctx.clone(),
            pixels * 8,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            false,
        )?;
        let history = Buffer::new(
            self.ctx.clone(),
            pixels * 64,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            self.headless,
        )?;
        let shadow_raw = Buffer::new(
            self.ctx.clone(),
            pixels * 16,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            self.headless,
        )?;
        self.buffers[24]=Buffer::new(self.ctx.clone(),if self.settings.vxgi {width.div_ceil(2) as usize*height.div_ceil(2) as usize*64} else {16},vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,self.headless)?;
        self.buffers[20] = history;
        self.buffers[22] = shadow_raw;
        self.clear_history = true;
        self.history_key.clear();
        self.buffers[17] = pyramid;
        self.buffers[16] = epochs;
        self.buffers[15] = scene;
        self.buffers[12] = mesh_hits;
        self.clear_mesh_epochs = true;
        self.buffers[13] = shadows;
        self.buffers[1] = visibility;
        self.buffers[2] = output;
        self.buffers[9] = ao;
        let infos = [
            [self.buffers[1].info()],
            [self.buffers[2].info()],
            [self.buffers[9].info()],
            [self.buffers[12].info()],
            [self.buffers[13].info()],
            [self.buffers[15].info()],
            [self.buffers[16].info()],
            [self.buffers[17].info()],
            [self.buffers[20].info()],
            [self.buffers[22].info()],
            [self.buffers[24].info()],
        ];
        let bindings = [1, 2, 7, 10, 11, 14, 17, 18, 21, 23, 28];
        let writes: Vec<_> = infos
            .iter()
            .zip(bindings)
            .map(|(info, binding)| {
                vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(binding)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(info)
            })
            .collect();
        unsafe {
            self.ctx.device.update_descriptor_sets(&writes, &[]);
        }
        self.width = width;
        self.height = height;
        self.gpu_ms = None;
        println!("Render targets resized: {width}x{height}");
        Ok(())
    }
    pub fn has_independent_background_queue(&self) -> bool {
        self.ctx.independent_queue
    }
    pub(crate) fn stream_context(&mut self) -> Arc<ContextVk> {
        self.fixture_scene = true;
        self.entity_snapshot = true;
        self.mesh_voxels.clear();
        self.mesh_voxels.shrink_to_fit();
        self.async_generation = Some(0);
        self.ctx.clone()
    }
    pub fn animation_clock(&self) -> f32 {
        self.animation_time.unwrap_or_else(|| {
            if self.settings.animate {
                self.started.elapsed().as_secs_f32()
            } else {
                0.0
            }
        })
    }
    fn update_snapshot_descriptors(&self) {
        let infos = [
            [self.buffers[0].info()],
            [self.buffers[6].info()],
            [self.buffers[8].info()],
            [self.buffers[10].info()],
            [self.buffers[11].info()],
        ];
        let writes: Vec<_> = infos
            .iter()
            .zip([0, 5, 6, 8, 9])
            .map(|(info, binding)| {
                vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(binding)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(info)
            })
            .collect();
        unsafe {
            self.ctx.device.update_descriptor_sets(&writes, &[]);
            if let Some(rt) = &self.rt {
                let handles = [rt.handle()];
                let mut as_info = vk::WriteDescriptorSetAccelerationStructureKHR::default()
                    .acceleration_structures(&handles);
                self.ctx.device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(self.set)
                        .dst_binding(12)
                        .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                        .descriptor_count(1)
                        .push_next(&mut as_info)],
                    &[],
                );
            }
        }
    }
    pub(crate) fn adopt_mesh(&mut self, frame: MeshFrame, ready: Ready) -> MeshFrame {
        let old = MeshFrame {
            objects: std::mem::replace(&mut self.buffers[10], frame.objects),
            triangles: std::mem::replace(&mut self.buffers[11], frame.triangles),
            rt: std::mem::replace(&mut self.rt, frame.rt),
            scene: std::mem::replace(&mut self.scene, frame.scene),
            generation: self.async_generation.unwrap_or(0),
            time: self.snapshot_time,
            prepare_ms: 0.0,
            gpu_ms: [0.0; 2],
        };
        if self.async_generation == Some(0) {
            self.mesh_timings.initial_build_ms = frame.gpu_ms.iter().sum();
        }
        self.mesh_timings.gpu[0] = frame.gpu_ms[0];
        self.mesh_timings.gpu[1] = frame.gpu_ms[1];
        self.async_generation = Some(frame.generation);
        self.entity_snapshot=true;
        self.snapshot_time = frame.time;
        self.background_ms = frame.prepare_ms;
        self.ready_waits.push(ready);
        self.fixture_scene = true;
        self.update_snapshot_descriptors();
        old
    }
    pub(crate) fn adopt_world(&mut self, gpu: WorldGpu) -> WorldGpu {
        self.world_revision = self.world_revision.wrapping_add(1);
        let old = WorldGpu {
            voxels: std::mem::replace(&mut self.buffers[0], gpu.voxels),
            occupancy: std::mem::replace(&mut self.buffers[6], gpu.occupancy),
            mask: std::mem::replace(&mut self.buffers[8], gpu.mask),
            sections: std::mem::replace(&mut self.sections, gpu.sections),
            section_bounds: std::mem::replace(&mut self.section_bounds, gpu.section_bounds),
            voxel_y: std::mem::replace(&mut self.voxel_y, gpu.voxel_y),
            edge: self.world_edge,
            origin: self.scene_origin.unwrap_or(IVec3::ZERO),
        };
        self.world_edge = gpu.edge;
        self.scene_origin = Some(gpu.origin);
        self.update_snapshot_descriptors();
        old
    }
    pub fn max_view_chunks(&self) -> i32 {
        MAX_VIEW_CHUNKS
    }
    pub(crate) fn prepare_lods(&mut self,player:&Player,world:&crate::world::World)->Result<()> {
        if let Some(lod)=&mut self.lod {lod.gi=self.settings.vxgi;lod.prepare(player.eye(),self.settings.view_chunks,self.height,self.settings.lod_pixels,world)?;}
        Ok(())
    }
    fn resize_world(&mut self, edge: usize) -> Result<()> {
        if edge == self.world_edge {
            return Ok(());
        }
        let voxels = edge * edge * HEIGHT;
        let sections = (edge / EDGE).pow(2) * SUBCHUNKS;
        let limit = unsafe {
            self.ctx
                .instance
                .get_physical_device_properties(self.ctx.physical)
        }
        .limits
        .max_storage_buffer_range as usize;
        anyhow::ensure!(
            voxels <= limit,
            "World window exceeds device storage-buffer range"
        );
        self.ctx.wait_idle()?;
        let storage = vk::BufferUsageFlags::STORAGE_BUFFER;
        let specs = [
            (
                0,
                voxels,
                storage | vk::BufferUsageFlags::TRANSFER_DST,
                false,
            ),
            (5, voxels, vk::BufferUsageFlags::TRANSFER_SRC, true),
            (
                6,
                sections * 16,
                storage | vk::BufferUsageFlags::TRANSFER_DST,
                false,
            ),
            (7, sections * 16, vk::BufferUsageFlags::TRANSFER_SRC, true),
            (8, sections * 4, storage, true),
        ];
        let replacements: Result<Vec<_>> = specs
            .iter()
            .map(|&(i, n, u, h)| Ok((i, Buffer::new(self.ctx.clone(), n, u, h)?)))
            .collect();
        for (i, buffer) in replacements? {
            self.buffers[i] = buffer;
        }
        let infos = [
            [self.buffers[0].info()],
            [self.buffers[6].info()],
            [self.buffers[8].info()],
        ];
        let writes: Vec<_> = infos
            .iter()
            .zip([0, 5, 6])
            .map(|(info, binding)| {
                vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(binding)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(info)
            })
            .collect();
        unsafe {
            self.ctx.device.update_descriptor_sets(&writes, &[]);
        }
        self.world_edge = edge;
        Ok(())
    }
    pub fn upload(&mut self, blocks: &[u8]) -> Result<()> {
        self.temporal_revision = self.temporal_revision.wrapping_add(1);
        self.world_revision = self.world_revision.wrapping_add(1);
        let edge = window_edge(blocks.len()).ok_or_else(|| anyhow!("Invalid world window"))?;
        self.resize_world(edge)?;
        self.buffers[5].write(bytemuck::cast_slice(blocks));
        (self.sections, self.section_bounds) = spatial::occupancy_with_bounds(blocks);
        self.voxel_y = spatial::voxel_y_bounds(blocks);
        self.buffers[7].write(bytemuck::cast_slice(&self.sections));
        unsafe {
            let d = &self.ctx.device;
            d.reset_fences(&[self.fence])?;
            d.reset_command_buffer(self.command, vk::CommandBufferResetFlags::empty())?;
            d.begin_command_buffer(
                self.command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            d.cmd_copy_buffer(
                self.command,
                self.buffers[5].handle,
                self.buffers[0].handle,
                &[vk::BufferCopy::default().size(blocks.len() as u64)],
            );
            d.cmd_copy_buffer(
                self.command,
                self.buffers[7].handle,
                self.buffers[6].handle,
                &[vk::BufferCopy::default().size((self.sections.len() * 16) as u64)],
            );
            let barrier = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ);
            d.cmd_pipeline_barrier(
                self.command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[barrier],
                &[],
                &[],
            );
            d.end_command_buffer(self.command)?;
            {
                let _guard = self.ctx.queue_lock.lock().unwrap();
                d.queue_submit(
                    self.queue,
                    &[vk::SubmitInfo::default().command_buffers(&[self.command])],
                    self.fence,
                )?;
            }
            d.wait_for_fences(&[self.fence], true, u64::MAX)?;
        }
        self.mesh_voxels = blocks.to_vec();
        self.scene_origin = None;
        self.fixture_scene = false;
        Ok(())
    }
    pub fn ray_query_supported(&self) -> bool {
        self.ctx.ray_query
    }
    pub fn as_memory_bytes(&self) -> usize {
        self.rt.as_ref().map_or(0, |r| r.memory_bytes)
    }
    pub fn set_mesh_scene(&mut self, scene: MeshScene) -> Result<()> {
        self.temporal_revision = self.temporal_revision.wrapping_add(1);
        self.world_revision = self.world_revision.wrapping_add(1);
        let start = Instant::now();
        self.ctx.wait_idle()?;
        self.buffers[10].write(bytemuck::cast_slice(&scene.objects));
        self.buffers[11].write(bytemuck::cast_slice(&scene.triangles));
        if self.ctx.ray_query {
            self.rt = Some(RayTracing::new(self.ctx.clone(), &scene)?);
            let handles = [self.rt.as_ref().unwrap().handle()];
            let mut as_info = vk::WriteDescriptorSetAccelerationStructureKHR::default()
                .acceleration_structures(&handles);
            let write = vk::WriteDescriptorSet::default()
                .dst_set(self.set)
                .dst_binding(12)
                .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                .descriptor_count(1)
                .push_next(&mut as_info);
            unsafe {
                self.ctx.device.update_descriptor_sets(&[write], &[]);
            }
        }
        self.entity_snapshot=false;
        self.entities=crate::entities::EntitySystem::from_scene(&scene);
        self.scene = scene;
        self.fixture_scene = true;
        self.mesh_timings.setup_ms = start.elapsed().as_secs_f32() * 1000.0;
        Ok(())
    }
    /// Publish an externally managed NPC scene with live player-volume rejection.
    pub fn set_entity_mesh_scene(&mut self, scene:MeshScene)->Result<()> {
        self.set_mesh_scene(scene)?;
        self.entity_snapshot=true;
        Ok(())
    }
    pub fn mesh_hits(&self) -> &[MeshHit] {
        assert!(self.headless);
        unsafe {
            std::slice::from_raw_parts(
                self.buffers[12].ptr.cast(),
                self.width as usize * self.height as usize,
            )
        }
    }
    pub fn init_imgui(&mut self, imgui: &mut imgui::Context) -> Result<()> {
        if let Some(p) = &mut self.presentation {
            p.init_imgui(self.queue, self.pool, imgui)?;
        }
        Ok(())
    }
    pub fn render(&mut self, player: &Player, origin: IVec3, debug: u32) -> Result<&[u32]> {
        assert!(
            self.headless,
            "Use present() for GPU-only interactive rendering"
        );
        self.execute(player, origin, debug, None)?;
        unsafe {
            Ok(std::slice::from_raw_parts(
                self.buffers[2].ptr.cast(),
                self.width as usize * self.height as usize,
            ))
        }
    }
    pub(crate) fn warm_up(&mut self, player: &Player, origin: IVec3) -> Result<()> {
        self.execute(player, origin, 0, None)
    }
    pub fn present(
        &mut self,
        player: &Player,
        origin: IVec3,
        debug: u32,
        draw: &imgui::DrawData,
    ) -> Result<()> {
        self.execute(player, origin, debug, Some(draw))
    }
    fn execute(
        &mut self,
        player: &Player,
        origin: IVec3,
        debug: u32,
        draw: Option<&imgui::DrawData>,
    ) -> Result<()> {
        crate::entities::set_local(&mut self.active_players,player.feet);
        let started = Instant::now();
        let acquired = if let Some(p) = &mut self.presentation {
            match p.acquire()? {
                Some(i) => Some(i),
                None => return Ok(()),
            }
        } else {
            None
        };
        let acquired_ms = started.elapsed().as_secs_f32() * 1000.0;
        if let Some(p) = &self.presentation {
            let (width, height) = p.extent();
            self.resize_targets(width, height)?;
        }
        if !self.fixture_scene && self.scene_origin != Some(origin) {
            self.set_mesh_scene(MeshScene::from_voxels(&self.mesh_voxels, origin))?;
            self.fixture_scene = false;
            self.scene_origin = Some(origin);
        }
        let gi_bytes=self.width.div_ceil(2) as usize*self.height.div_ceil(2) as usize*64;
        if self.settings.vxgi && self.buffers[24].size!=gi_bytes {
            let replacement=Buffer::new(self.ctx.clone(),gi_bytes,vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,self.headless)?;
            self.ctx.wait_idle()?;self.buffers[24]=replacement;
            let info=[self.buffers[24].info()];
            unsafe {self.ctx.device.update_descriptor_sets(&[vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(28).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&info)],&[]);}
            self.clear_history=true;
        }
        let csm_resolution = self.settings.csm_resolution.clamp(128, 2048).next_power_of_two();
        let csm_bytes = 8 * csm_resolution as usize * csm_resolution as usize * 4;
        if self.buffers[19].size != csm_bytes {
            let replacement = Buffer::new(self.ctx.clone(), csm_bytes, vk::BufferUsageFlags::STORAGE_BUFFER, false)?;
            self.ctx.wait_idle()?;
            self.buffers[19] = replacement;
            let info = [self.buffers[19].info()];
            unsafe { self.ctx.device.update_descriptor_sets(&[vk::WriteDescriptorSet::default()
                .dst_set(self.set).dst_binding(20).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&info)], &[]); }
            self.shadow_cache_key.clear();
        }
        let animation = Instant::now();
        if self.settings.meshes && self.async_generation.is_none() {
            let time=self.animation_clock();
            if self.fixture_scene {self.scene.update(time);}
            else {
                crate::entities::set_local(&mut self.active_players,player.feet);
                self.entities.update(&mut self.scene,&self.active_players,time,self.started.elapsed().as_secs_f64());
            }
        }
        self.mesh_timings.animation_ms = if self.async_generation.is_some() {
            0.0
        } else {
            animation.elapsed().as_secs_f32() * 1000.0
        };
        let upload = Instant::now();
        if self.async_generation.is_none() {
            self.buffers[10].write(bytemuck::cast_slice(&self.scene.objects));
            self.buffers[11].write(bytemuck::cast_slice(&self.scene.triangles));
        }
        let hardware = self.settings.hardware && self.ctx.ray_query;
        let mesh_active = self.settings.meshes
            && !self.scene.objects.is_empty()
            && (self.settings.fish || self.settings.foliage);
        let as_active = hardware && mesh_active && self.async_generation.is_none();
        if let Some(rt) = &mut self.rt {
            if as_active || !rt.built {
                rt.upload(&self.scene);
            }
        }
        self.mesh_timings.upload_ms = if self.async_generation.is_some() {
            0.0
        } else {
            upload.elapsed().as_secs_f32() * 1000.0
        };
        let camera_frustum = Frustum::new(
            player,
            self.width as f32 / self.height as f32,
            self.settings.view_chunks as f32 * EDGE as f32,
        );
        let (mask, occupied, visible) = spatial::visible_sections(
            &self.sections,
            Some(&self.section_bounds),
            origin,
            &camera_frustum,
            self.settings.frustum,
        );
        self.section_counts = [occupied, visible];
        let columns = (self.world_edge / EDGE).pow(2);
        self.chunk_counts = [0; 2];
        for i in 0..columns {
            self.chunk_counts[0] +=
                (0..SUBCHUNKS).any(|y| self.sections[i + y * columns].any()) as usize;
            self.chunk_counts[1] += (0..SUBCHUNKS).any(|y| mask[i + y * columns] != 0) as usize;
        }
        self.buffers[8].write(bytemuck::cast_slice(&mask));
        let soft_enabled = self.settings.soft_shadows && !self.reference_validation;
        let atmosphere_enabled = self.settings.atmosphere && !self.reference_validation;
        let azimuth = self.settings.sun_azimuth.to_radians();
        let clock = self
            .water_time
            .unwrap_or_else(|| self.started.elapsed().as_secs_f32());
        if let Some(pack)=&self.resource_pack {
            let metadata=pack.metadata(clock as f64,self.settings.texture_linear);
            if metadata!=self.texture_metadata {
                let alpha_changed=metadata.as_chunks::<RECORD_WORDS>().0.iter().zip(self.texture_metadata.as_chunks::<RECORD_WORDS>().0.iter())
                    .any(|(a,b)|a[3]&1!=0 && a[..8]!=b[..8]);
                if alpha_changed {self.shadow_cache_key.clear();}
                self.buffers[3].write(bytemuck::cast_slice(&metadata));self.texture_metadata=metadata;
            }
        }
        let solar_phase = self.settings.sun_elevation.to_radians()
            + if self.settings.day_cycle {
                clock * std::f32::consts::TAU / self.settings.day_seconds.max(30.0)
            } else {
                0.0
            };
        let solar = Vec3::new(
            azimuth.sin() * solar_phase.cos(),
            solar_phase.sin(),
            azimuth.cos() * solar_phase.cos(),
        );
        let moon = solar.y < -0.02;
        // Quantize the shadow direction below disk radius to bound cache rebuild cadence.
        let elevation = if self.settings.day_cycle {
            (solar_phase.to_degrees() * 10.0).round().to_radians() / 10.0
        } else {
            solar_phase
        };
        let mut sun = [
            azimuth.sin() * elevation.cos(),
            elevation.sin(),
            azimuth.cos() * elevation.cos(),
            self.settings.sun_intensity,
        ];
        if moon {
            sun[0] = -sun[0];
            sun[1] = -sun[1];
            sun[2] = -sun[2];
            sun[3] *= 0.035;
        }
        let f = player.forward();
        let right = player.right();
        let up = right.cross(f);
        let (mut mesh_min, mut mesh_max) =
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        let mut primary_mesh = false;
        let mut mesh_distance = f32::INFINITY;
        for object in &self.scene.objects {
            if object.info[3]&crate::entities::INACTIVE!=0 {continue;}
            if object.info[2]==0 && (!self.fixture_scene || self.entity_snapshot) && !crate::entities::inside(Vec3::from_slice(&object.world[3]),&self.active_players) {continue;}
            if (object.info[2] == 0 && !self.settings.fish)
                || (object.info[2] != 0 && !self.settings.foliage)
            {
                continue;
            }
            if !primary_mesh
                && camera_frustum.intersects(
                    Vec3::from_slice(&object.low),
                    Vec3::from_slice(&object.high),
                )
            {
                let transform = Mat4::from_cols_array_2d(&object.world);
                for triangle in &self.scene.triangles
                    [object.info[0] as usize..(object.info[0] + object.info[1]) as usize]
                {
                    let a = transform.transform_point3(Vec3::from_slice(&triangle.a));
                    let b = transform.transform_point3(Vec3::from_slice(&triangle.b));
                    let c = transform.transform_point3(Vec3::from_slice(&triangle.c));
                    if camera_frustum.intersects(a.min(b).min(c) - 0.001, a.max(b).max(c) + 0.001) {
                        primary_mesh = true;
                        break;
                    }
                }
            }
            let closest = player.eye().clamp(
                Vec3::from_slice(&object.low),
                Vec3::from_slice(&object.high),
            );
            mesh_distance = mesh_distance.min((closest.distance(player.eye()) - 0.001).max(0.0));
            mesh_min = mesh_min.min(Vec3::from_slice(&object.low));
            mesh_max = mesh_max.max(Vec3::from_slice(&object.high));
        }
        primary_mesh = mesh_active && (primary_mesh || !self.settings.frustum);
        self.sky_only = self.settings.frustum && visible == 0 && !primary_mesh && !self.lod.as_ref().is_some_and(|l|l.visible(&camera_frustum));
        self.mesh_epoch = (self.mesh_epoch + 1) & 0x7fff_ffff;
        if self.mesh_epoch == 0 {
            self.mesh_epoch = 1;
            self.clear_mesh_epochs = true;
        }
        let light = Vec3::from_slice(&sun);
        let helper = if light.y.abs() > 0.99 {
            Vec3::X
        } else {
            Vec3::Y
        };
        let light_right = light.cross(helper).normalize();
        let light_up = light_right.cross(light).normalize();
        let cascade_center = (player.eye() / 4.0).floor() * 4.0;
        let cascade_range = (self.settings.view_chunks as f32 * EDGE as f32 * 1.5).max(432.0);
        let mut cache_key: Vec<u32> = sun
            .iter()
            .chain(cascade_center.as_ref().iter())
            .map(|v| v.to_bits())
            .collect();
        cache_key.extend(self.scene.objects.iter().filter(|o| o.info[2]==0).map(|o| {
            (o.info[3]&crate::entities::INACTIVE==0 && (self.fixture_scene && !self.entity_snapshot || crate::entities::inside(Vec3::from_slice(&o.world[3]),&self.active_players))) as u32
        }));
        cache_key.extend([
            cascade_range.to_bits(),
            csm_resolution,
            self.world_revision,
            self.lod.as_ref().map_or(0,|l|l.epoch),
            origin.x as u32,
            origin.y as u32,
            origin.z as u32,
            self.settings.fish as u32,
            self.settings.foliage as u32,
            mesh_active as u32,
            self.settings.far_shadows as u32,
            hardware as u32,
            self.settings.alpha_test as u32,
            self.settings.shadow_type as u32,
            self.settings.soft_shadows as u32,
        ]);
        let refresh_voxel_cascades = self.shadow_cache_key != cache_key;
        let refresh_mesh_cascades = refresh_voxel_cascades
            || self.shadow_mesh_bounds.len() != self.scene.objects.len()
            || self
                .scene
                .objects
                .iter()
                .zip(&self.shadow_mesh_bounds)
                .any(|(o, b)| {
                    !Vec3::from_slice(&o.low).cmpge(b[0]).all()
                        || !Vec3::from_slice(&o.high).cmple(b[1]).all()
                });
        if !self.sky_only && self.settings.csm && self.settings.shadows {
            self.shadow_cache_key = cache_key.clone();
            if refresh_mesh_cascades {
                self.shadow_mesh_bounds = self
                    .scene
                    .objects
                    .iter()
                    .map(|o| {
                        [
                            Vec3::from_slice(&o.low) - Vec3::ONE,
                            Vec3::from_slice(&o.high) + Vec3::ONE,
                        ]
                    })
                    .collect();
            }
        }
        let pose_time = if self.async_generation.is_some() {
            self.snapshot_time
        } else {
            self.animation_clock()
        };
        let refresh_mesh_depth = refresh_mesh_cascades
            || (mesh_active
                && self.settings.animate
                && self.settings.fish
                && (pose_time - self.shadow_mesh_time).abs() >= 0.25);
        let mut world_low = Vec3::new(
            origin.x as f32,
            origin.y as f32 + self.voxel_y[0] as f32,
            origin.z as f32,
        );
        let mut world_high = Vec3::new(
            origin.x as f32 + self.world_edge as f32,
            origin.y as f32 + self.voxel_y[1] as f32,
            origin.z as f32 + self.world_edge as f32,
        );
        if self.lod.as_ref().is_some_and(|l|l.active()) {
            let radius=(self.settings.view_chunks+1) as f32*EDGE as f32;
            world_low=world_low.min(Vec3::new(player.eye().x-radius,MIN_Y as f32,player.eye().z-radius));
            world_high=world_high.max(Vec3::new(player.eye().x+radius,MAX_Y as f32,player.eye().z+radius));
        }
        let low = if mesh_active {
            world_low.min(mesh_min)
        } else {
            world_low
        };
        let high = if mesh_active {
            world_high.max(mesh_max)
        } else {
            world_high
        };
        let center_z = ((low + high) * 0.5 - cascade_center).dot(light);
        let half_z = ((high - low) * 0.5).dot(light.abs());
        // Map pose/cadence is not a change to the receiving surface. Resetting
        // on each animated depth refresh caused periodic raw noisy frames.
        // Streaming swaps change cache revisions/origins, not the identity of
        // already visible world-space surfaces. Let reprojection reject only
        // the changed pixels; explicit edits/fixture replacement reset history.
        let mut history_key = vec![self.temporal_revision, csm_resolution,
            self.settings.view_chunks as u32, self.settings.fish as u32,
            self.settings.foliage as u32, mesh_active as u32,
            self.settings.far_shadows as u32, hardware as u32,
            self.settings.alpha_test as u32, self.settings.shadow_type as u32,
            self.settings.csm as u32];
        history_key.extend([self.settings.sun_elevation.to_bits(), self.settings.sun_azimuth.to_bits(), debug]);
        history_key.extend([
            self.settings.vxgi as u32,self.settings.sun_intensity.to_bits(),self.settings.gi_range.to_bits(),self.settings.gi_angle.to_bits(),self.settings.gi_temporal as u32,
            self.settings.shadow_ray_distance.to_bits(),
            self.settings.disk_radius.to_bits(),
            self.settings.shadows as u32,
            self.settings.penumbra_samples as u32,
            soft_enabled as u32,
            self.settings.atmosphere as u32,
            self.settings.water as u32,
            self.settings.waves as u32,
            self.settings.stochastic_waves as u32,
            self.settings.multiscale_waves as u32,self.settings.wave_warp.to_bits(),self.settings.wave_distance.to_bits(),
            self.settings.caustic_near as u32,self.settings.caustic_adaptive as u32,self.settings.caustic_shallow as u32,
            self.settings.caustics as u32,self.settings.caustic_strength.to_bits(),self.settings.caustic_distance.to_bits(),
            self.settings.ssr as u32,
            moon as u32,
        ]);
        let reset_history =
            self.clear_history || self.history_key != history_key;
        self.history_key = history_key;
        let sky_work = self.sky_cache.advance(
            solar,
            player.eye().y,
            clock,
            atmosphere_enabled,
            self.settings.day_cycle,
            self.settings.day_seconds,
            self.headless,
        );
        let p = Params {
            soft: [
                self.settings.disk_radius.to_radians(),
                soft_enabled as u32 as f32,
                self.settings.penumbra_samples as f32,
                moon as u32 as f32,
            ],
            history: [
                reset_history as u32,
                self.mesh_epoch & 1,
                self.mesh_epoch,
                self.mesh_epoch.wrapping_sub(1),
            ],
            previous_vp: self.previous_vp.to_cols_array_2d(),
            sky_light: solar.extend(self.settings.sun_intensity).to_array(),
            sky_build: self
                .sky_cache
                .build_sun
                .extend(self.sky_cache.build_height)
                .to_array(),
            sky_state: [
                self.sky_cache.active as u32,
                self.sky_cache.write_bank as u32,
                (atmosphere_enabled && self.sky_cache.ready) as u32,
                csm_resolution,
            ],
            lod: [self.settings.lod_pixels,if self.settings.lod_merging {1.0} else {0.0},0.0,0.0],
            wave_detail:[self.settings.multiscale_waves as u32 as f32,self.settings.wave_warp.clamp(0.0,4.0),self.settings.wave_distance.clamp(16.0,512.0),0.0],
            caustics:[self.settings.caustics as u32 as f32,self.settings.caustic_strength.clamp(0.0,2.0),self.settings.caustic_distance.clamp(8.0,96.0),24.0],
            caustic_map:[(player.eye().x/6.0).floor()*6.0-48.0,(player.eye().z/6.0).floor()*6.0-48.0,96.0/256.0,(self.settings.caustic_near as u32 | ((self.settings.caustic_adaptive as u32)<<1) | ((self.settings.caustic_shallow as u32)<<2)) as f32],
            gi:[self.settings.vxgi as u32 as f32,self.settings.gi_range,(self.settings.gi_angle.to_radians()*0.5).tan(),self.settings.gi_temporal as u32 as f32],
            display: [self.settings.exposure_ev.clamp(-8.0,8.0), (self.settings.tone_mapping && !self.reference_validation) as u32 as f32, self.settings.saturation.clamp(0.0,2.0), self.settings.ground_bounce as u32 as f32],
            shadow_controls: [
                self.settings.shadow_ray_distance,
                if self.settings.csm && self.settings.far_shadows {
                    self.settings.shadow_type as f32
                } else {
                    0.0
                },
                center_z + half_z + 2.0,
                2.0 * half_z + 4.0,
            ],
            light_right: light_right.extend(0.0).to_array(),
            light_up: light_up.extend(0.0).to_array(),
            cascade_center: cascade_center.extend(cascade_range).to_array(),
            acceleration: [
                self.settings.depth_pyramid as u32,
                self.settings.csm as u32,
                (pyramid_bytes(self.width, self.height) / 8 - 1) as u32,
                32 - (self.width.max(self.height) - 1).leading_zeros(),
            ],
            eye: player
                .eye()
                .extend(self.width as f32 / self.height as f32)
                .to_array(),
            forward: f
                .extend((FOV_Y_DEGREES.to_radians() / 2.0).tan())
                .to_array(),
            right: right.extend(0.0).to_array(),
            up: up
                .extend(self.settings.view_chunks as f32 * EDGE as f32)
                .to_array(),
            origin: origin
                .extend(self.voxel_y[0].saturating_sub(1) as i32)
                .to_array(),
            dims: [
                self.world_edge as u32,
                HEIGHT as u32,
                self.world_edge as u32,
                (self.voxel_y[1] + 1).min(HEIGHT as u32),
            ],
            screen: [
                self.width,
                self.height,
                debug,
                (self.mesh_epoch << 1) | (self.headless && !self.sparse_mesh_validation) as u32,
            ],
            scene_min: mesh_min.extend(mesh_distance).to_array(),
            scene_max: mesh_max
                .extend(if self.lod.as_ref().is_some_and(|l|l.active()){MAX_Y as f32}else{origin.y as f32 + self.voxel_y[1] as f32})
                .to_array(),
            sun,
            effects: [
                self.settings.ao_strength,
                self.settings.ao_radius,
                self.settings.water_density,
                self.settings.shadows as u32 as f32,
            ],
            meshes: [
                if mesh_active {
                    self.scene.objects.len() as u32
                } else {
                    0
                },
                self.settings.fish as u32,
                self.settings.foliage as u32,
                self.settings.alpha_test as u32,
            ],
            water: [
                self.water_time
                    .unwrap_or_else(|| self.started.elapsed().as_secs_f32()),
                self.settings.wave_strength,
                self.settings.ssr as u32 as f32,
                self.settings.waves as u32 as f32,
            ],
            optics: [
                self.settings.water_ior,
                self.settings.underwater_density,
                primary_mesh as u32 as f32,
                self.settings.stochastic_waves as u32 as f32,
            ],
            flags: [
                self.settings.hierarchy as u32,
                self.settings.frustum as u32,
                (self.settings.ssao && !self.settings.vxgi) as u32,
                self.settings.water as u32,
            ],
        };
        self.previous_vp = Mat4::perspective_rh_gl(
            FOV_Y_DEGREES.to_radians(),
            self.width as f32 / self.height as f32,
            0.01,
            4096.0,
        ) * Mat4::look_to_rh(player.eye(), f, up);
        crate::entities::set_local(&mut self.active_players,player.feet);
        let mut players=Vec::with_capacity(self.active_players.len()+1);
        players.push([f32::from_bits(self.active_players.len() as u32),(!self.fixture_scene || self.entity_snapshot) as u32 as f32,0.0,0.0]);
        players.extend(self.active_players.iter().map(|p| p.position.extend(0.0).to_array()));
        if players.len()*16>self.buffers[23].size {
            self.buffers[23]=Buffer::new(self.ctx.clone(),(players.len()*16).next_power_of_two(),vk::BufferUsageFlags::STORAGE_BUFFER,true)?;
            let info=[self.buffers[23].info()];
            unsafe {self.ctx.device.update_descriptor_sets(&[vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(27).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&info)],&[]);}
        }
        self.buffers[23].write(bytemuck::cast_slice(&players));
        self.buffers[4].write(bytemuck::bytes_of(&p));
        let d = &self.ctx.device;
        // SAFETY: render resources are exclusive to this thread and reused after its
        // frame fence. The worker owns separate buffers/AS and publishes completion semaphores.
        unsafe {
            d.reset_fences(&[self.fence])?;
            d.reset_command_buffer(self.command, vk::CommandBufferResetFlags::empty())?;
            d.begin_command_buffer(
                self.command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            if let Some(lod)=&mut self.lod {lod.record(d,self.command);}
            if self.queries != vk::QueryPool::null() {
                d.cmd_reset_query_pool(self.command, self.queries, 0, 22);
                d.cmd_write_timestamp(
                    self.command,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    self.queries,
                    0,
                );
            }
            if self.clear_history {
                d.cmd_fill_buffer(self.command, self.buffers[24].handle, 0, vk::WHOLE_SIZE, 0);
                d.cmd_fill_buffer(self.command, self.buffers[20].handle, 0, vk::WHOLE_SIZE, 0);
                d.cmd_fill_buffer(self.command, self.buffers[22].handle, 0, vk::WHOLE_SIZE, 0);
                let clear = vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
                d.cmd_pipeline_barrier(
                    self.command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[clear],
                    &[],
                    &[],
                );
                self.clear_history = false;
            }
            if self.clear_mesh_epochs {
                d.cmd_fill_buffer(self.command, self.buffers[16].handle, 0, vk::WHOLE_SIZE, 0);
                d.cmd_pipeline_barrier(
                    self.command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[vk::MemoryBarrier::default()
                        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                        .dst_access_mask(
                            vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE,
                        )],
                    &[],
                    &[],
                );
                self.clear_mesh_epochs = false;
            }
            let first_build = self.rt.as_ref().is_some_and(|rt| !rt.built);
            if let Some(rt) = &self.rt {
                if as_active || first_build {
                    rt.record_blas(self.command, &self.scene, self.settings.rebuild_blas);
                }
            }
            let as_stage = if self.ctx.ray_query {
                vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR
            } else {
                vk::PipelineStageFlags::TOP_OF_PIPE
            };
            if self.queries != vk::QueryPool::null() {
                d.cmd_write_timestamp(self.command, as_stage, self.queries, 1);
            }
            if let Some(rt) = &self.rt {
                if as_active || first_build {
                    rt.record_tlas(self.command, &self.scene);
                }
            }
            if self.queries != vk::QueryPool::null() {
                d.cmd_write_timestamp(self.command, as_stage, self.queries, 2);
            }
            d.cmd_bind_descriptor_sets(
                self.command,
                vk::PipelineBindPoint::COMPUTE,
                self.layout,
                0,
                &[self.set],
                &[],
            );
            let barrier = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
            let extra = if self.ctx.ray_query { 14 } else { 11 };
            if self.queries != vk::QueryPool::null() {
                d.cmd_write_timestamp(
                    self.command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    self.queries,
                    14,
                );
            }
            if let Some((first_row, row_count, finish)) = sky_work {
                d.cmd_bind_pipeline(
                    self.command,
                    vk::PipelineBindPoint::COMPUTE,
                    self.pipelines[extra + 2],
                );
                d.cmd_push_constants(
                    self.command,
                    self.layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    bytemuck::cast_slice(&[first_row, 0, 0, 0]),
                );
                d.cmd_dispatch(self.command, 128, row_count.div_ceil(8), 1);
                d.cmd_pipeline_barrier(
                    self.command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[barrier],
                    &[],
                    &[],
                );
                if finish {
                    d.cmd_bind_pipeline(
                        self.command,
                        vk::PipelineBindPoint::COMPUTE,
                        self.pipelines[extra + 3],
                    );
                    for mode in [0u32, 1, 2] {
                        d.cmd_push_constants(
                            self.command,
                            self.layout,
                            vk::ShaderStageFlags::COMPUTE,
                            0,
                            bytemuck::cast_slice(&[mode, 0, 0, 0]),
                        );
                        d.cmd_dispatch(self.command, 128, 1, 1);
                        d.cmd_pipeline_barrier(
                            self.command,
                            vk::PipelineStageFlags::COMPUTE_SHADER,
                            vk::PipelineStageFlags::COMPUTE_SHADER,
                            vk::DependencyFlags::empty(),
                            &[barrier],
                            &[],
                            &[],
                        );
                    }
                }
            }
            if self.queries != vk::QueryPool::null() {d.cmd_write_timestamp(self.command,vk::PipelineStageFlags::COMPUTE_SHADER,self.queries,18);}
            if self.settings.caustics && self.settings.caustic_strength>0.0 && self.settings.water && self.settings.ssr && self.settings.waves && self.settings.wave_strength>0.0001 && sun[1]>0.06 && !self.sky_only {
                d.cmd_fill_buffer(self.command,self.buffers[25].handle,0,vk::WHOLE_SIZE,0);
                let clear=vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
                d.cmd_pipeline_barrier(self.command,vk::PipelineStageFlags::TRANSFER,vk::PipelineStageFlags::COMPUTE_SHADER,vk::DependencyFlags::empty(),&[clear],&[],&[]);
                d.cmd_bind_pipeline(self.command,vk::PipelineBindPoint::COMPUTE,self.pipelines[extra+5]);
                d.cmd_dispatch(self.command,32,32,if self.settings.caustic_near {2} else {1});
                d.cmd_pipeline_barrier(self.command,vk::PipelineStageFlags::COMPUTE_SHADER,vk::PipelineStageFlags::COMPUTE_SHADER,vk::DependencyFlags::empty(),&[barrier],&[],&[]);
            }
            if self.queries != vk::QueryPool::null() {d.cmd_write_timestamp(self.command,vk::PipelineStageFlags::COMPUTE_SHADER,self.queries,19);}
            // Refresh a bounded terrain sample without CPU readback or per-pixel rays.
            if atmosphere_enabled && (self.headless || self.mesh_epoch.is_multiple_of(16) || sky_work.is_some_and(|v| v.2)) {
                d.cmd_bind_pipeline(self.command,vk::PipelineBindPoint::COMPUTE,self.pipelines[extra+3]);
                for mode in [3u32,4] {
                    d.cmd_push_constants(self.command,self.layout,vk::ShaderStageFlags::COMPUTE,0,bytemuck::cast_slice(&[mode,0,0,0]));
                    d.cmd_dispatch(self.command,if mode==3 {128} else {1},1,1);
                    d.cmd_pipeline_barrier(self.command,vk::PipelineStageFlags::COMPUTE_SHADER,vk::PipelineStageFlags::COMPUTE_SHADER,vk::DependencyFlags::empty(),&[barrier],&[],&[]);
                }
            }
            if self.queries != vk::QueryPool::null() {
                d.cmd_write_timestamp(
                    self.command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    self.queries,
                    15,
                );
            }
            if self.queries != vk::QueryPool::null() {
                d.cmd_write_timestamp(
                    self.command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    self.queries,
                    12,
                );
            }
            if !self.sky_only
                && self.settings.csm
                && self.settings.shadows
                && (refresh_voxel_cascades || (mesh_active && refresh_mesh_cascades))
            {
                // Persistent voxel cascades; mesh envelopes conservatively contain the published AS pose.
                let transfer = vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
                d.cmd_pipeline_barrier(
                    self.command,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[transfer],
                    &[],
                    &[],
                );
                let layer_bytes = 4 * 128 * 128 * 4;
                if refresh_voxel_cascades {
                    d.cmd_fill_buffer(self.command, self.buffers[18].handle, 0, layer_bytes, 0);
                }
                if refresh_mesh_cascades {
                    d.cmd_fill_buffer(
                        self.command,
                        self.buffers[18].handle,
                        layer_bytes,
                        layer_bytes,
                        0,
                    );
                }
                let cleared = vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
                d.cmd_pipeline_barrier(
                    self.command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[cleared],
                    &[],
                    &[],
                );
                d.cmd_bind_pipeline(
                    self.command,
                    vk::PipelineBindPoint::COMPUTE,
                    self.pipelines[if self.ctx.ray_query { 11 } else { 9 }],
                );
                if refresh_voxel_cascades {
                    self.cascade_updates[0] += 1;
                    d.cmd_push_constants(
                        self.command,
                        self.layout,
                        vk::ShaderStageFlags::COMPUTE,
                        0,
                        bytemuck::cast_slice(&[0u32, 0, 0, 0]),
                    );
                    d.cmd_dispatch(
                        self.command,
                        (self.sections.len() as u32).div_ceil(64),
                        1,
                        1,
                    );
                }
                if refresh_voxel_cascades && self.lod.as_ref().is_some_and(|l|l.active()) {
                    d.cmd_push_constants(self.command,self.layout,vk::ShaderStageFlags::COMPUTE,0,bytemuck::cast_slice(&[2u32,0,0,0]));
                    d.cmd_dispatch(self.command,((crate::lod::PAGE_SIDE*crate::lod::PAGE_SIDE) as u32).div_ceil(64),1,1);
                }
                if mesh_active && refresh_mesh_cascades {
                    self.cascade_updates[1] += 1;
                    d.cmd_push_constants(
                        self.command,
                        self.layout,
                        vk::ShaderStageFlags::COMPUTE,
                        0,
                        bytemuck::cast_slice(&[1u32, 0, 0, 0]),
                    );
                    d.cmd_dispatch(
                        self.command,
                        (self.scene.objects.len() as u32).div_ceil(64),
                        1,
                        1,
                    );
                }
                d.cmd_pipeline_barrier(
                    self.command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[barrier],
                    &[],
                    &[],
                );
            }
            if !self.sky_only
                && self.settings.csm
                && self.settings.shadows
                && ((self.settings.far_shadows && self.settings.shadow_type != 0) || soft_enabled)
            {
                let software = if self.ctx.ray_query { 12 } else { 10 };
                if refresh_voxel_cascades {
                    self.far_depth_updates[0] += 1;
                    d.cmd_bind_pipeline(
                        self.command,
                        vk::PipelineBindPoint::COMPUTE,
                        self.pipelines[software],
                    );
                    d.cmd_push_constants(
                        self.command,
                        self.layout,
                        vk::ShaderStageFlags::COMPUTE,
                        0,
                        bytemuck::cast_slice(&[0u32, 0, 0, 0]),
                    );
                    d.cmd_dispatch(self.command, csm_resolution.div_ceil(8), csm_resolution.div_ceil(8), 4);
                }
                if refresh_mesh_depth {
                    self.far_depth_updates[1] += 1;
                    self.shadow_mesh_time = pose_time;
                    let pipeline = if hardware && mesh_active {
                        13
                    } else {
                        software
                    };
                    d.cmd_bind_pipeline(
                        self.command,
                        vk::PipelineBindPoint::COMPUTE,
                        self.pipelines[pipeline],
                    );
                    d.cmd_push_constants(
                        self.command,
                        self.layout,
                        vk::ShaderStageFlags::COMPUTE,
                        0,
                        bytemuck::cast_slice(&[1u32, 0, 0, 0]),
                    );
                    d.cmd_dispatch(self.command, csm_resolution.div_ceil(8), csm_resolution.div_ceil(8), 4);
                }
                d.cmd_pipeline_barrier(
                    self.command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[barrier],
                    &[],
                    &[],
                );
            }
            if self.queries != vk::QueryPool::null() {
                d.cmd_write_timestamp(
                    self.command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    self.queries,
                    13,
                );
            }
            // Opaque voxel depth bounds every polygon primary ray.
            for (pipeline, query, polygon) in [
                (0, 3, false),
                (if hardware { 5 } else { 3 }, 0, true),
                (if self.ctx.ray_query { 7 } else { 5 }, 4, true),
                (1, 5, false),
                (if hardware { 6 } else { 4 }, 6, true),
                (2, 9, false),
                (if self.ctx.ray_query { 8 } else { 6 }, 7, false),
            ] {
                if pipeline == 1 && self.queries != vk::QueryPool::null() {
                    d.cmd_write_timestamp(
                        self.command,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        self.queries,
                        10,
                    );
                }
                if pipeline == 1
                    && !self.sky_only
                    && self.settings.ssao && !self.settings.vxgi
                    && self.settings.depth_pyramid
                {
                    let mut w = self.width.div_ceil(2);
                    let mut h = self.height.div_ceil(2);
                    let mut level = 0u32;
                    if self.width > 1 || self.height > 1 {
                        loop {
                            d.cmd_bind_pipeline(
                                self.command,
                                vk::PipelineBindPoint::COMPUTE,
                                self.pipelines[if self.ctx.ray_query { 10 } else { 8 }],
                            );
                            d.cmd_push_constants(
                                self.command,
                                self.layout,
                                vk::ShaderStageFlags::COMPUTE,
                                0,
                                bytemuck::cast_slice(&[level, 0u32, 0, 0]),
                            );
                            d.cmd_dispatch(self.command, w.div_ceil(8), h.div_ceil(8), 1);
                            d.cmd_pipeline_barrier(
                                self.command,
                                vk::PipelineStageFlags::COMPUTE_SHADER,
                                vk::PipelineStageFlags::COMPUTE_SHADER,
                                vk::DependencyFlags::empty(),
                                &[barrier],
                                &[],
                                &[],
                            );
                            level += 4;
                            if level >= p.acceleration[3] {
                                break;
                            }
                            w = self.width.div_ceil(1 << (level + 1));
                            h = self.height.div_ceil(1 << (level + 1));
                        }
                    }
                }
                if pipeline == 1 && self.queries != vk::QueryPool::null() {
                    d.cmd_write_timestamp(
                        self.command,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        self.queries,
                        11,
                    );
                }
                if pipeline==2 && self.queries!=vk::QueryPool::null() {d.cmd_write_timestamp(self.command,vk::PipelineStageFlags::COMPUTE_SHADER,self.queries,20);}
                if pipeline==2 && !self.sky_only && self.settings.vxgi {
                    d.cmd_bind_pipeline(self.command,vk::PipelineBindPoint::COMPUTE,self.pipelines[extra+4]);
                    d.cmd_dispatch(self.command,self.width.div_ceil(2).div_ceil(8),self.height.div_ceil(2).div_ceil(8),1);
                    d.cmd_pipeline_barrier(self.command,vk::PipelineStageFlags::COMPUTE_SHADER,vk::PipelineStageFlags::COMPUTE_SHADER,vk::DependencyFlags::empty(),&[barrier],&[],&[]);
                }
                if pipeline==2 && self.queries!=vk::QueryPool::null() {d.cmd_write_timestamp(self.command,vk::PipelineStageFlags::COMPUTE_SHADER,self.queries,21);}
                if self.sky_only {
                    if pipeline == 0 {
                        d.cmd_bind_pipeline(
                            self.command,
                            vk::PipelineBindPoint::COMPUTE,
                            self.pipelines[if self.ctx.ray_query { 9 } else { 7 }],
                        );
                        d.cmd_dispatch(
                            self.command,
                            self.width.div_ceil(8),
                            self.height.div_ceil(8),
                            1,
                        );
                    }
                } else if (!polygon || mesh_active)
                    && (pipeline != if hardware { 5 } else { 3 } || primary_mesh)
                    && (pipeline != if self.ctx.ray_query { 7 } else { 5 } || primary_mesh)
                {
                    d.cmd_bind_pipeline(
                        self.command,
                        vk::PipelineBindPoint::COMPUTE,
                        self.pipelines[pipeline],
                    );
                    d.cmd_dispatch(
                        self.command,
                        self.width.div_ceil(8),
                        self.height.div_ceil(8),
                        1,
                    );
                }
                if pipeline == 2 && self.queries != vk::QueryPool::null() {
                    d.cmd_write_timestamp(
                        self.command,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        self.queries,
                        16,
                    );
                }
                if pipeline == 2 && !self.sky_only && soft_enabled {
                    for filter in [extra, extra + 1] {
                        d.cmd_pipeline_barrier(
                            self.command,
                            vk::PipelineStageFlags::COMPUTE_SHADER,
                            vk::PipelineStageFlags::COMPUTE_SHADER,
                            vk::DependencyFlags::empty(),
                            &[barrier],
                            &[],
                            &[],
                        );
                        d.cmd_bind_pipeline(
                            self.command,
                            vk::PipelineBindPoint::COMPUTE,
                            self.pipelines[filter],
                        );
                        d.cmd_dispatch(
                            self.command,
                            self.width.div_ceil(8),
                            self.height.div_ceil(8),
                            1,
                        );
                    }
                }
                if pipeline == 2 && self.queries != vk::QueryPool::null() {
                    d.cmd_write_timestamp(
                        self.command,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        self.queries,
                        17,
                    );
                }
                if query != 0 && self.queries != vk::QueryPool::null() {
                    d.cmd_write_timestamp(
                        self.command,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        self.queries,
                        query,
                    );
                }
                d.cmd_pipeline_barrier(
                    self.command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[barrier],
                    &[],
                    &[],
                );
            }
            if let Some(index) = acquired {
                self.presentation.as_mut().unwrap().record(
                    self.command,
                    index,
                    self.buffers[2].handle,
                    draw,
                )?;
            } else {
                let host = vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::HOST_READ);
                d.cmd_pipeline_barrier(
                    self.command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::HOST,
                    vk::DependencyFlags::empty(),
                    &[host],
                    &[],
                    &[],
                );
            }
            if self.queries != vk::QueryPool::null() {
                d.cmd_write_timestamp(
                    self.command,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    self.queries,
                    8,
                );
            }
            d.end_command_buffer(self.command)?;
            let commands = [self.command];
            let mut submit = vk::SubmitInfo::default().command_buffers(&commands);
            let mut waits: Vec<_> = self.ready_waits.iter().map(|v| v.semaphore).collect();
            let mut stages = vec![vk::PipelineStageFlags::COMPUTE_SHADER; waits.len()];
            let signals =
                acquired.map(|i| [self.presentation.as_ref().unwrap().finished[i as usize]]);
            if acquired.is_some() {
                waits.push(self.presentation.as_ref().unwrap().acquired);
                stages.push(vk::PipelineStageFlags::TRANSFER);
            }
            submit = submit.wait_semaphores(&waits).wait_dst_stage_mask(&stages);
            if let Some(s) = &signals {
                submit = submit.signal_semaphores(s);
            }
            let prepare_ms = started.elapsed().as_secs_f32() * 1000.0 - acquired_ms;
            let submit_started = Instant::now();
            {
                let _guard = self.ctx.queue_lock.lock().unwrap();
                d.queue_submit(self.queue, &[submit], self.fence)?;
                if let Some(index) = acquired {
                    self.presentation
                        .as_mut()
                        .unwrap()
                        .present(self.queue, index)?;
                }
            }
            let submitted_ms = submit_started.elapsed().as_secs_f32() * 1000.0;
            let wait_started = Instant::now();
            // Render frame completion. Background snapshots never block this CPU on their fences.
            d.wait_for_fences(&[self.fence], true, u64::MAX)?;
            for (max, value) in self.cpu_stage_max.iter_mut().zip([
                acquired_ms,
                prepare_ms,
                submitted_ms,
                wait_started.elapsed().as_secs_f32() * 1000.0,
            ]) {
                *max = max.max(value);
            }
            self.ready_waits.clear();
            if self.queries != vk::QueryPool::null() {
                let mut times = [0u64; 22];
                d.get_query_pool_results(
                    self.queries,
                    0,
                    &mut times,
                    vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
                )?;
                let mask = if self.timestamp_bits == 64 {
                    u64::MAX
                } else {
                    (1u64 << self.timestamp_bits) - 1
                };
                let ms = |a: usize, b: usize| {
                    ((times[b].wrapping_sub(times[a])) & mask) as f32 * self.timestamp_period
                        / 1_000_000.0
                };
                self.water_ms = ms(9, 7);
                self.acceleration_ms = [ms(10, 11), ms(12, 13)];
                self.caustic_ms = ms(18, 19);
                self.gi_ms=ms(20,21);
                self.sky_ms = (ms(14, 15)-self.caustic_ms).max(0.0);
                self.denoise_ms = ms(16, 17);
                self.gpu_ms = Some([ms(2, 3), ms(4, 5), ms(6, 7), ms(0, 8)]);
                self.mesh_timings.gpu = [
                    if self.async_generation.is_some() {
                        self.mesh_timings.gpu[0]
                    } else if as_active || first_build {
                        ms(0, 1)
                    } else {
                        0.0
                    },
                    if self.async_generation.is_some() {
                        self.mesh_timings.gpu[1]
                    } else if as_active || first_build {
                        ms(1, 2)
                    } else {
                        0.0
                    },
                    if mesh_active { ms(3, 4) } else { 0.0 },
                    if mesh_active { ms(5, 6) } else { 0.0 },
                ];
                if first_build {
                    self.mesh_timings.initial_build_ms = ms(0, 2);
                }
            }
            if let Some(rt) = &mut self.rt {
                if as_active || first_build {
                    rt.built = true;
                }
            }
            Ok(())
        }
    }
    fn update_atlas_descriptors(&self) {
        let atlas=self.atlas_gpu.as_ref().unwrap();
        let images=vec![vk::DescriptorImageInfo::default().image_view(atlas.view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);if self.ctx.bindless{DESCRIPTORS as usize}else{1}];
        let sampler=[vk::DescriptorImageInfo::default().sampler(atlas.sampler)];
        unsafe {self.ctx.device.update_descriptor_sets(&[
            vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(24).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&images),
            vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(25).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&sampler),
        ],&[]);}
    }
    pub fn load_resource_pack(&mut self,path:&std::path::Path)->Result<()> {
        let pack=Pack::open(path,&textures())?;
        self.install_resource_pack(pack)
    }
    pub fn reset_resource_pack(&mut self)->Result<()> {self.install_resource_pack(Pack::generated(&textures())?)}
    fn install_resource_pack(&mut self,mut pack:Pack)->Result<()> {
        let image=AtlasTexture::new(self.ctx.clone(),self.queue,self.pool,pack.width,pack.height,&pack.rgba)?;
        self.ctx.wait_idle()?;
        self.atlas_gpu=Some(image);
        self.texture_metadata=pack.metadata(0.0,self.settings.texture_linear);
        self.buffers[3].write(bytemuck::cast_slice(&self.texture_metadata));
        self.update_atlas_descriptors();
        println!("Resource pack {}: {} textures, {} animated, {} frames, {} skipped, {}x{} atlas; {}",pack.stats.name,pack.stats.imported,pack.stats.animated,pack.stats.frames,pack.stats.skipped,pack.width,pack.height,if self.ctx.bindless{"bindless descriptor table"}else{"single-atlas fallback"});
        for warning in &pack.stats.warnings {eprintln!("Resource pack: {warning}");}
        pack.rgba.clear();pack.rgba.shrink_to_fit();self.resource_pack=Some(pack);
        self.shadow_cache_key.clear();self.history_key.clear();
        Ok(())
    }
    pub fn bindless_textures(&self)->bool {self.ctx.bindless}
    pub fn wave_anisotropy(&self) -> f32 {
        self.wave_texture.as_ref().map_or(1.0, |w| w.anisotropy)
    }
    pub fn depth_bounds(&self) -> &[[f32; 2]] {
        assert!(self.headless);
        unsafe {
            std::slice::from_raw_parts(
                self.buffers[17].ptr.cast(),
                pyramid_bytes(self.width, self.height) / 8,
            )
        }
    }
    pub fn caustic_data(&self)->&[u32] {
        assert!(self.headless);
        unsafe {std::slice::from_raw_parts(self.buffers[25].ptr.cast(),self.buffers[25].size/4)}
    }
    pub fn gi_history_data(&self)->&[[f32;4]] {
        assert!(self.headless);
        unsafe {std::slice::from_raw_parts(self.buffers[24].ptr.cast(),self.buffers[24].size/16)}
    }
    pub fn shadow_frame(&self) -> u32 {
        self.mesh_epoch
    }
    pub fn shadow_samples(&self) -> &[[f32; 4]] {
        assert!(self.headless);
        unsafe {
            std::slice::from_raw_parts(
                self.buffers[22].ptr.cast(),
                self.width as usize * self.height as usize,
            )
        }
    }
    pub fn shadow_history_data(&self) -> &[[f32; 4]] {
        assert!(self.headless);
        unsafe {
            std::slice::from_raw_parts(
                self.buffers[20].ptr.cast(),
                self.width as usize * self.height as usize * 4,
            )
        }
    }
    pub fn sky_data(&self) -> &[[f32; 4]] {
        assert!(self.headless);
        unsafe {
            std::slice::from_raw_parts(self.buffers[21].ptr.cast(), crate::sky::BANK_ENTRIES * 2)
        }
    }
    pub fn ambient_occlusion(&self) -> &[f32] {
        assert!(self.headless);
        unsafe {
            std::slice::from_raw_parts(
                self.buffers[9].ptr.cast(),
                self.width as usize * self.height as usize,
            )
        }
    }
    pub fn visibility(&self) -> &[Visibility] {
        // Only inspect after render() has completed its fence wait.
        assert!(self.headless);
        unsafe {
            std::slice::from_raw_parts(
                self.buffers[1].ptr.cast(),
                self.width as usize * self.height as usize,
            )
        }
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let d = &self.ctx.device;
            let _ = self.ctx.wait_idle();
            self.presentation.take();
            self.wave_texture.take();
            self.atlas_gpu.take();
            self.rt.take();
            d.destroy_query_pool(self.queries, None);
            for p in self.pipelines.drain(..) {
                d.destroy_pipeline(p, None);
            }
            d.destroy_pipeline_layout(self.layout, None);
            d.destroy_descriptor_pool(self.descriptor_pool, None);
            d.destroy_descriptor_set_layout(self.descriptor_layout, None);
            d.destroy_fence(self.fence, None);
            d.destroy_command_pool(self.pool, None);
            // Buffers drop next; their Arc keeps device + instance alive until last buffer is freed.
        }
    }
}
/// Tiny deterministic RGBA textures, packed as 0xAARRGGBB.
pub(crate) fn textures() -> [u32; MATERIALS * TEXTURE_PIXELS] {
    let base = [
        [126, 130, 138],
        [88, 155, 55],
        [125, 83, 48],
        [213, 195, 132],
        [43, 117, 181],
        [132, 93, 48],
        [235, 132, 42],
        [54, 171, 204],
        [63, 150, 43],
        [47, 125, 52],
        [69, 139, 51],
        [235, 243, 250], // snow
        [66, 132, 43],   // oak leaves
        [35, 89, 64],    // pine needles
        [130, 126, 120], // gravel
        [205, 181, 125], // sandstone
        [157, 118, 103], // granite
        [113, 174, 65], // meadow grass
        [98, 69, 42], // podzol
    ];
    std::array::from_fn(|i| {
        let m = i / TEXTURE_PIXELS;
        let tx = i % TEXTURE_EDGE;
        let ty = (i / TEXTURE_EDGE) % TEXTURE_EDGE;
        let x = tx / 2;
        let y = ty / 2;
        let noise = ((tx * 13 + ty * 29 + m * 7) % 19) as i32 - 9;
        let grain = if m==15 && ty.is_multiple_of(5) {
            -20
        } else if (m==14 || m==16) && (tx*17+ty*31).is_multiple_of(7) {
            -35
        } else if m == 5 && x.is_multiple_of(3) {
            -22
        } else if m == 4 && (x + y).is_multiple_of(5) {
            20
        } else if (m == 6 || m == 7) && x.is_multiple_of(3) {
            -40
        } else {
            0
        };
        let c = base[m].map(|v: i32| (v + noise + grain).clamp(0, 255) as u32);
        let alpha = match m {
            8 => x == 1 || x == 6 || (y > 3 && x == 2),
            9 => x == 3 || x == 4 || (y.is_multiple_of(2) && x.abs_diff(3) <= y / 2 + 1),
            10 => {
                ((x as f32 - 3.5) / 3.4).powi(2) + ((y as f32 - 3.5) / 3.8).powi(2) < 1.0
                    && !(x + y).is_multiple_of(5)
            }
            _ => true,
        };
        let mut rgb = (c[0] << 16) | (c[1] << 8) | c[2];
        if (m == 6 || m == 7) && x == 6 && y == 3 {
            rgb = 0x111820;
        }
        rgb | if alpha { 0xff000000 } else { 0 }
    })
}

fn pyramid_bytes(mut w: u32, mut h: u32) -> usize {
    let mut pixels = 0usize;
    loop {
        pixels += w as usize * h as usize;
        if w == 1 && h == 1 {
            return pixels * 8;
        }
        w = w.div_ceil(2);
        h = h.div_ceil(2);
    }
}


impl ContextVk {
    pub(crate) fn create(window: Option<Arc<Window>>) -> Result<Arc<Self>> {
        let (width,height)=window.as_ref().map(|w| {let size=w.inner_size();(size.width.max(1),size.height.max(1))}).unwrap_or((RENDER_WIDTH,RENDER_HEIGHT));
        // SAFETY: ash dynamically resolves functions; Entry remains alive for the instance lifetime.
        let entry = unsafe { Entry::load() }
            .context("Vulkan loader missing; install a Vulkan driver (or Mesa lavapipe)")?;
        let name = CString::new("BlockWorld")?;
        let app = vk::ApplicationInfo::default()
            .application_name(&name)
            .api_version(vk::API_VERSION_1_2);
        let validation = std::env::var_os("BLOCKWORLD_VALIDATION").is_some();
        let layer = CString::new("VK_LAYER_KHRONOS_validation")?;
        let layers = if validation {
            vec![layer.as_ptr()]
        } else {
            vec![]
        };
        let extensions = if let Some(w) = &window {
            ash_window::enumerate_required_extensions(w.display_handle()?.as_raw())?.to_vec()
        } else {
            vec![]
        };
        let sync_features = [vk::ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION];
        let mut validation_features =
            vk::ValidationFeaturesEXT::default().enabled_validation_features(&sync_features);
        let mut instance_info = vk::InstanceCreateInfo::default()
            .application_info(&app)
            .enabled_layer_names(&layers)
            .enabled_extension_names(&extensions);
        if validation && std::env::var_os("BLOCKWORLD_SYNC_VALIDATION").is_some() {
            instance_info = instance_info.push_next(&mut validation_features);
        }
        let instance = unsafe { entry.create_instance(&instance_info, None) }?;
        let surface = if let Some(w) = &window {
            let loader = ash::khr::surface::Instance::new(&entry, &instance);
            let handle = unsafe {
                ash_window::create_surface(
                    &entry,
                    &instance,
                    w.display_handle()?.as_raw(),
                    w.window_handle()?.as_raw(),
                    None,
                )
            };
            match handle {
                Ok(h) => Some((loader, h)),
                Err(e) => {
                    unsafe {
                        instance.destroy_instance(None);
                    }
                    return Err(e.into());
                }
            }
        } else {
            None
        };
        let selection = (|| -> Result<_> {
            let mut choices = Vec::new();
            for physical in unsafe { instance.enumerate_physical_devices() }? {
                let properties = unsafe { instance.get_physical_device_properties(physical) };
                if properties.api_version < vk::API_VERSION_1_2
                    || properties.limits.max_storage_buffer_range
                        < (GPU_VOXELS).max(width as usize * height as usize * 64) as u32
                    || properties.limits.max_per_stage_descriptor_storage_buffers < 24
                    || properties.limits.max_descriptor_set_storage_buffers < 24
                {
                    continue;
                }
                if let Some(family) =
                    unsafe { instance.get_physical_device_queue_family_properties(physical) }
                        .iter()
                        .enumerate()
                        .position(|(i, p)| {
                            let flags = if surface.is_some() {
                                vk::QueueFlags::COMPUTE | vk::QueueFlags::GRAPHICS
                            } else {
                                vk::QueueFlags::COMPUTE
                            };
                            p.queue_flags.contains(flags)
                                && surface.as_ref().is_none_or(|(l, h)| unsafe {
                                    l.get_physical_device_surface_support(physical, i as u32, *h)
                                        .unwrap_or(false)
                                })
                        })
                {
                    let rank = match properties.device_type {
                        vk::PhysicalDeviceType::DISCRETE_GPU => 0,
                        vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
                        _ => 2,
                    };
                    choices.push((rank, physical, family as u32, properties));
                }
            }
            choices.sort_by_key(|c| c.0);
            let (_, physical, family, properties) =
                choices.into_iter().next().ok_or_else(|| {
                    anyhow!("No Vulkan 1.2 compute device with sufficient storage buffer capacity")
                })?;
            let queue_count =
                unsafe { instance.get_physical_device_queue_family_properties(physical) }
                    [family as usize]
                    .queue_count
                    .min(2);
            let priorities = &[1.0, 0.25][..queue_count as usize];
            let queues = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(family)
                .queue_priorities(priorities)];
            let mut device_extensions = if surface.is_some() {
                vec![ash::khr::swapchain::NAME.as_ptr()]
            } else {
                vec![]
            };
            let available = unsafe { instance.enumerate_device_extension_properties(physical) }?;
            let required = [
                ash::khr::acceleration_structure::NAME,
                ash::khr::ray_query::NAME,
                ash::khr::deferred_host_operations::NAME,
            ];
            let extensions_ok = required.iter().all(|name| {
                available
                    .iter()
                    .any(|e| unsafe { CStr::from_ptr(e.extension_name.as_ptr()) } == *name)
            });
            let mut bda = vk::PhysicalDeviceVulkan12Features::default();
            let mut acceleration = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default();
            let mut query = vk::PhysicalDeviceRayQueryFeaturesKHR::default();
            let mut features = vk::PhysicalDeviceFeatures2::default()
                .push_next(&mut bda)
                .push_next(&mut acceleration)
                .push_next(&mut query);
            unsafe {
                instance.get_physical_device_features2(physical, &mut features);
            }
            let anisotropy = features.features.sampler_anisotropy != 0;
            let dynamic_indexing=features.features.shader_sampled_image_array_dynamic_indexing!=0;
            let bindless=std::env::var_os("BLOCKWORLD_DISABLE_BINDLESS").is_none()
                && bda.runtime_descriptor_array!=0 && bda.shader_sampled_image_array_non_uniform_indexing!=0
                && dynamic_indexing;
            let ray_query = std::env::var_os("BLOCKWORLD_DISABLE_RAY_QUERY").is_none()
                && extensions_ok
                && bda.buffer_device_address != 0
                && acceleration.acceleration_structure != 0
                && query.ray_query != 0;
            let mut enable_bda =
                vk::PhysicalDeviceVulkan12Features::default().buffer_device_address(ray_query)
                    .runtime_descriptor_array(bindless).shader_sampled_image_array_non_uniform_indexing(bindless);
            let mut enable_as = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default()
                .acceleration_structure(true);
            let mut enable_query = vk::PhysicalDeviceRayQueryFeaturesKHR::default().ray_query(true);
            if ray_query {
                device_extensions.extend(required.iter().map(|v| v.as_ptr()));
            }
            let core = vk::PhysicalDeviceFeatures::default().sampler_anisotropy(anisotropy)
                .shader_sampled_image_array_dynamic_indexing(bindless);
            let mut create = vk::DeviceCreateInfo::default()
                .enabled_features(&core)
                .queue_create_infos(&queues)
                .enabled_extension_names(&device_extensions)
                .push_next(&mut enable_bda);
            if ray_query {
                create = create
                    .push_next(&mut enable_as)
                    .push_next(&mut enable_query);
            }
            let device = unsafe { instance.create_device(physical, &create, None) }?;
            let device_name = unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            Ok((physical, family, device, device_name, ray_query, anisotropy, bindless))
        })();
        let (physical, family, device, _device_name, ray_query, anisotropy, bindless) = match selection {
            Ok(s) => s,
            Err(e) => {
                unsafe {
                    if let Some((l, h)) = &surface {
                        l.destroy_surface(*h, None);
                    }
                    instance.destroy_instance(None);
                }
                return Err(e);
            }
        };
        let count = unsafe { instance.get_physical_device_queue_family_properties(physical) }
            [family as usize]
            .queue_count;
        let background_queue =
            unsafe { device.get_device_queue(family, if count > 1 { 1 } else { 0 }) };
        let ctx = Arc::new(ContextVk {
            family,
            background_queue,
            queue_lock: Mutex::new(()),
            background_lock: Mutex::new(()),
            independent_queue: count > 1,
            _entry: entry,
            instance,
            device,
            physical,
            ray_query,
            anisotropy,
            bindless,
            surface,
        });
        Ok(ctx)
    }
}
