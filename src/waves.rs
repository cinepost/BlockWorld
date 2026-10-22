//! Seamless wave heights/slopes baked once on CPU; no spectrum synthesis per pixel.
use crate::renderer::{Buffer, ContextVk};
use anyhow::Result;
use ash::vk;
use std::sync::Arc;
pub const WAVE_EDGE: usize = 64;
pub const WAVE_MIPS: u32 = 7;
pub fn wave_mips() -> Vec<Vec<u8>> {
    let mut values = vec![[0.0f32; 3]; WAVE_EDGE * WAVE_EDGE];
    for z in 0..WAVE_EDGE {
        for x in 0..WAVE_EDGE {
            let u = x as f32 / WAVE_EDGE as f32;
            let v = z as f32 / WAVE_EDGE as f32;
            for mode in 0..32u32 {
                let hash = mode.wrapping_mul(747796405).wrapping_add(2891336453);
                let kx = (hash % 9) as f32 - 4.0;
                let kz = ((hash >> 8) % 9) as f32 - 4.0;
                let k2 = kx * kx + kz * kz;
                if k2 == 0.0 {
                    continue;
                }
                let amplitude = 1.0 / (k2 * k2 + 1.0).sqrt();
                let phase =
                    std::f32::consts::TAU * (kx * u + kz * v + (hash >> 16) as f32 / 65536.0);
                let value = &mut values[x + z * WAVE_EDGE];
                value[0] += amplitude * kx * phase.cos();
                value[1] += amplitude * kz * phase.cos();
                value[2] += amplitude * phase.sin();
            }
        }
    }
    let slope = values
        .iter()
        .flat_map(|v| v[..2].iter())
        .fold(0.0f32, |a, &b| a.max(b.abs()))
        .max(0.001);
    let height = values
        .iter()
        .map(|v| v[2].abs())
        .fold(0.0f32, f32::max)
        .max(0.001);
    let mut mips = vec![values
        .iter()
        .flat_map(|v| {
            [
                (v[0] / slope * 127.0 + 128.0).round() as u8,
                (v[1] / slope * 127.0 + 128.0).round() as u8,
                (v[2] / height * 127.0 + 128.0).round() as u8,
                255,
            ]
        })
        .collect::<Vec<_>>()];
    let mut edge = WAVE_EDGE;
    while edge > 1 {
        let next = edge / 2;
        let previous = mips.last().unwrap();
        let mut mip = vec![0u8; next * next * 4];
        for y in 0..next {
            for x in 0..next {
                for channel in 0..4 {
                    let mut sum = 0u32;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            sum +=
                                previous[((y * 2 + dy) * edge + x * 2 + dx) * 4 + channel] as u32;
                        }
                    }
                    mip[(y * next + x) * 4 + channel] = sum.div_ceil(4) as u8;
                }
            }
        }
        mips.push(mip);
        edge = next;
    }
    mips
}
pub struct WaveTexture {
    ctx: Arc<ContextVk>,
    image: vk::Image,
    memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    pub anisotropy: f32,
}
impl WaveTexture {
    pub(crate) fn new(
        ctx: Arc<ContextVk>,
        queue: vk::Queue,
        pool: vk::CommandPool,
    ) -> Result<Self> {
        unsafe {
            let d = &ctx.device;
            let image = d.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .extent(vk::Extent3D {
                        width: WAVE_EDGE as u32,
                        height: WAVE_EDGE as u32,
                        depth: 1,
                    })
                    .mip_levels(WAVE_MIPS)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )?;
            let req = d.get_image_memory_requirements(image);
            let props = ctx
                .instance
                .get_physical_device_memory_properties(ctx.physical);
            let memory_type = (0..props.memory_type_count)
                .find(|&i| {
                    req.memory_type_bits & (1 << i) != 0
                        && props.memory_types[i as usize]
                            .property_flags
                            .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
                })
                .expect("Device-local image memory");
            let memory = match d.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(memory_type),
                None,
            ) {
                Ok(m) => m,
                Err(e) => {
                    d.destroy_image(image, None);
                    return Err(e.into());
                }
            };
            let mut texture = Self {
                ctx: ctx.clone(),
                image,
                memory,
                view: vk::ImageView::null(),
                sampler: vk::Sampler::null(),
                anisotropy: 1.0,
            };
            d.bind_image_memory(image, memory, 0)?;
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(WAVE_MIPS)
                .layer_count(1);
            texture.view = d.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .subresource_range(range),
                None,
            )?;
            texture.anisotropy = if ctx.anisotropy {
                ctx.instance
                    .get_physical_device_properties(ctx.physical)
                    .limits
                    .max_sampler_anisotropy
                    .min(8.0)
            } else {
                1.0
            };
            texture.sampler = d.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::REPEAT)
                    .address_mode_v(vk::SamplerAddressMode::REPEAT)
                    .address_mode_w(vk::SamplerAddressMode::REPEAT)
                    .anisotropy_enable(ctx.anisotropy)
                    .max_anisotropy(texture.anisotropy)
                    .max_lod((WAVE_MIPS - 1) as f32),
                None,
            )?;
            let mips = wave_mips();
            let bytes: Vec<_> = mips.iter().flatten().copied().collect();
            let mut staging = Buffer::new(
                ctx.clone(),
                bytes.len(),
                vk::BufferUsageFlags::TRANSFER_SRC,
                true,
            )?;
            staging.write(&bytes);
            let command = d.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            let fence = d.create_fence(&vk::FenceCreateInfo::default(), None)?;
            let upload = (|| -> Result<()> {
                d.begin_command_buffer(
                    command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )?;
                let transition = vk::ImageMemoryBarrier::default()
                    .image(image)
                    .subresource_range(range)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
                d.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[transition],
                );
                let mut offset = 0u64;
                let copies: Vec<_> = mips
                    .iter()
                    .enumerate()
                    .map(|(level, data)| {
                        let size = (WAVE_EDGE >> level) as u32;
                        let copy = vk::BufferImageCopy::default()
                            .buffer_offset(offset)
                            .image_subresource(
                                vk::ImageSubresourceLayers::default()
                                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                                    .mip_level(level as u32)
                                    .layer_count(1),
                            )
                            .image_extent(vk::Extent3D {
                                width: size,
                                height: size,
                                depth: 1,
                            });
                        offset += data.len() as u64;
                        copy
                    })
                    .collect();
                d.cmd_copy_buffer_to_image(
                    command,
                    staging.handle,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &copies,
                );
                let ready = transition
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ);
                d.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[ready],
                );
                d.end_command_buffer(command)?;
                {
                    let _guard = ctx.queue_lock.lock().unwrap();
                    d.queue_submit(
                        queue,
                        &[vk::SubmitInfo::default().command_buffers(&[command])],
                        fence,
                    )?;
                }
                d.wait_for_fences(&[fence], true, u64::MAX)?;
                Ok(())
            })();
            d.destroy_fence(fence, None);
            d.free_command_buffers(pool, &[command]);
            upload?;
            Ok(texture)
        }
    }
}
impl Drop for WaveTexture {
    fn drop(&mut self) {
        unsafe {
            let d = &self.ctx.device;
            d.destroy_sampler(self.sampler, None);
            d.destroy_image_view(self.view, None);
            d.destroy_image(self.image, None);
            d.free_memory(self.memory, None);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn baked_waves_are_deterministic_and_mipmapped() {
        let a = wave_mips();
        assert_eq!(a, wave_mips());
        assert_eq!(a.len(), 7);
        assert_eq!(a[0].len(), 64 * 64 * 4);
        assert_eq!(a[6].len(), 4);
        assert!((a[6][0] as i32 - 128).abs() <= 3);
        assert!((a[6][1] as i32 - 128).abs() <= 3);
        assert!(a[0].as_chunks::<4>().0.iter().any(|p| p[0] > 170));
    }
}
