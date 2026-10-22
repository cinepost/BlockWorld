//! Optional Vulkan triangle BLAS/TLAS path. No emulated timings on unsupported devices.
use crate::{
    meshes::MeshScene,
    renderer::{Buffer, ContextVk},
};
use anyhow::Result;
use ash::vk;
use std::sync::Arc;
struct Acceleration {
    _ctx: Arc<ContextVk>,
    loader: ash::khr::acceleration_structure::Device,
    pub handle: vk::AccelerationStructureKHR,
    _storage: Option<Buffer>,
    _scratch: Option<Buffer>,
    scratch_address: u64,
}
impl Acceleration {
    fn sizes(
        loader: &ash::khr::acceleration_structure::Device,
        ty: vk::AccelerationStructureTypeKHR,
        geometry: &vk::AccelerationStructureGeometryKHR<'_>,
        count: u32,
    ) -> vk::AccelerationStructureBuildSizesInfoKHR<'static> {
        let info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(ty)
            .flags(
                vk::BuildAccelerationStructureFlagsKHR::ALLOW_UPDATE
                    | vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE,
            )
            .geometries(std::slice::from_ref(geometry));
        let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
        unsafe {
            loader.get_acceleration_structure_build_sizes(
                vk::AccelerationStructureBuildTypeKHR::DEVICE,
                &info,
                &[count],
                &mut sizes,
            );
        }
        sizes
    }
    fn alignment(ctx: &ContextVk) -> u64 {
        let mut props = vk::PhysicalDeviceAccelerationStructurePropertiesKHR::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut props);
        unsafe {
            ctx.instance
                .get_physical_device_properties2(ctx.physical, &mut properties);
        }
        props.min_acceleration_structure_scratch_offset_alignment as u64
    }
    fn new(
        ctx: Arc<ContextVk>,
        loader: &ash::khr::acceleration_structure::Device,
        ty: vk::AccelerationStructureTypeKHR,
        geometry: &vk::AccelerationStructureGeometryKHR<'_>,
        count: u32,
    ) -> Result<Self> {
        let sizes = Self::sizes(loader, ty, geometry, count);
        let alignment = Self::alignment(&ctx);
        let storage = Buffer::new(
            ctx.clone(),
            sizes.acceleration_structure_size.max(256) as usize,
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR
                | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            false,
        )?;
        let scratch = Buffer::new(
            ctx.clone(),
            (sizes.build_scratch_size.max(sizes.update_scratch_size) + alignment).max(256) as usize,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            false,
        )?;
        let handle = unsafe {
            loader.create_acceleration_structure(
                &vk::AccelerationStructureCreateInfoKHR::default()
                    .buffer(storage.handle)
                    .size(sizes.acceleration_structure_size)
                    .ty(ty),
                None,
            )
        }?;
        let scratch_address = scratch.address().next_multiple_of(alignment);
        Ok(Self {
            _ctx: ctx,
            loader: loader.clone(),
            handle,
            _storage: Some(storage),
            _scratch: Some(scratch),
            scratch_address,
        })
    }
    fn build_info<'a>(
        &self,
        ty: vk::AccelerationStructureTypeKHR,
        geometry: &'a vk::AccelerationStructureGeometryKHR<'a>,
        update: bool,
    ) -> vk::AccelerationStructureBuildGeometryInfoKHR<'a> {
        vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(ty)
            .flags(
                vk::BuildAccelerationStructureFlagsKHR::ALLOW_UPDATE
                    | vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE,
            )
            .mode(if update {
                vk::BuildAccelerationStructureModeKHR::UPDATE
            } else {
                vk::BuildAccelerationStructureModeKHR::BUILD
            })
            .src_acceleration_structure(if update {
                self.handle
            } else {
                vk::AccelerationStructureKHR::null()
            })
            .dst_acceleration_structure(self.handle)
            .geometries(std::slice::from_ref(geometry))
            .scratch_data(vk::DeviceOrHostAddressKHR {
                device_address: self.scratch_address,
            })
    }
    fn address(&self) -> u64 {
        unsafe {
            self.loader.get_acceleration_structure_device_address(
                &vk::AccelerationStructureDeviceAddressInfoKHR::default()
                    .acceleration_structure(self.handle),
            )
        }
    }
    fn record(
        &self,
        cmd: vk::CommandBuffer,
        ty: vk::AccelerationStructureTypeKHR,
        geometry: &vk::AccelerationStructureGeometryKHR<'_>,
        count: u32,
        update: bool,
    ) {
        let info = self.build_info(ty, geometry, update);
        let range = [vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(count)];
        unsafe {
            self.loader
                .cmd_build_acceleration_structures(cmd, &[info], &[&range]);
        }
    }
}
impl Drop for Acceleration {
    fn drop(&mut self) {
        unsafe {
            self.loader
                .destroy_acceleration_structure(self.handle, None);
        }
    }
}
pub struct RayTracing {
    ctx: Arc<ContextVk>,
    positions: Buffer,
    instances: Buffer,
    blas: Vec<Acceleration>,
    tlas: Acceleration,
    _blas_storage: Buffer,
    _blas_scratch: Buffer,
    pub built: bool,
    pub memory_bytes: usize,
}
impl RayTracing {
    pub(crate) fn new(ctx: Arc<ContextVk>, scene: &MeshScene) -> Result<Self> {
        let loader = ash::khr::acceleration_structure::Device::new(&ctx.instance, &ctx.device);
        let usage = vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
            | vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR;
        let positions = Buffer::new(
            ctx.clone(),
            (scene.triangles.len() * 48).max(48),
            usage,
            true,
        )?;
        let instances = Buffer::new(ctx.clone(), (scene.objects.len() * 64).max(64), usage, true)?;
        let alignment = Acceleration::alignment(&ctx);
        let mut offsets = Vec::new();
        let mut storage_size = 0u64;
        let mut scratch_size = 0u64;
        for object in &scene.objects {
            let geometry = triangle_geometry(
                positions.address() + object.info[0] as u64 * 48,
                object.info[1],
                object.info[2] == 0,
            );
            let sizes = Acceleration::sizes(
                &loader,
                vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL,
                &geometry,
                object.info[1],
            );
            storage_size = storage_size.next_multiple_of(256);
            scratch_size = scratch_size.next_multiple_of(alignment);
            offsets.push((
                storage_size,
                scratch_size,
                sizes.acceleration_structure_size,
            ));
            storage_size += sizes.acceleration_structure_size.max(256);
            scratch_size += sizes
                .build_scratch_size
                .max(sizes.update_scratch_size)
                .max(256);
        }
        // Separate aligned slices permit all independent BLAS builds in one batch.
        let blas_storage = Buffer::new(
            ctx.clone(),
            storage_size.max(256) as usize,
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR
                | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            false,
        )?;
        let blas_scratch = Buffer::new(
            ctx.clone(),
            (scratch_size + alignment).max(256) as usize,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            false,
        )?;
        let scratch_base = blas_scratch.address().next_multiple_of(alignment);
        let mut blas = Vec::new();
        for (offset, scratch_offset, size) in offsets {
            let handle = unsafe {
                loader.create_acceleration_structure(
                    &vk::AccelerationStructureCreateInfoKHR::default()
                        .buffer(blas_storage.handle)
                        .offset(offset)
                        .size(size)
                        .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL),
                    None,
                )
            }?;
            blas.push(Acceleration {
                _ctx: ctx.clone(),
                loader: loader.clone(),
                handle,
                _storage: None,
                _scratch: None,
                scratch_address: scratch_base + scratch_offset,
            });
        }
        let geometry = instance_geometry(instances.address());
        let tlas = Acceleration::new(
            ctx.clone(),
            &loader,
            vk::AccelerationStructureTypeKHR::TOP_LEVEL,
            &geometry,
            scene.objects.len() as u32,
        )?;
        let memory_bytes = positions.size
            + instances.size
            + blas_storage.size
            + blas_scratch.size
            + tlas._storage.as_ref().unwrap().size
            + tlas._scratch.as_ref().unwrap().size;
        let mut result = Self {
            ctx,
            positions,
            instances,
            blas,
            tlas,
            _blas_storage: blas_storage,
            _blas_scratch: blas_scratch,
            built: false,
            memory_bytes,
        };
        result.upload(scene);
        Ok(result)
    }
    pub fn handle(&self) -> vk::AccelerationStructureKHR {
        self.tlas.handle
    }
    pub fn upload(&mut self, scene: &MeshScene) {
        self.positions
            .write(bytemuck::cast_slice(&scene.positions()));
        let instances: Vec<_> = scene
            .objects
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let matrix = o.world;
                let mut transform = [0.0; 12];
                for row in 0..3 {
                    for col in 0..4 {
                        transform[row * 4 + col] = matrix[col][row];
                    }
                }
                vk::AccelerationStructureInstanceKHR {
                    transform: vk::TransformMatrixKHR { matrix: transform },
                    instance_custom_index_and_mask: vk::Packed24_8::new(
                        i as u32,
                        if o.info[3]&crate::entities::INACTIVE!=0 {0} else if o.info[2] == 0 { 1 } else { 2 },
                    ),
                    instance_shader_binding_table_record_offset_and_flags: vk::Packed24_8::new(
                        0,
                        vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE.as_raw() as u8,
                    ),
                    acceleration_structure_reference: vk::AccelerationStructureReferenceKHR {
                        device_handle: self.blas[i].address(),
                    },
                }
            })
            .collect();
        // Vulkan instance ABI is exactly 64 bytes; all fields above are initialized.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                instances.as_ptr().cast::<u8>(),
                instances.len() * std::mem::size_of::<vk::AccelerationStructureInstanceKHR>(),
            )
        };
        self.instances.write(bytes);
    }
    pub fn record_blas(&self, cmd: vk::CommandBuffer, scene: &MeshScene, rebuild: bool) {
        let indices: Vec<_> = scene
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| !self.built || (o.info[2] == 0 && o.info[3]&crate::entities::INACTIVE==0))
            .map(|(i, _)| i)
            .collect();
        self.record_blas_batch(cmd, scene, rebuild, &indices);
    }
    pub fn record_blas_batch(
        &self,
        cmd: vk::CommandBuffer,
        scene: &MeshScene,
        rebuild: bool,
        indices: &[usize],
    ) {
        unsafe {
            let host = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::HOST_WRITE)
                .dst_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR);
            self.ctx.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::HOST,
                vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR,
                vk::DependencyFlags::empty(),
                &[host],
                &[],
                &[],
            );
        }
        let selected: Vec<_> = indices.iter().map(|&i| (i, &scene.objects[i])).collect();
        let geometries: Vec<_> = selected
            .iter()
            .map(|(_, o)| {
                triangle_geometry(
                    self.positions.address() + o.info[0] as u64 * 48,
                    o.info[1],
                    o.info[2] == 0,
                )
            })
            .collect();
        let infos: Vec<_> = selected
            .iter()
            .zip(&geometries)
            .map(|((i, _), g)| {
                self.blas[*i].build_info(
                    vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL,
                    g,
                    self.built && !rebuild,
                )
            })
            .collect();
        let ranges: Vec<_> =
            selected
                .iter()
                .map(|(_, o)| {
                    [vk::AccelerationStructureBuildRangeInfoKHR::default()
                        .primitive_count(o.info[1])]
                })
                .collect();
        let range_refs: Vec<_> = ranges.iter().map(|r| r.as_slice()).collect();
        if !infos.is_empty() {
            unsafe {
                self.tlas
                    .loader
                    .cmd_build_acceleration_structures(cmd, &infos, &range_refs);
            }
        }
        self.barrier(
            cmd,
            vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR,
        );
    }
    pub fn record_tlas(&self, cmd: vk::CommandBuffer, scene: &MeshScene) {
        let geometry = instance_geometry(self.instances.address());
        self.tlas.record(
            cmd,
            vk::AccelerationStructureTypeKHR::TOP_LEVEL,
            &geometry,
            scene.objects.len() as u32,
            self.built,
        );
        self.barrier(cmd, vk::PipelineStageFlags::COMPUTE_SHADER);
    }
    fn barrier(&self, cmd: vk::CommandBuffer, dst: vk::PipelineStageFlags) {
        let barrier = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_WRITE_KHR)
            .dst_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR);
        unsafe {
            self.ctx.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR,
                dst,
                vk::DependencyFlags::empty(),
                &[barrier],
                &[],
                &[],
            );
        }
    }
}
fn triangle_geometry(
    address: u64,
    count: u32,
    opaque: bool,
) -> vk::AccelerationStructureGeometryKHR<'static> {
    let triangles = vk::AccelerationStructureGeometryTrianglesDataKHR::default()
        .vertex_format(vk::Format::R32G32B32_SFLOAT)
        .vertex_data(vk::DeviceOrHostAddressConstKHR {
            device_address: address,
        })
        .vertex_stride(16)
        .max_vertex(count * 3 - 1)
        .index_type(vk::IndexType::NONE_KHR);
    vk::AccelerationStructureGeometryKHR::default()
        .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
        .geometry(vk::AccelerationStructureGeometryDataKHR { triangles })
        .flags(if opaque {
            vk::GeometryFlagsKHR::OPAQUE
        } else {
            vk::GeometryFlagsKHR::empty()
        })
}
fn instance_geometry(address: u64) -> vk::AccelerationStructureGeometryKHR<'static> {
    let instances = vk::AccelerationStructureGeometryInstancesDataKHR::default()
        .array_of_pointers(false)
        .data(vk::DeviceOrHostAddressConstKHR {
            device_address: address,
        });
    vk::AccelerationStructureGeometryKHR::default()
        .geometry_type(vk::GeometryTypeKHR::INSTANCES)
        .geometry(vk::AccelerationStructureGeometryDataKHR { instances })
}
