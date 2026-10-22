//! Small GPU-only startup presenter, independent of world/render pipeline setup.
use crate::{
    presentation::Presentation,
    renderer::{Buffer, ContextVk},
};
use anyhow::Result;
use ash::vk;
use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Instant,
};
use winit::window::Window;
pub const IMAGE_WIDTH: u32 = 1672;
pub const IMAGE_HEIGHT: u32 = 941;
pub struct LoadingScreen {
    ctx: Arc<ContextVk>,
    presentation: Option<Presentation>,
    image: Option<Buffer>,
    output: Option<Buffer>,
    extent: (u32, u32),
    pool: vk::CommandPool,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    descriptors: vk::DescriptorPool,
    set_layout: vk::DescriptorSetLayout,
    set: vk::DescriptorSet,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    pub progress: Arc<AtomicU32>,
    started: Instant,
    shown: bool,
}
impl LoadingScreen {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let started = Instant::now();
        let ctx = ContextVk::create(Some(window.clone()))?;
        let mut s = Self {
            ctx,
            presentation: None,
            image: None,
            output: None,
            extent: (0, 0),
            pool: vk::CommandPool::null(),
            command: vk::CommandBuffer::null(),
            fence: vk::Fence::null(),
            descriptors: vk::DescriptorPool::null(),
            set_layout: vk::DescriptorSetLayout::null(),
            set: vk::DescriptorSet::null(),
            layout: vk::PipelineLayout::null(),
            pipeline: vk::Pipeline::null(),
            progress: Arc::new(AtomicU32::new(0)),
            started,
            shown: false,
        };
        let d = &s.ctx.device;
        unsafe {
            s.pool = d.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(s.ctx.family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            s.command = d.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(s.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            s.fence = d.create_fence(&vk::FenceCreateInfo::default(), None)?;
            let bindings = [0, 1].map(|binding| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE)
            });
            s.set_layout = d.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            s.descriptors = d.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&[vk::DescriptorPoolSize::default()
                        .ty(vk::DescriptorType::STORAGE_BUFFER)
                        .descriptor_count(2)]),
                None,
            )?;
            s.set = d.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(s.descriptors)
                    .set_layouts(&[s.set_layout]),
            )?[0];
            s.layout = d.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&[s.set_layout])
                    .push_constant_ranges(&[vk::PushConstantRange::default()
                        .stage_flags(vk::ShaderStageFlags::COMPUTE)
                        .size(32)]),
                None,
            )?;
            let code = ash::util::read_spv(&mut std::io::Cursor::new(include_bytes!(concat!(
                env!("OUT_DIR"),
                "/loading.spv"
            ))))?;
            let module =
                d.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None)?;
            let result = d.create_compute_pipelines(
                vk::PipelineCache::null(),
                &[vk::ComputePipelineCreateInfo::default()
                    .layout(s.layout)
                    .stage(
                        vk::PipelineShaderStageCreateInfo::default()
                            .stage(vk::ShaderStageFlags::COMPUTE)
                            .module(module)
                            .name(c"main"),
                    )],
                None,
            );
            d.destroy_shader_module(module, None);
            s.pipeline = result.map_err(|(_, e)| e)?[0];
        }
        let bytes = include_bytes!("../assets/loading_screen.bgra");
        anyhow::ensure!(
            bytes.len() == IMAGE_WIDTH as usize * IMAGE_HEIGHT as usize * 4,
            "Invalid loading asset dimensions"
        );
        let mut image = Buffer::new(
            s.ctx.clone(),
            bytes.len(),
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        image.write(bytes);
        s.image = Some(image);
        let queue = unsafe { s.ctx.device.get_device_queue(s.ctx.family, 0) };
        window.set_visible(true);
        s.presentation = Some(Presentation::new(s.ctx.clone(), queue, s.pool, window)?);
        Ok(s)
    }
    pub fn context(&self) -> Arc<ContextVk> {
        self.ctx.clone()
    }
    pub fn finish(&mut self) -> Presentation {
        self.presentation.take().unwrap()
    }
    pub fn draw(&mut self) -> Result<()> {
        let p = self.presentation.as_mut().unwrap();
        let Some(index) = p.acquire()? else {
            return Ok(());
        };
        let extent = p.extent();
        if extent != self.extent {
            let output = Buffer::new(
                self.ctx.clone(),
                extent.0 as usize * extent.1 as usize * 4,
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
                false,
            )?;
            self.output = Some(output);
            self.extent = extent;
            let infos = [
                [self.image.as_ref().unwrap().info()],
                [self.output.as_ref().unwrap().info()],
            ];
            let writes: Vec<_> = infos
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.set)
                        .dst_binding(i as u32)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .buffer_info(b)
                })
                .collect();
            unsafe {
                self.ctx.device.update_descriptor_sets(&writes, &[]);
            }
        }
        let d = &self.ctx.device;
        let queue = unsafe { d.get_device_queue(self.ctx.family, 0) };
        unsafe {
            d.reset_command_buffer(self.command, vk::CommandBufferResetFlags::empty())?;
            d.begin_command_buffer(
                self.command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            d.cmd_bind_pipeline(self.command, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            d.cmd_bind_descriptor_sets(
                self.command,
                vk::PipelineBindPoint::COMPUTE,
                self.layout,
                0,
                &[self.set],
                &[],
            );
            let constants = [
                extent.0,
                extent.1,
                IMAGE_WIDTH,
                IMAGE_HEIGHT,
                (self.progress.load(Ordering::Relaxed).min(100) as f32 / 100.0).to_bits(),
                0,
                0,
                0,
            ];
            d.cmd_push_constants(
                self.command,
                self.layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytemuck::cast_slice(&constants),
            );
            d.cmd_dispatch(self.command, extent.0.div_ceil(8), extent.1.div_ceil(8), 1);
            p.record(
                self.command,
                index,
                self.output.as_ref().unwrap().handle,
                None,
            )?;
            d.end_command_buffer(self.command)?;
            let waits = [p.acquired];
            let signals = [p.finished[index as usize]];
            let commands = [self.command];
            let submit = vk::SubmitInfo::default()
                .wait_semaphores(&waits)
                .wait_dst_stage_mask(&[vk::PipelineStageFlags::TRANSFER])
                .command_buffers(&commands)
                .signal_semaphores(&signals);
            {
                let _guard = self.ctx.queue_lock.lock().unwrap();
                d.queue_submit(queue, &[submit], self.fence)?;
                p.present(queue, index)?;
            }
            d.wait_for_fences(&[self.fence], true, u64::MAX)?;
            d.reset_fences(&[self.fence])?;
        }
        if !self.shown {
            println!(
                "Loading screen presented in {:.1} ms (before world preparation)",
                self.started.elapsed().as_secs_f64() * 1000.0
            );
            self.shown = true;
        }
        Ok(())
    }
}
impl Drop for LoadingScreen {
    fn drop(&mut self) {
        let _ = self.ctx.wait_idle();
        unsafe {
            let d = &self.ctx.device;
            d.destroy_pipeline(self.pipeline, None);
            d.destroy_pipeline_layout(self.layout, None);
            d.destroy_descriptor_pool(self.descriptors, None);
            d.destroy_descriptor_set_layout(self.set_layout, None);
            d.destroy_fence(self.fence, None);
            d.destroy_command_pool(self.pool, None);
        }
    }
}
