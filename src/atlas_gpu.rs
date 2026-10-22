//! Device-local atlas image, staged once and exposed through the descriptor table.
use crate::renderer::{Buffer, ContextVk};
use anyhow::Result;
use ash::vk;
use std::sync::Arc;
pub struct AtlasTexture {
    ctx: Arc<ContextVk>,
    image: vk::Image,
    memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
}
impl AtlasTexture {
    pub(crate) fn new(
        ctx: Arc<ContextVk>,
        queue: vk::Queue,
        pool: vk::CommandPool,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<Self> {
        let limits = unsafe { ctx.instance.get_physical_device_properties(ctx.physical) }.limits;
        anyhow::ensure!(
            width <= limits.max_image_dimension2_d && height <= limits.max_image_dimension2_d,
            "Atlas exceeds GPU image dimensions"
        );
        anyhow::ensure!(
            rgba.len() == width as usize * height as usize * 4,
            "Invalid RGBA atlas byte count"
        );
        unsafe {
            let d = &ctx.device;
            let image = d.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })
                    .mip_levels(1)
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
            };
            d.bind_image_memory(image, memory, 0)?;
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            texture.view = d.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .subresource_range(range),
                None,
            )?;
            texture.sampler = d.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::NEAREST)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    // Preserve min/mag selection with a single atlas level. Zero
                    // forces magnification; .25 selects minFilter but rounds to mip 0.
                    .max_lod(0.25),
                None,
            )?;
            let bytes = rgba;
            let mut staging = Buffer::new(
                ctx.clone(),
                bytes.len(),
                vk::BufferUsageFlags::TRANSFER_SRC,
                true,
            )?;
            staging.write(bytes);
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
                let copies = [vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })];
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
impl Drop for AtlasTexture {
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
