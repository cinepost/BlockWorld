//! GPU-only presentation. CPU never maps or reads rendered pixels in window mode.
use crate::renderer::ContextVk;
use anyhow::{anyhow, bail, Result};
use ash::vk;
use std::sync::Arc;
use winit::window::Window;

pub struct Presentation {
    ctx: Arc<ContextVk>,
    window: Arc<Window>,
    loader: ash::khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    pub acquired: vk::Semaphore,
    pub finished: Vec<vk::Semaphore>,
    render_pass: vk::RenderPass,
    format: vk::Format,
    extent: vk::Extent2D,
    gui: Option<imgui_rs_vulkan_renderer::Renderer>,
    recreate: bool,
    pub mode: vk::PresentModeKHR,
}
impl Presentation {
    pub(crate) fn new(
        ctx: Arc<ContextVk>,
        _queue: vk::Queue,
        _pool: vk::CommandPool,
        window: Arc<Window>,
    ) -> Result<Self> {
        let loader = ash::khr::swapchain::Device::new(&ctx.instance, &ctx.device);
        let mut p = Self {
            ctx,
            window,
            loader,
            swapchain: vk::SwapchainKHR::null(),
            images: vec![],
            views: vec![],
            framebuffers: vec![],
            acquired: vk::Semaphore::null(),
            finished: vec![],
            render_pass: vk::RenderPass::null(),
            format: vk::Format::UNDEFINED,
            extent: vk::Extent2D::default(),
            gui: None,
            recreate: true,
            mode: vk::PresentModeKHR::FIFO,
        };
        p.init()?;
        Ok(p)
    }
    fn init(&mut self) -> Result<()> {
        let (surface, handle) = self.ctx.surface.as_ref().unwrap();
        let formats =
            unsafe { surface.get_physical_device_surface_formats(self.ctx.physical, *handle) }?;
        self.format = formats
            .iter()
            .find(|f| f.format == vk::Format::B8G8R8A8_UNORM)
            .or_else(|| {
                formats
                    .iter()
                    .find(|f| f.format == vk::Format::B8G8R8A8_SRGB)
            })
            .ok_or_else(|| anyhow!("Surface requires BGRA8 UNORM or SRGB"))?
            .format;
        let d = &self.ctx.device;
        unsafe {
            self.acquired = d.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
            let attachments = [vk::AttachmentDescription::default()
                .format(self.format)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::LOAD)
                .store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];
            let refs = [vk::AttachmentReference::default()
                .attachment(0)
                .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
            let subpasses = [vk::SubpassDescription::default()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(&refs)];
            self.render_pass = d.create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(&attachments)
                    .subpasses(&subpasses),
                None,
            )?;
        }
        self.resize()?;
        Ok(())
    }
    pub fn init_imgui(
        &mut self,
        queue: vk::Queue,
        pool: vk::CommandPool,
        imgui: &mut imgui::Context,
    ) -> Result<()> {
        self.gui = Some(imgui_rs_vulkan_renderer::Renderer::with_default_allocator(
            &self.ctx.instance,
            self.ctx.physical,
            self.ctx.device.clone(),
            queue,
            pool,
            self.render_pass,
            imgui,
            Some(imgui_rs_vulkan_renderer::Options {
                in_flight_frames: 1,
                ..Default::default()
            }),
        )?);
        Ok(())
    }
    fn clear_swapchain(&mut self) {
        unsafe {
            let d = &self.ctx.device;
            for f in self.framebuffers.drain(..) {
                d.destroy_framebuffer(f, None);
            }
            for v in self.views.drain(..) {
                d.destroy_image_view(v, None);
            }
            for s in self.finished.drain(..) {
                d.destroy_semaphore(s, None);
            }
            self.loader.destroy_swapchain(self.swapchain, None);
            self.swapchain = vk::SwapchainKHR::null();
        }
    }
    fn resize(&mut self) -> Result<()> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        self.ctx.wait_idle()?;
        self.clear_swapchain();
        let (surface, handle) = self.ctx.surface.as_ref().unwrap();
        let caps = unsafe {
            surface.get_physical_device_surface_capabilities(self.ctx.physical, *handle)
        }?;
        let usage = vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::COLOR_ATTACHMENT;
        if !caps.supported_usage_flags.contains(usage) {
            bail!("Swapchain requires transfer-destination and color-attachment support");
        }
        self.extent = if caps.current_extent.width != u32::MAX {
            caps.current_extent
        } else {
            vk::Extent2D {
                width: size
                    .width
                    .clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                height: size
                    .height
                    .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
            }
        };
        let formats =
            unsafe { surface.get_physical_device_surface_formats(self.ctx.physical, *handle) }?;
        let format = formats
            .iter()
            .find(|f| f.format == self.format)
            .ok_or_else(|| anyhow!("Surface format changed"))?;
        let modes = unsafe {
            surface.get_physical_device_surface_present_modes(self.ctx.physical, *handle)
        }?;
        self.mode = if std::env::var_os("BLOCKWORLD_VSYNC").is_some() {
            vk::PresentModeKHR::FIFO
        } else if modes.contains(&vk::PresentModeKHR::MAILBOX) {
            vk::PresentModeKHR::MAILBOX
        } else if modes.contains(&vk::PresentModeKHR::IMMEDIATE) {
            vk::PresentModeKHR::IMMEDIATE
        } else {
            vk::PresentModeKHR::FIFO
        };
        let mut count = caps.min_image_count + 1;
        if caps.max_image_count > 0 {
            count = count.min(caps.max_image_count);
        }
        let composite = [
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::INHERIT,
        ]
        .into_iter()
        .find(|f| caps.supported_composite_alpha.contains(*f))
        .unwrap();
        unsafe {
            self.swapchain = self.loader.create_swapchain(
                &vk::SwapchainCreateInfoKHR::default()
                    .surface(*handle)
                    .min_image_count(count)
                    .image_format(self.format)
                    .image_color_space(format.color_space)
                    .image_extent(self.extent)
                    .image_array_layers(1)
                    .image_usage(usage)
                    .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .pre_transform(caps.current_transform)
                    .composite_alpha(composite)
                    .present_mode(self.mode)
                    .clipped(true),
                None,
            )?;
            self.images = self.loader.get_swapchain_images(self.swapchain)?;
            for &image in &self.images {
                self.finished.push(
                    self.ctx
                        .device
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?,
                );
                let view = self.ctx.device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(self.format)
                        .subresource_range(color_range()),
                    None,
                )?;
                self.views.push(view);
                self.framebuffers.push(
                    self.ctx.device.create_framebuffer(
                        &vk::FramebufferCreateInfo::default()
                            .render_pass(self.render_pass)
                            .attachments(&[view])
                            .width(self.extent.width)
                            .height(self.extent.height)
                            .layers(1),
                        None,
                    )?,
                );
            }
        }
        self.recreate = false;
        Ok(())
    }
    pub fn extent(&self) -> (u32, u32) {
        (self.extent.width, self.extent.height)
    }
    pub fn acquire(&mut self) -> Result<Option<u32>> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(None);
        }
        if self.recreate || (size.width, size.height) != (self.extent.width, self.extent.height) {
            self.resize()?;
        }
        match unsafe {
            self.loader.acquire_next_image(
                self.swapchain,
                u64::MAX,
                self.acquired,
                vk::Fence::null(),
            )
        } {
            Ok((i, suboptimal)) => {
                self.recreate |= suboptimal;
                Ok(Some(i))
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.recreate = true;
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }
    /// Called after shading in the same command buffer. No GPU → CPU transfer.
    pub fn record(
        &mut self,
        cmd: vk::CommandBuffer,
        index: u32,
        output: vk::Buffer,
        draw: Option<&imgui::DrawData>,
    ) -> Result<()> {
        let d = &self.ctx.device;
        let target = self.images[index as usize];
        unsafe {
            let buffer = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
            d.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[buffer],
                &[],
                &[],
            );
            // Render buffers exactly match the acquired surface extent: no scaling/blit.
            transition(
                d,
                cmd,
                target,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
            );
            let layers = vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1);
            let copy = vk::BufferImageCopy::default()
                .image_subresource(layers)
                .image_extent(vk::Extent3D {
                    width: self.extent.width,
                    height: self.extent.height,
                    depth: 1,
                });
            d.cmd_copy_buffer_to_image(
                cmd,
                output,
                target,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[copy],
            );
            transition(
                d,
                cmd,
                target,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            );
            d.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(self.framebuffers[index as usize])
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D::default(),
                        extent: self.extent,
                    }),
                vk::SubpassContents::INLINE,
            );
            if let (Some(gui), Some(draw)) = (&mut self.gui, draw) {
                gui.cmd_draw(cmd, draw)?;
            }
            d.cmd_end_render_pass(cmd);
        }
        Ok(())
    }
    pub fn present(&mut self, queue: vk::Queue, index: u32) -> Result<()> {
        self.window.pre_present_notify();
        let waits = [self.finished[index as usize]];
        let chains = [self.swapchain];
        let indices = [index];
        match unsafe {
            self.loader.queue_present(
                queue,
                &vk::PresentInfoKHR::default()
                    .wait_semaphores(&waits)
                    .swapchains(&chains)
                    .image_indices(&indices),
            )
        } {
            Ok(suboptimal) => {
                self.recreate |= suboptimal;
                Ok(())
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.recreate = true;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
}
fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}
#[allow(clippy::too_many_arguments)]
unsafe fn transition(
    d: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
    src: vk::PipelineStageFlags,
    dst: vk::PipelineStageFlags,
    read: vk::AccessFlags,
    write: vk::AccessFlags,
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .old_layout(old)
        .new_layout(new)
        .src_access_mask(read)
        .dst_access_mask(write)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(color_range());
    d.cmd_pipeline_barrier(
        cmd,
        src,
        dst,
        vk::DependencyFlags::empty(),
        &[],
        &[],
        &[barrier],
    );
}
impl Drop for Presentation {
    fn drop(&mut self) {
        unsafe {
            let _ = self.ctx.device.device_wait_idle();
            self.gui.take();
            self.clear_swapchain();
            let d = &self.ctx.device;
            d.destroy_render_pass(self.render_pass, None);
            d.destroy_semaphore(self.acquired, None);
        }
    }
}
