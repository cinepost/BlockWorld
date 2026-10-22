mod audio;
mod atlas_gpu;
mod resource_pack;
mod config;
mod meshes;
mod entities;
mod loading;
mod player;
mod presentation;
mod renderer;
mod rt;
mod sky;
mod smoke;
mod spatial;
mod streaming;
mod lod;
mod waves;
mod world;
mod terrain;
mod caves;

use anyhow::{Context, Result};
use config::*;
use glam::IVec3;
use player::Player;
use renderer::Renderer;
use std::{collections::HashSet, io::Write, sync::Arc, time::Instant};
use streaming::{Completed, Request, Streamer};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};
use world::{terrain_height, Block, CacheStats, World};
type StreamKey = ((i32, i32), i32, i32, u64);

struct Game {
    world: World,
    player: Player,
    streamer: Option<Streamer>,
    renderer: Renderer,
    origin: IVec3,
    center: (i32, i32),
    keys: HashSet<KeyCode>,
    selected: Block,
    debug: u32,
    dirty: bool,
    loaded_view: i32,
    cache_mb: i32,
    cache: CacheStats,
    revision: u64,
    generation: u64,
    world_pending: bool,
    requested: Option<StreamKey>,
    stream_counts: [u64; 3],
    max_sync_ms: f32,
}
impl Game {
    fn new(window: Option<Arc<Window>>) -> Result<Self> {
        Self::build(window,None,&|_| Ok(()))
    }
    fn build(window: Option<Arc<Window>>, context:Option<Arc<renderer::ContextVk>>, progress:&dyn Fn(u32)->Result<()>) -> Result<Self> {
        progress(3)?;
        world::NOISY_WORLD.store(std::env::args().any(|a| a=="--noisy-world"), std::sync::atomic::Ordering::Relaxed);
        let mut world = World::new(42);
        // Start on nearby dry ground rather than dropping directly into the sea.
        let (sx, sz) = (-2048i32..=2048)
            .step_by(16)
            .flat_map(|z| (-2048i32..=2048).step_by(16).map(move |x| (x, z)))
            .filter(|&(x, z)| terrain_height(x, z, world.seed) > SEA_LEVEL + 3 && !terrain::entrance(x,z,world.seed))
            .min_by_key(|&(x, z)| x * x + z * z)
            .unwrap_or((0, 0));
        let mut center = (sx.div_euclid(EDGE as i32), sz.div_euclid(EDGE as i32));
        if context.is_some() {anyhow::ensure!(world.load_radius_progress(center.0,center.1,LOAD_RADIUS,|done,total| progress(5+(done*25/total) as u32).is_ok()),"Startup cancelled");}
        else {world.load_around(center.0,center.1);}
        let mut player =
            Player::new((terrain_height(sx, sz, world.seed) + 1).max(SEA_LEVEL + 1) as f32 + 0.05);
        player.feet.x = sx as f32 + 0.5;
        player.feet.z = sz as f32 + 0.5;
        // Spawn above tree blocks too, if a generated collection occupies this cell.
        while world.get(player.feet.floor().as_ivec3()).solid() {
            player.feet.y += 1.0;
        }
        let (width, height) = window
            .as_ref()
            .map(|w| {
                let s = w.inner_size();
                (s.width.max(1), s.height.max(1))
            })
            .unwrap_or((RENDER_WIDTH, RENDER_HEIGHT));
        let interactive = window.is_some();
        progress(32)?;
        let mut renderer = if let Some(ctx)=context {Renderer::from_context(width,height,ctx,false)?} else {Renderer::new(width,height,window)?};
        progress(55)?;
        let args: Vec<_> = std::env::args().collect();
        renderer.settings.vxgi=args.iter().any(|a| a=="--vxgi");
        renderer.settings.gi_temporal = !args.iter().any(|a| a=="--no-gi-temporal");
        renderer.settings.lod_merging = !args.iter().any(|a| a=="--no-lod-merging");
        if let Some(i)=args.iter().position(|a| a=="--texture-filter") {
            renderer.settings.texture_linear=match args.get(i+1).map(String::as_str) {
                Some("nearest")=>false,Some("linear")=>true,_=>anyhow::bail!("--texture-filter requires nearest or linear"),
            };
        }
        if let Some(i)=args.iter().position(|a| a=="--resource-pack") {
            let path=args.get(i+1).context("--resource-pack needs a ZIP path")?;
            renderer.load_resource_pack(std::path::Path::new(path))?;
        }
        if let Some(i)=args.iter().position(|a|a=="--lod-pixels") {
            renderer.settings.lod_pixels=args.get(i+1).context("--lod-pixels needs a number")?.parse()?;
            anyhow::ensure!((1.0..=64.0).contains(&renderer.settings.lod_pixels),"LOD pixels must be 1..64");
        }
        if let Some(i) = args.iter().position(|a| a == "--view-chunks") {
            let view = args
                .get(i + 1)
                .context("--view-chunks needs a number")?
                .parse::<i32>()
                .context("Invalid chunk distance")?;
            anyhow::ensure!(
                (1..=renderer.max_view_chunks()).contains(&view),
                "View distance must be 1..={} chunks",
                renderer.max_view_chunks()
            );
            renderer.settings.view_chunks = view;
            world.load_radius(center.0, center.1, view.min(NEAR_VIEW_CHUNKS) + 1);
        }
        if args
            .iter()
            .any(|a| a == "--water-view" || a == "--underwater-view")
        {
            let lake = (-64i32..=64)
                .flat_map(|z| (-64i32..=64).map(move |x| (x, z)))
                .filter(|&(x, z)| terrain_height(x, z, world.seed) <= SEA_LEVEL - 2)
                .min_by_key(|&(x, z)| x * x + z * z)
                .unwrap_or((0, 0));
            player.feet = glam::Vec3::new(
                lake.0 as f32 + 0.5,
                SEA_LEVEL as f32 + 2.0,
                lake.1 as f32 + 0.5,
            );
            player.yaw = 0.0;
            player.pitch = -0.25;
            if args.iter().any(|a| a == "--underwater-view") {
                player.feet.y = (SEA_LEVEL - 2).max(terrain_height(lake.0, lake.1, world.seed) + 1)
                    as f32
                    + 0.05;
                player.pitch = 0.65;
            }
            center = (lake.0.div_euclid(16), lake.1.div_euclid(16));
            world.load_radius(center.0, center.1, renderer.settings.view_chunks.min(NEAR_VIEW_CHUNKS) + 1);
        }
        if let Some(i)=args.iter().position(|a|a=="--biome") {
            let name=args.get(i+1).context("--biome needs a name (use quotes for deep ocean / jagged peaks)")?;
            let biome=terrain::Biome::ALL.into_iter().find(|b|b.name()==name.replace('-'," ")).context("Unknown biome")?;
            let (x,z)=(-8192i32..=8192).step_by(32).flat_map(|z|(-8192i32..=8192).step_by(32).map(move |x|(x,z)))
                .filter(|&(x,z)|terrain::sample(x,z,world.seed).biome==biome).min_by_key(|&(x,z)|x*x+z*z).context("Biome not found in search radius")?;
            player.feet=glam::Vec3::new(x as f32+0.5,terrain_height(x,z,world.seed).max(SEA_LEVEL) as f32+18.0,z as f32+0.5);
            player.yaw=0.0;player.pitch= -0.3;
            center=(x.div_euclid(16),z.div_euclid(16));
            world.load_radius(center.0,center.1,renderer.settings.view_chunks.min(NEAR_VIEW_CHUNKS)+1);
        }
        if args.iter().any(|a| a == "--mountain-view") {
            let peak = (-2048i32..=2048)
                .step_by(16)
                .flat_map(|z| (-2048i32..=2048).step_by(16).map(move |x| (x, z)))
                .filter(|&(x, z)| terrain_height(x, z, world.seed) >= SNOW_LEVEL + 15)
                .min_by_key(|&(x, z)| x * x + z * z)
                .context("No snowy mountain found for this seed")?;
            let summit = glam::Vec3::new(
                peak.0 as f32 + 0.5,
                terrain_height(peak.0, peak.1, world.seed) as f32,
                peak.1 as f32 + 0.5,
            );
            player.feet = summit + glam::Vec3::new(45.0, 20.0, 55.0);
            player.feet.y = player.feet.y.max(
                terrain_height(player.feet.x as i32, player.feet.z as i32, world.seed) as f32 + 5.0,
            );
            let direction = (summit - player.eye()).normalize();
            player.yaw = direction.x.atan2(-direction.z);
            player.pitch = direction.y.asin();
            center = (
                (player.feet.x as i32).div_euclid(16),
                (player.feet.z as i32).div_euclid(16),
            );
            world.load_radius(center.0, center.1, renderer.settings.view_chunks.min(NEAR_VIEW_CHUNKS) + 1);
        }
        if args.iter().any(|a| a == "--noisy-world") {
            player.feet.y=72.0;player.pitch= -0.35;
        }
        if args.iter().any(|a| a == "--sky-view") {
            player.feet.y = MAX_Y as f32 + 8.0;
            player.pitch = std::f32::consts::FRAC_PI_2;
        }
        if args.iter().any(|a| a == "--ground-sky-view") {
            player.pitch = std::f32::consts::FRAC_PI_2;
        }
        if args.iter().any(|a| a == "--ground-view") {
            let x = player.feet.x.floor() as i32;
            let z = player.feet.z.floor() as i32;
            let mut chosen = None;
            for dz in -8..=8 {
                for dx in -8..=8 {
                    let p = IVec3::new(x + dx, terrain_height(x + dx, z + dz, world.seed), z + dz);
                    if p.x.rem_euclid(3) != 0
                        && world.get(p) == Block::Grass
                        && world.get(p + IVec3::Y) == Block::Air
                        && chosen.is_none_or(|(_, distance)| dx * dx + dz * dz < distance)
                    {
                        chosen = Some((p, dx * dx + dz * dz));
                    }
                }
            }
            let (p, _) = chosen.context("Find unobstructed ground profile surface")?;
            player.feet =
                glam::Vec3::new(p.x as f32 + 0.5, p.y as f32 + 1.05 - 1.62, p.z as f32 + 0.5);
            player.pitch = -std::f32::consts::FRAC_PI_2;
        }
        if args.iter().any(|a| a == "--block-view") {
            let mut block = IVec3::new(
                player.feet.x.floor() as i32,
                100,
                player.feet.z.floor() as i32,
            );
            if block.x.rem_euclid(3) == 0 {
                block.x += 1;
            }
            player.feet.x = block.x as f32 + 0.5;
            world.set(block, Block::Grass);
            player.feet.y = 101.05 - 1.62;
            player.pitch = -std::f32::consts::FRAC_PI_2;
        }
        progress(65)?;
        let loaded_view = renderer.settings.view_chunks;
        let (origin, blocks) = world.snapshot_radius(center.0, center.1, loaded_view.min(NEAR_VIEW_CHUNKS) + 1);
        renderer.upload(&blocks)?;
        progress(80)?;
        for arg in std::env::args().skip(1) {
            match arg.as_str() {
                "--no-hierarchy" => renderer.settings.hierarchy = false,
                "--no-culling" => renderer.settings.frustum = false,
                "--no-ssao" => renderer.settings.ssao = false,
                "--no-depth-pyramid" => renderer.settings.depth_pyramid = false,
                "--no-csm" => renderer.settings.csm = false,
                "--no-far-shadows" => {
                    renderer.settings.far_shadows = false;
                    renderer.settings.shadow_type = 0;
                }
                "--hard-shadows" => renderer.settings.soft_shadows = false,
                "--no-atmosphere" => renderer.settings.atmosphere = false,
                "--day-cycle" => renderer.settings.day_cycle = true,
                "--no-shadows" => renderer.settings.shadows = false,
                "--opaque-water" => renderer.settings.water = false,
                "--no-ssr" => renderer.settings.ssr = false,
                "--no-caustic-near" => renderer.settings.caustic_near=false,
                "--no-caustic-adaptive" => renderer.settings.caustic_adaptive=false,
                "--no-caustic-shallow" => renderer.settings.caustic_shallow=false,
                "--no-caustics" => renderer.settings.caustics=false,
                "--no-multiscale-waves" => renderer.settings.multiscale_waves=false,
                "--no-wave-warp" => renderer.settings.wave_warp=0.0,
                "--no-stochastic-waves" => renderer.settings.stochastic_waves=false,
                "--no-waves" => renderer.settings.waves = false,
                "--no-meshes" => renderer.settings.meshes = false,
                "--no-rt" => renderer.settings.hardware = false,
                "--freeze-animation" => renderer.settings.animate = false,
                "--rebuild-blas" => renderer.settings.rebuild_blas = true,
                _ => {}
            }
        }
        if let Some(i) = args.iter().position(|a| a == "--csm-resolution") {
            let resolution = args.get(i+1).context("--csm-resolution needs 128, 256, 512, 1024 or 2048")?.parse::<u32>()?;
            anyhow::ensure!([128,256,512,1024,2048].contains(&resolution), "CSM resolution must be 128, 256, 512, 1024 or 2048");
            renderer.settings.csm_resolution = resolution;
        }
        if let Some(i) = args.iter().position(|a| a == "--shadow-type") {
            renderer.settings.shadow_type = match args.get(i + 1).map(String::as_str) {
                Some("ray") => 0,
                Some("hybrid") => 1,
                Some("csm") => 2,
                _ => anyhow::bail!("--shadow-type requires ray, hybrid or csm"),
            };
        }
        if let Some(i) = args.iter().position(|a| a == "--sun-elevation") {
            renderer.settings.sun_elevation = args
                .get(i + 1)
                .context("--sun-elevation needs degrees")?
                .parse::<f32>()?;
            anyhow::ensure!(
                renderer.settings.sun_elevation.is_finite()
                    && renderer.settings.sun_elevation.abs() <= 90.0,
                "Sun elevation must be between -90 and 90"
            );
        }
        if let Some(i) = args.iter().position(|a| a == "--shadow-ray-distance") {
            let distance = args
                .get(i + 1)
                .context("--shadow-ray-distance needs metres")?
                .parse::<f32>()
                .context("Invalid shadow distance")?;
            anyhow::ensure!(
                distance.is_finite() && distance >= 0.0,
                "Shadow distance must be finite and nonnegative"
            );
            renderer.settings.shadow_ray_distance = distance;
        }
        if let Some(i) = args.iter().position(|a| a == "--fixed-time") {
            let time = args
                .get(i + 1)
                .context("--fixed-time needs seconds")?
                .parse::<f32>()
                .context("Invalid fixed time")?;
            anyhow::ensure!(
                time.is_finite() && time >= 0.0,
                "Fixed time must be finite and nonnegative"
            );
            renderer.animation_time = Some(time);
            renderer.water_time = Some(time);
            renderer.settings.animate = false;
        }
        let cache_mb = args
            .iter()
            .position(|a| a == "--cache-mb")
            .map(|i| {
                args.get(i + 1)
                    .context("--cache-mb needs a number")?
                    .parse::<i32>()
                    .context("Invalid cache budget")
            })
            .transpose()?
            .unwrap_or(512)
            .clamp(8, 1024);
        let cache = world.trim_cache(center, loaded_view.min(NEAR_VIEW_CHUNKS) + 1, cache_mb as usize * 1024 * 1024);
        let streamer = if interactive {
            Some(Streamer::new(renderer.stream_context(), world.clone())?)
        } else {
            None
        };
        println!(
            "Vulkan device: {} | background queue: {}",
            renderer.device_name,
            if renderer.has_independent_background_queue() {
                "independent"
            } else {
                "shared"
            }
        );
        progress(90)?;
        Ok(Self {
            world,
            player,
            streamer,
            renderer,
            origin,
            center,
            keys: HashSet::new(),
            selected: Block::Stone,
            debug: 0,
            dirty: false,
            loaded_view,
            cache_mb,
            cache,
            revision: 0,
            generation: 0,
            world_pending: false,
            requested: None,
            stream_counts: [0; 3],
            max_sync_ms: 0.0,
        })
    }
    fn sync_world(&mut self) -> Result<()> {
        entities::set_local(&mut self.renderer.active_players,self.player.feet);
        let p = self.player.feet.floor().as_ivec3();
        let next = (p.x.div_euclid(EDGE as i32), p.z.div_euclid(EDGE as i32));
        let view = self
            .renderer
            .settings
            .view_chunks
            .clamp(1, self.renderer.max_view_chunks());
        self.renderer.settings.view_chunks = view;
        if self.streamer.is_some() {
            let start = Instant::now();
            let result = self.sync_stream(next, view);
            self.renderer.prepare_lods(&self.player, &self.world)?;
            self.max_sync_ms = self.max_sync_ms.max(start.elapsed().as_secs_f32() * 1000.0);
            return result;
        }

        if next != self.center || view != self.loaded_view {
            self.center = next;
            self.world.load_radius(next.0, next.1, view.min(NEAR_VIEW_CHUNKS) + 1);
            self.loaded_view = view;
            self.dirty = true;
        }
        if self.dirty {
            let (origin, blocks) =
                self.world
                    .snapshot_radius(self.center.0, self.center.1, self.loaded_view.min(NEAR_VIEW_CHUNKS) + 1);
            self.origin = origin;
            self.renderer.upload(&blocks)?;
            self.dirty = false;
        }
        self.renderer.prepare_lods(&self.player, &self.world)?;
        Ok(())
    }
    fn sync_stream(&mut self, next: (i32, i32), view: i32) -> Result<()> {
        if self.dirty {
            self.revision += 1;
            self.dirty = false;
        }
        let key = (next, view.min(NEAR_VIEW_CHUNKS), self.cache_mb, self.revision);
        let streamer = self.streamer.as_mut().unwrap();
        while let Some(done) = streamer.poll() {
            match done {
                Completed::World(packet) => {
                    self.world_pending = false;
                    let packet = *packet;
                    if self.requested == Some(key) && packet.request.generation == self.generation {
                        self.stream_counts[0] += 1;
                        self.origin = packet.gpu.origin;
                        self.center = packet.request.center;
                        self.loaded_view = packet.request.view;
                        streamer.retire_cpu(std::mem::replace(&mut self.world, packet.world));
                        self.cache = packet.cache;
                        let retired = self.renderer.adopt_world(packet.gpu);
                        streamer.retire_world(retired);
                        let old = self.renderer.adopt_mesh(packet.frame, packet.ready);
                        streamer.recycle(old);
                    } else {
                        self.stream_counts[1] += 1;
                        streamer.retire_cpu(packet.world);
                        streamer.retire_world(packet.gpu);
                        streamer.recycle(packet.frame);
                    }
                }
                Completed::Animation(Some(packet)) => {
                    let (frame, ready) = *packet;
                    if Some(frame.generation) == self.renderer.async_generation {
                        self.stream_counts[2] += 1;
                        let old = self.renderer.adopt_mesh(frame, ready);
                        streamer.recycle(old);
                    } else {
                        streamer.recycle(frame);
                    }
                }
                Completed::Animation(None) => {}
                Completed::Error(e) => anyhow::bail!("Streaming: {e}"),
            }
        }
        if !self.world_pending && self.requested != Some(key) {
            self.generation += 1;
            self.requested = Some(key);
            self.world_pending = true;
            streamer.request(Request {
                generation: self.generation,
                center: next,
                view: view.min(NEAR_VIEW_CHUNKS),
                budget: self.cache_mb as usize * 1024 * 1024,
                edits: self.world.edits.clone(),
                time: self.renderer.animation_clock(),
                players: self.renderer.active_players.clone(),
            })?;
        }
        if !self.world_pending && self.renderer.settings.meshes && self.renderer.settings.fish {
            if let Some(generation) = self.renderer.async_generation.filter(|g| *g != 0) {
                let time = self.renderer.animation_clock();
                streamer.animate(generation, time, self.renderer.settings.rebuild_blas, self.renderer.active_players.clone())?;
            }
        }
        Ok(())
    }
    fn edit(&mut self, place: bool) {
        if let Some((hit, adjacent)) =
            self.world
                .raycast(self.player.eye(), self.player.forward(), REACH)
        {
            if place {
                if adjacent != hit
                    && !self.player.overlaps(adjacent)
                    && !self.world.get(adjacent).solid()
                {
                    self.dirty |= self.world.set(adjacent, self.selected);
                }
            } else {
                self.dirty |= self.world.destroy(hit) > 0;
            }
        }
    }
}
struct Gui {
    context: imgui::Context,
    platform: imgui_winit_support::WinitPlatform,
}
struct App {
    audio: Option<audio::Audio>,
    audio_error: Option<String>,
    game: Option<Game>,
    loading: Option<loading::LoadingScreen>,
    startup: Option<std::sync::mpsc::Receiver<Result<Game>>>,
    startup_worker: Option<std::thread::JoinHandle<()>>,
    next_loading_redraw: Instant,
    startup_cancel: Arc<std::sync::atomic::AtomicBool>,
    window: Option<Arc<Window>>,
    gui: Option<Gui>,
    captured: bool,
    hud_visible: bool,
    last: Instant,
    frames: u32,
    frame_limit: Option<u32>,
    resize_test: bool,
    stream_test: bool,
    frame_times: Vec<f32>,
    error: Option<anyhow::Error>,
    average_ms: f32,
    total_seconds: f64,
    measured_frames: u32,
    gpu_sums: [f64; 4],
    mesh_sums: [f64; 4],
    cpu_sums: [f64; 2],
    water_sum: f64,
    caustic_sum: f64,
    gi_sum: f64,
}
impl App {
    fn capture(&mut self, capture: bool) {
        if let Some(w) = &self.window {
            if capture {
                self.captured = w
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined))
                    .is_ok();
            } else {
                let _ = w.set_cursor_grab(CursorGrabMode::None);
                self.captured = false;
            }
            w.set_cursor_visible(!self.captured);
        }
    }
    fn draw_loading(&mut self)->Result<()> {
        let w=self.window.as_ref().unwrap().clone();
        if w.inner_size().width==0 || w.inner_size().height==0 {return Ok(());}
        self.loading.as_mut().unwrap().draw()?;
        if self.startup.is_none() {
            let ctx=self.loading.as_ref().unwrap().context();
            let progress=self.loading.as_ref().unwrap().progress.clone();
            let cancelled=self.startup_cancel.clone();
            let (tx,rx)=std::sync::mpsc::sync_channel(1);
            self.startup=Some(rx);
            self.startup_worker=Some(std::thread::Builder::new().name("world-startup".into()).spawn(move || {
                let result=(|| {
                    let mut game=Game::build(Some(w),Some(ctx),&|n| {anyhow::ensure!(!cancelled.load(std::sync::atomic::Ordering::Relaxed),"Startup cancelled");progress.store(n,std::sync::atomic::Ordering::Relaxed);Ok(())})?;
                    progress.store(92,std::sync::atomic::Ordering::Relaxed);
                    // Prepare the initial streamed polygon/AS snapshot before the
                    // handoff, rather than showing an empty mesh scene briefly.
                    game.sync_world()?;
                    while game.stream_counts[0]==0 {
                        anyhow::ensure!(!cancelled.load(std::sync::atomic::Ordering::Relaxed),"Startup cancelled");
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        game.sync_world()?;
                    }
                    progress.store(96,std::sync::atomic::Ordering::Relaxed);
                    game.renderer.warm_up(&game.player,game.origin)?;
                    progress.store(98,std::sync::atomic::Ordering::Relaxed);
                    Ok(game)
                })();
                let _=tx.send(result);
            })?);
        }
        let result=match self.startup.as_ref().unwrap().try_recv() {
            Ok(result)=>result,
            Err(std::sync::mpsc::TryRecvError::Empty)=>return Ok(()),
            Err(std::sync::mpsc::TryRecvError::Disconnected)=>anyhow::bail!("Startup worker stopped unexpectedly"),
        };
        let mut game=result?;
        if let Some(worker)=self.startup_worker.take() {let _=worker.join();}
        self.startup.take();
        if std::env::args().any(|a| a=="--fly") {game.player.toggle_fly();}
        let loading=self.loading.as_mut().unwrap();
        loading.progress.store(100,std::sync::atomic::Ordering::Relaxed);
        loading.draw()?;
        game.renderer.presentation=Some(loading.finish());
        self.loading.take();
        let mut context=imgui::Context::create();context.set_ini_filename(None);
        let mut platform=imgui_winit_support::WinitPlatform::new(&mut context);
        platform.attach_window(context.io_mut(),self.window.as_ref().unwrap(),imgui_winit_support::HiDpiMode::Default);
        game.renderer.init_imgui(&mut context)?;
        if !std::env::args().any(|a| a=="--no-audio") {
            match audio::Audio::start() {
                Ok(audio)=>self.audio=Some(audio),
                Err(error)=>{eprintln!("Audio unavailable: {error:#}");self.audio_error=Some(format!("{error:#}"));}
            }
        }
        self.game=Some(game);self.gui=Some(Gui{context,platform});
        self.last=Instant::now();
        println!("Loading complete; first game frame prepared on GPU");
        Ok(())
    }
    fn draw(&mut self) -> Result<()> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last);
        self.last = now;
        let dt = elapsed.as_secs_f32();
        self.average_ms = if self.frames == 0 {
            dt * 1000.0
        } else {
            self.average_ms * 0.95 + dt * 1000.0 * 0.05
        };
        self.total_seconds += dt as f64;
        let w = self.window.as_ref().unwrap();
        if w.inner_size().width == 0 || w.inner_size().height == 0 {
            return Ok(());
        }
        let game = self.game.as_mut().unwrap();
        if std::env::var_os("BLOCKWORLD_TRACE_FRAMES").is_some() && self.frames.is_multiple_of(30) {eprintln!("Frame {} begin | LOD pages {}",self.frames,game.renderer.lod.as_ref().map_or(0,|l|l.levels.iter().sum::<usize>()));}
        if self.stream_test {
            game.player.yaw += 0.035;
            if self.frames > 0 && self.frames.is_multiple_of(120) {
                game.player.feet.x += 32.0;
                game.player.feet.y = terrain_height(
                    game.player.feet.x as i32,
                    game.player.feet.z as i32,
                    game.world.seed,
                ) as f32
                    + 5.0;
            }
            if self.frames.is_multiple_of(240) {
                game.renderer.settings.view_chunks = if (self.frames / 240).is_multiple_of(2) {
                    game.renderer.max_view_chunks()
                } else {
                    2
                };
                game.cache_mb = 8;
            }
        }
        game.sync_world()?;
        if self.captured {
            let old = game.player.feet;
            game.player.update(&game.world, &game.keys, dt.min(0.05));
            let q = game.player.feet.floor().as_ivec3();
            if !game.player.flying && !game.world.resident(q.x.div_euclid(16), q.z.div_euclid(16)) {
                game.player.feet = old;
            }
        }
        game.sync_world()?;
        let gui = self.gui.as_mut().unwrap();
        gui.context
            .io_mut()
            .update_delta_time(elapsed.max(std::time::Duration::from_micros(1)));
        gui.platform.prepare_frame(gui.context.io_mut(), w)?;
        let ui = gui.context.frame();
        if self.hud_visible {
            ui.window("BlockWorld · Vulkan debug").position([12.0,12.0],imgui::Condition::FirstUseEver).size([440.0,670.0],imgui::Condition::FirstUseEver).build(|| {
            ui.text(format!("{:.1} FPS | {:.2} ms frame",1000.0/self.average_ms.max(0.01),self.average_ms));
            ui.text_wrapped(&game.renderer.device_name);
            if let Some(ms)=game.renderer.gpu_ms {ui.text(format!("GPU vis {:.3} | AO {:.3} | shade {:.3} ms",ms[0],ms[1],ms[2]));}
            ui.text(format!("Depth mips {:.3} | cascade update {:.3} ms",game.renderer.acceleration_ms[0],game.renderer.acceleration_ms[1]));
            ui.text(format!("Cascade builds: voxels {} | meshes {}",game.renderer.cascade_updates[0],game.renderer.cascade_updates[1]));
            ui.text(format!("Far shadow depth builds: {} / {}",game.renderer.far_depth_updates[0],game.renderer.far_depth_updates[1]));
            ui.text(format!("Selected: {}",game.selected.name()));
            if let Some((hit,_))=game.world.raycast(game.player.eye(),game.player.forward(),REACH) {
                if let Some(tree)=game.world.tree_at(hit) {
                    ui.text(format!("Tree {:?} {:?} | root {:?} | {} blocks",tree.kind,tree.id,tree.root,tree.blocks.len()));
                }
            }
            ui.text(format!("Render targets: {} x {}",game.renderer.width,game.renderer.height));
            if let Some(p)=&game.renderer.presentation {ui.text(format!("Swapchain: {:?}",p.mode));}
            ui.text(format!("XYZ: {:.2}, {:.2}, {:.2}",game.player.feet.x,game.player.feet.y,game.player.feet.z));
            ui.text(format!("Chunk: {}, {} | seed {}",game.center.0,game.center.1,game.world.seed));
            ui.text(format!("Fish {} | plants {} | objects {} | triangles {}",game.renderer.scene.fish_count(),game.renderer.scene.plants,game.renderer.scene.objects.len(),game.renderer.scene.triangles.len()));
            ui.text(if game.renderer.ray_query_supported() && game.renderer.settings.hardware {"Mesh traversal: Vulkan ray query"}else{"Mesh traversal: software triangle tests"});
            let mt=game.renderer.mesh_timings;
            ui.text(format!("CPU skin {:.3} | upload {:.3} ms",mt.animation_ms,mt.upload_ms));
            ui.text(format!("GPU BLAS {:.3} | TLAS {:.3} ms",mt.gpu[0],mt.gpu[1]));
            ui.text(format!("GPU mesh rays {:.3} | shadows {:.3} ms",mt.gpu[2],mt.gpu[3]));
            ui.text(format!("AS active slot (3-slot ring) {:.1} KiB",game.renderer.as_memory_bytes() as f32/1024.0));
            let chunks=game.renderer.chunk_counts;
            ui.text(format!("Primary frustum: {} / {} chunks",chunks[1],chunks[0]));
            ui.text(format!("Entity activation: {} player(s), 64m box per player",game.renderer.active_players.len()));
            ui.text(if game.renderer.sky_only {"Empty frame: sky-only fast path"} else {"Scene passes active"});
            let stats=game.renderer.section_counts;
            ui.text(format!("Sections: {} / {} visible ({} culled)",stats[1],stats[0],stats[0]-stats[1]));
            ui.text(format!("Caustic map {:.3} ms",game.renderer.caustic_ms));
            ui.text(format!("Water pass {:.3} ms | wave filter {:.0}x",game.renderer.water_ms,game.renderer.wave_anisotropy()));
            let hardware_supported=game.renderer.ray_query_supported();
            let max_view=game.renderer.max_view_chunks();
            ui.text(format!("Chunk cache: {} chunks | {:.1} / {} MiB (pinned {:.1}, evicted {})",game.cache.chunks,game.cache.bytes as f32/1048576.0,game.cache.budget/1048576,game.cache.pinned as f32/1048576.0,game.cache.evicted));
            ui.text(format!("Streaming: {} | AS snapshot age {:.1} ms | worker {:.1} ms",if game.world_pending {"loading"}else{"ready"},(game.renderer.animation_clock()-game.renderer.snapshot_time).max(0.0)*1000.0,game.renderer.background_ms));
            ui.slider("CPU chunk cache (MiB)",8,1024,&mut game.cache_mb);
            ui.checkbox("Auto-jump",&mut game.player.auto_jump);
            let mut flying=game.player.flying;
            if ui.checkbox("Fly camera (F2)",&mut flying) {game.player.toggle_fly();}
            if let Some(audio)=&mut self.audio {
                ui.checkbox("BGM",&mut audio.playing);ui.slider("Music volume",0.0,1.0,&mut audio.volume);
                let (played,gap,late,errors)=audio.stats();
                ui.text(format!("Audio {:.1}/{:.1}s | max gap {:.1}ms | late {} | errors {}",played%audio.duration,audio.duration,gap,late,errors));
                audio.update();
            } else if let Some(error)=&self.audio_error {ui.text_wrapped(format!("Audio: {error}"));}
            ui.text(format!("Sky 1024x512 / importance 256x128 | builds {} | rows {}/512",game.renderer.sky_cache.rebuilds,game.renderer.sky_cache.progress()));
            ui.text(format!("Sky {} | build {:.3}ms | denoise {:.3}ms",if game.renderer.settings.day_cycle {format!("target {:.2}s",game.renderer.sky_cache.interval)}else{"on change".into()},game.renderer.sky_ms,game.renderer.denoise_ms));
            let resolutions=[128u32,256,512,1024,2048];
            let mut selected=resolutions.iter().position(|r| *r==game.renderer.settings.csm_resolution).unwrap_or(2);
            if ui.combo_simple_string("CSM resolution",&mut selected,&["128","256","512","1024","2048"]) {game.renderer.settings.csm_resolution=resolutions[selected];}
            if let Some(pack)=&game.renderer.resource_pack {
                ui.text(format!("Pack: {} | {}x{} | {} PNGs / {} animations / {} skipped",pack.stats.name,pack.width,pack.height,pack.stats.imported,pack.stats.animated,pack.stats.skipped));
                ui.text(if game.renderer.bindless_textures(){"Textures: bindless atlas table"}else{"Textures: single-atlas compatibility path"});
            }
            if let Some(lod)=&game.renderer.lod {ui.text(format!("LOD pages {:?} | {:.1} / {} MiB | uploaded {}",lod.levels,lod.bytes as f32/1048576.0,lod::POOL_BYTES/1048576,lod.uploaded));ui.text(format!("LOD residency target {:.1}px (actual VRAM budget)",lod.effective_pixels));}
            let climate=terrain::sample(game.player.feet.x.floor() as i32,game.player.feet.z.floor() as i32,game.world.seed);
            ui.text(format!("Biome: {} | temperature {:.2} | humidity {:.2}",game.world.biome_at(game.player.feet.x.floor() as i32,game.player.feet.z.floor() as i32).name(),climate.temperature,climate.humidity));
            ui.text(format!("Terrain: erosion {:.2}, roughness {:.2}",climate.erosion,climate.roughness));
            ui.text(format!("Voxel GI pass {:.3} ms",game.renderer.gi_ms));
            let settings=&mut game.renderer.settings;
            ui.slider("View distance (chunks)",1,max_view,&mut settings.view_chunks);
            ui.slider("LOD voxel target (pixels)",1.0,64.0,&mut settings.lod_pixels);
            ui.checkbox("Cross-chunk uniform merging", &mut settings.lod_merging);
            ui.checkbox("Empty-space hierarchy",&mut settings.hierarchy);
            ui.checkbox("Frustum culling (camera only)",&mut settings.frustum);
            ui.checkbox("AO depth pyramid",&mut settings.depth_pyramid);
            ui.checkbox("Linear minification (nearest magnification)",&mut settings.texture_linear);
            if ui.combo_simple_string("Shadow type",&mut settings.shadow_type,&["Ray Traced","Hybrid","CSM"]) {settings.csm=true;settings.far_shadows=true;}
            ui.checkbox("Soft disk shadows",&mut settings.soft_shadows);
            ui.slider("Disk angular radius (deg)",0.05,2.0,&mut settings.disk_radius);
            if settings.shadow_type != 2 { ui.slider("Ray samples in penumbra",2,4,&mut settings.penumbra_samples); }
            ui.checkbox("Experimental voxel GI (replaces ambient + AO)",&mut settings.vxgi);
            if settings.vxgi {
                ui.slider("GI distance (m)",8.0,256.0,&mut settings.gi_range);
                ui.slider("GI cone angle (deg)",5.0,60.0,&mut settings.gi_angle);
                ui.checkbox("GI temporal reprojection",&mut settings.gi_temporal);
            }
            ui.checkbox("Ground bounce",&mut settings.ground_bounce);
            ui.checkbox("Atmospheric sky",&mut settings.atmosphere);
            ui.checkbox("Day / night animation",&mut settings.day_cycle);
            ui.slider("Day duration (seconds)",60.0,3600.0,&mut settings.day_seconds);
            ui.slider("Exact shadow rays (m)",0.0,192.0,&mut settings.shadow_ray_distance);
            ui.checkbox("SSAO",&mut settings.ssao);ui.same_line();ui.checkbox("Sun shadows",&mut settings.shadows);
            ui.checkbox("Transparent water",&mut settings.water);
            ui.checkbox("Water optical rays",&mut settings.ssr);ui.same_line();ui.checkbox("Wave bump",&mut settings.waves);
            ui.checkbox("Polygon meshes",&mut settings.meshes);ui.same_line();if hardware_supported {ui.checkbox("Hardware ray queries",&mut settings.hardware);}else{ui.text("Ray queries unavailable");}
            ui.checkbox("Fish",&mut settings.fish);ui.same_line();ui.checkbox("Foliage",&mut settings.foliage);
            ui.checkbox("Alpha cutouts",&mut settings.alpha_test);ui.same_line();ui.checkbox("Animate fish",&mut settings.animate);
            ui.checkbox("Rebuild fish BLAS (vs refit)",&mut settings.rebuild_blas);
            if ui.collapsing_header("Lighting controls",imgui::TreeNodeFlags::empty()) {
            ui.slider("AO radius (m)",0.25,3.0,&mut settings.ao_radius);
            ui.slider("AO strength",0.0,8.0,&mut settings.ao_strength);
            ui.checkbox("Hable Uncharted 2 tone mapping",&mut settings.tone_mapping);
            ui.slider("Exposure (EV)",-8.0,8.0,&mut settings.exposure_ev);
            ui.slider("Saturation",0.0,2.0,&mut settings.saturation);
            if ui.button("Reset exposure") {settings.exposure_ev=0.0;}
            ui.text("+1 EV doubles intensity; AO affects sky and ground bounce");
            ui.slider("Water density",0.1,2.0,&mut settings.water_density);
            ui.slider("Underwater fog",0.2,5.0,&mut settings.underwater_density);
            ui.slider("Water IOR",1.0,1.6,&mut settings.water_ior);
            ui.checkbox("Underwater caustics (optical rays)",&mut settings.caustics);
            if settings.caustics {
                ui.checkbox("Fine near caustic map",&mut settings.caustic_near);
                ui.checkbox("Adaptive photon footprints",&mut settings.caustic_adaptive);
                ui.checkbox("Shallow caustic depth slices",&mut settings.caustic_shallow);
                ui.slider("Caustic strength",0.0,2.0,&mut settings.caustic_strength);
                ui.slider("Caustic fade distance (m)",8.0,96.0,&mut settings.caustic_distance);
            }
            ui.checkbox("Six wave octaves",&mut settings.multiscale_waves);
            if settings.multiscale_waves {
                ui.slider("Wave domain warp (m)",0.0,4.0,&mut settings.wave_warp);
                ui.slider("Detail fade distance (m)",16.0,512.0,&mut settings.wave_distance);
            }
            ui.checkbox("Stochastic wave tiling",&mut settings.stochastic_waves);
            ui.slider("Wave strength",0.0,0.7,&mut settings.wave_strength);
            ui.slider("Sun azimuth",-180.0,180.0,&mut settings.sun_azimuth);
            ui.slider("Sun elevation",-90.0,90.0,&mut settings.sun_elevation);
            ui.slider("Sun intensity",0.0,2.0,&mut settings.sun_intensity);
            }
            let labels=["Stone","Grass","Dirt","Sand","Water","Wood","Snow","Oak leaves","Pine needles","Gravel","Sandstone","Granite","Meadow grass","Podzol"];
            let materials=[Block::Stone,Block::Grass,Block::Dirt,Block::Sand,Block::Water,Block::Wood,Block::Snow,Block::OakLeaves,Block::PineLeaves,Block::Gravel,Block::Sandstone,Block::Granite,Block::MeadowGrass,Block::Podzol];
            let mut material=materials.iter().position(|&b| b==game.selected).unwrap_or(0);
            if ui.combo_simple_string("Material",&mut material,&labels) {game.selected=materials[material];}
            let mut view=game.debug as usize;
            if ui.combo_simple_string("View",&mut view,&["Shaded","Opaque normals","Depth / view distance","Opaque material","Opaque UV","Ambient occlusion","Water thickness","Sun visibility","Traversal work","Mesh / alpha flags","Water reflection coverage","Water bump normals","Water Fresnel / TIR","Water path","Reflected voxel material"]) {game.debug=view as u32;}
            ui.separator();ui.text_wrapped("Click world to capture; Esc releases. WASD / mouse, Space jump, Ctrl / Shift run. LMB delete, RMB place. 1-7 material, F1 view, F11 HUD, F2 fly (Space/C rise/fall).");
        });
        }
        gui.platform.prepare_render(ui, w);
        if self.captured {
            w.set_cursor_visible(false);
        }
        let draw = gui.context.render();
        game.sync_world()?;
        game.renderer
            .present(&game.player, game.origin, game.debug, draw)?;
        if self.frames >= 60 {
            self.frame_times.push(dt * 1000.0);
            if let Some(gpu) = game.renderer.gpu_ms {
                self.measured_frames += 1;
                self.water_sum += game.renderer.water_ms as f64;
                self.caustic_sum += game.renderer.caustic_ms as f64;
                self.gi_sum += game.renderer.gi_ms as f64;
                for (sum, value) in self.gpu_sums.iter_mut().zip(gpu) {
                    *sum += value as f64;
                }
                for (sum, value) in self
                    .mesh_sums
                    .iter_mut()
                    .zip(game.renderer.mesh_timings.gpu)
                {
                    *sum += value as f64;
                }
                self.cpu_sums[0] += game.renderer.mesh_timings.animation_ms as f64;
                self.cpu_sums[1] += game.renderer.mesh_timings.upload_ms as f64;
            }
        }
        self.frames += 1;
        if self.resize_test {
            let next = match self.frames {
                60 => Some((800, 600)),
                120 => Some((1440, 810)),
                180 => Some((480, 640)),
                240 => Some((1280, 720)),
                _ => None,
            };
            if let Some((width, height)) = next {
                let _ = w.request_inner_size(winit::dpi::PhysicalSize::new(width, height));
            }
        }
        Ok(())
    }
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let setup = (|| -> Result<()> {
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title("BlockWorld · Rust / Vulkan")
                        .with_visible(false)
                        .with_inner_size(winit::dpi::LogicalSize::new(1280, 720)),
                )?,
            );
            if std::env::args().any(|a| a == "--fullscreen") {
                window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
            }
            self.loading=Some(loading::LoadingScreen::new(window.clone())?);
            self.window=Some(window);
            self.last = Instant::now();
            Ok(())
        })();
        if let Err(e) = setup {
            self.error = Some(e);
            event_loop.exit();
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let (Some(gui), Some(w)) = (&mut self.gui, &self.window) {
            gui.platform.handle_event::<()>(
                gui.context.io_mut(),
                w,
                &winit::event::Event::WindowEvent {
                    window_id: id,
                    event: event.clone(),
                },
            );
        }
        if self.game.is_none() {
            match event {
                WindowEvent::CloseRequested => {
                    self.startup_cancel.store(true,std::sync::atomic::Ordering::Relaxed);
                    if let Some(w)=&self.window {w.set_visible(false);}
                    event_loop.exit();
                }
                WindowEvent::RedrawRequested => {
                    if let Err(e)=self.draw_loading() {self.error=Some(e);event_loop.exit();}
                }
                _=>{}
            }
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(false) => {
                if let Some(g) = &mut self.game {
                    g.keys.clear();
                }
                self.capture(false);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if event.state == ElementState::Pressed {
                        let g = self.game.as_mut().unwrap();
                        g.keys.insert(code);
                        if !event.repeat {
                            match code {
                                KeyCode::Escape => self.capture(false),
                                KeyCode::F1 => g.debug = (g.debug + 1) % 15,
                                KeyCode::F2 => g.player.toggle_fly(),
                                KeyCode::F11 => self.hud_visible = !self.hud_visible,
                                KeyCode::Digit7 => g.selected = Block::Snow,
                                KeyCode::Digit1 => g.selected = Block::Stone,
                                KeyCode::Digit2 => g.selected = Block::Grass,
                                KeyCode::Digit3 => g.selected = Block::Dirt,
                                KeyCode::Digit4 => g.selected = Block::Sand,
                                KeyCode::Digit5 => g.selected = Block::Water,
                                KeyCode::Digit6 => g.selected = Block::Wood,
                                _ => {}
                            }
                        }
                    } else {
                        self.game.as_mut().unwrap().keys.remove(&code);
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                let ui_capture = self
                    .gui
                    .as_ref()
                    .is_some_and(|g| g.context.io().want_capture_mouse);
                if !self.captured && !ui_capture {
                    self.capture(true);
                } else if self.captured {
                    let g = self.game.as_mut().unwrap();
                    if button == MouseButton::Left {
                        g.edit(false);
                    } else if button == MouseButton::Right {
                        g.edit(true);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.draw() {
                    self.error = Some(e);
                    event_loop.exit();
                }
                if self.frame_limit.is_some_and(|n| self.frames >= n) {
                    if let Some(audio) = &self.audio {
                        audio.report();
                    }
                    if let Some(g) = &self.game {
                        println!("Render extent: {}x{}", g.renderer.width, g.renderer.height);
                    }
                    println!("Window benchmark: {} frames in {:.3}s, {:.1} FPS average (includes presentation; startup excluded)",self.frames,self.total_seconds,self.frames as f64/self.total_seconds.max(0.001));
                    if let Some(g) = &self.game {
                        println!("Streaming: requests {}, adopted {}, stale {}, AS swaps {}; max main-thread sync {:.3} ms; cache {} chunks {:.2} MiB (pinned {:.2}, budget {} MiB)",g.generation,g.stream_counts[0],g.stream_counts[1],g.stream_counts[2],g.max_sync_ms,g.cache.chunks,g.cache.bytes as f32/1048576.0,g.cache.pinned as f32/1048576.0,g.cache_mb);
                        println!("Max CPU renderer stages: acquire {:.3}, prepare {:.3}, submit/present {:.3}, fence wait {:.3} ms",g.renderer.cpu_stage_max[0],g.renderer.cpu_stage_max[1],g.renderer.cpu_stage_max[2],g.renderer.cpu_stage_max[3]);
                        if let Some(lod)=&g.renderer.lod {println!("LOD pages {:?}, uploaded {}, {:.2}/{} MiB, effective target {:.1}px",lod.levels,lod.uploaded,lod.bytes as f32/1048576.0,lod::POOL_BYTES/1048576,lod.effective_pixels);}
                        let mt = g.renderer.mesh_timings;
                        println!("Mesh stats: fish {}, foliage {}, triangles {}, objects {}, backend {}, CPU skin {:.3} ms, upload {:.3} ms, setup {:.3} ms, GPU BLAS {:.3} ms, TLAS {:.3} ms, mesh primary {:.3} ms, mesh shadows {:.3} ms, initial AS build {:.3} ms, AS buffer bytes {}",g.renderer.scene.fish_count(),g.renderer.scene.plants,g.renderer.scene.triangles.len(),g.renderer.scene.objects.len(),if g.renderer.ray_query_supported() && g.renderer.settings.hardware {"ray-query"}else{"software"},mt.animation_ms,mt.upload_ms,mt.setup_ms,mt.gpu[0],mt.gpu[1],mt.gpu[2],mt.gpu[3],mt.initial_build_ms,g.renderer.as_memory_bytes());
                    }
                    if !self.frame_times.is_empty() {
                        self.frame_times.sort_by(f32::total_cmp);
                        let n = self.frame_times.len();
                        println!("Frame latency after warmup: p50 {:.3}, p95 {:.3}, p99 {:.3}, max {:.3} ms",self.frame_times[n/2],self.frame_times[(n*95/100).min(n-1)],self.frame_times[(n*99/100).min(n-1)],self.frame_times[n-1]);
                    }
                    if self.measured_frames > 0 {
                        let n = self.measured_frames as f64;
                        let m = self.mesh_sums.map(|v| v / n);
                        let c = self.cpu_sums.map(|v| v / n);
                        let g = self.gpu_sums.map(|v| v / n);
                        println!("Mean voxel GI: {:.3} ms", self.gi_sum / n);
                        println!("Mean caustic generation: {:.3} ms", self.caustic_sum / n);
                        println!(
                            "Mean water composition (optical rays + bump): {:.3} ms",
                            self.water_sum / n
                        );
                        println!("Mean over {} frames after 60-frame warmup: CPU skin {:.3} ms, upload {:.3} ms; GPU BLAS {:.3} ms, TLAS {:.3} ms, mesh primary {:.3} ms, mesh shadow {:.3} ms; voxel {:.3} ms, SSAO {:.3} ms, shade {:.3} ms, total {:.3} ms",self.measured_frames,c[0],c[1],m[0],m[1],m[2],m[3],g[0],g[1],g[2],g[3]);
                    }
                    if let Some(game) = self.game.as_ref() {
                        println!("Last acceleration: depth mips {:.3} ms, cascade updates {:.3} ms; voxel/mesh builds {:?}",game.renderer.acceleration_ms[0],game.renderer.acceleration_ms[1],game.renderer.cascade_updates);
                        println!(
                            "Sky builds {}, last {:.3} ms; denoise {:.3} ms",
                            game.renderer.sky_cache.rebuilds,
                            game.renderer.sky_ms,
                            game.renderer.denoise_ms
                        );
                        println!(
                            "Far depth voxel/mesh builds {:?}",
                            game.renderer.far_depth_updates
                        );
                    }
                    if let Some(ms) = self.game.as_ref().and_then(|g| g.renderer.gpu_ms) {
                        println!(
                            "Last GPU frame: visibility {:.3} ms, SSAO {:.3} ms, shade {:.3} ms, total {:.3} ms",
                            ms[0], ms[1], ms[2],ms[3]
                        );
                    }
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }
    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        if self.captured {
            if let DeviceEvent::MouseMotion { delta } = event {
                self.game.as_mut().unwrap().player.look(delta.0, delta.1);
            }
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.game.is_none() {
            let now=Instant::now();
            if now>=self.next_loading_redraw {
                if let Some(w)=&self.window {w.request_redraw();}
                self.next_loading_redraw=now+std::time::Duration::from_millis(16);
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_loading_redraw));
        } else {
            if let Some(w)=&self.window {w.request_redraw();}
            event_loop.set_control_flow(ControlFlow::Poll);
        }
    }
}
fn ppm(path: &str, pixels: &[u32]) -> Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(f, "P6\n{} {}\n255\n", RENDER_WIDTH, RENDER_HEIGHT)?;
    for &v in pixels {
        f.write_all(&[(v >> 16) as u8, (v >> 8) as u8, v as u8])?;
    }
    Ok(())
}
fn headless(mut game: Game, smoke: bool, path: &str) -> Result<()> {
    let pixels = game.renderer.render(&game.player, game.origin, 0)?;
    ppm(path, pixels)?;
    let hits = game.renderer.visibility();
    let count = hits.iter().filter(|h| h.valid != 0).count();
    anyhow::ensure!(count > 100, "Terrain frame contains too few visible blocks");
    for h in hits.iter().filter(|h| h.valid != 0) {
        anyhow::ensure!(
            (1..=meshes::MATERIALS as u32).contains(&h.material)
                && h.normal_depth[3].is_finite()
                && h.normal_depth[3] >= 0.0,
            "Invalid visibility record"
        );
        anyhow::ensure!(h.uv.iter().all(|v| *v >= 0.0 && *v <= 1.0), "Invalid UV");
    }
    println!("Rendered {count} visible pixels to {path}");
    if smoke {
        game.renderer.reference_validation = true;
        smoke::traversal(&mut game.renderer, &game.player, game.origin)?;
        smoke::acceleration(&mut game.renderer, &game.player, game.origin)?;
        game.renderer.settings.meshes = false;
        game.player = Player::new(100.0);
        game.player.yaw = 0.0;
        game.player.pitch = 0.0;
        let front = IVec3::new(0, 101, -3);
        let back = IVec3::new(0, 101, -5);
        game.world.set(front, Block::Wood);
        game.world.set(back, Block::Stone);
        game.dirty = true;
        game.sync_world()?;
        game.renderer.render(&game.player, game.origin, 0)?;
        let center = (RENDER_HEIGHT / 2 * RENDER_WIDTH + RENDER_WIDTH / 2) as usize;
        anyhow::ensure!(
            game.renderer.visibility()[center].material == Block::Wood as u32,
            "Expected wood in GPU visibility buffer"
        );
        game.edit(false);
        game.sync_world()?;
        game.renderer.render(&game.player, game.origin, 0)?;
        anyhow::ensure!(game.world.get(front) == Block::Air, "Delete failed");
        anyhow::ensure!(
            game.renderer.visibility()[center].material == Block::Stone as u32,
            "Delete did not reveal stone on GPU"
        );
        game.selected = Block::Sand;
        game.edit(true);
        game.sync_world()?;
        game.renderer.render(&game.player, game.origin, 0)?;
        anyhow::ensure!(
            game.renderer.visibility()[center].material == Block::Sand as u32,
            "Placement did not reach GPU"
        );
        for mode in 1..15 {
            game.renderer.render(&game.player, game.origin, mode)?;
        }
        smoke::block_materials(&mut game.renderer)?;
        smoke::lighting(&mut game.renderer)?;
        smoke::ao_planar(&mut game.renderer)?;
        smoke::voxel_edges(&mut game.renderer)?;
        smoke::far_shadows(&mut game.renderer)?;
        smoke::resizing(&mut game.renderer)?;
        smoke::polygons(&mut game.renderer)?;
        smoke::view_distance(&mut game.renderer)?;
        smoke::lod(&mut game.renderer)?;
        smoke::svo(&mut game.renderer)?;
        smoke::lod_edits(&mut game.renderer)?;
        smoke::close_up(&mut game.renderer)?;
        smoke::empty_frame(&mut game.renderer)?;
        smoke::sparse_mesh(&mut game.renderer)?;
        smoke::water_effects(&mut game.renderer)?;
        smoke::caustics(&mut game.renderer)?;
        smoke::streaming(&mut game.renderer)?;
        smoke::soft_sky(&mut game.renderer)?;
        smoke::shadow_quality(&mut game.renderer)?;
        smoke::resource_pack(&mut game.renderer)?;
        smoke::texture_filtering(&mut game.renderer)?;
        smoke::tone_mapping(&mut game.renderer)?;
        smoke::ground_bounce(&mut game.renderer)?;
        smoke::entity_visibility(&mut game.renderer)?;
        println!("Vulkan smoke test passed: visibility, deferred shading, deletion, placement, all debug views.");
    }
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a|a=="--vxgi-test") {return smoke::vxgi(&mut Game::new(None)?.renderer);}
    if args.iter().any(|a|a=="--svo-test") {return smoke::svo(&mut Game::new(None)?.renderer);}
    if args.iter().any(|a|a=="--entity-test") {return smoke::entity_visibility(&mut Game::new(None)?.renderer);}
    if args.iter().any(|a|a=="--shadow-quality-test") {return smoke::shadow_quality(&mut Game::new(None)?.renderer);}
    if args.iter().any(|a|a=="--texture-filter-test") {return smoke::texture_filtering(&mut Game::new(None)?.renderer);}
    if args.iter().any(|a|a=="--voxel-edge-test") {return smoke::voxel_edges(&mut Game::new(None)?.renderer);}
    if args.iter().any(|a|a=="--lod-edit-test") {return smoke::lod_edits(&mut Game::new(None)?.renderer);}
    if args.iter().any(|a|a=="--lod-stream-test") {return smoke::lod_streaming(&mut Game::new(None)?.renderer);}
    if args.iter().any(|a|a=="--lod-test") {return smoke::lod(&mut Game::new(None)?.renderer);}
    if args.iter().any(|s| s == "--help" || s == "-h") {
        println!("BlockWorld\n  cargo run --release\n  --headless [output.ppm]  Render one frame without a window\n  --smoke-test [output.ppm]  Exercise Vulkan and block editing\n  --stream-test  Automated turning, movement, range and cache-budget stress test\n  --cache-mb N  CPU chunk cache budget (active chunks stay pinned)\n  --view-chunks N  Set view distance 1..128 chunks (also in ImGui)\n  --noisy-world --fly  Deterministic incompressible terrain stress fixture\n  --vxgi --no-gi-temporal  Experimental voxel GI; optional temporal bypass\n  --vxgi-test  Validate experimental GI on Vulkan\n  --lod-pixels N  Projected voxel target, 1..64 pixels (default 1; budget adaptive)\n  --frames N  Exit window after N frames\n  --resize-test  Exercise window/buffer resizing (use --frames 360)\n  --fullscreen  Use the monitor native extent\n  --sky-view --ground-sky-view  Empty-sky profiling cameras\n  --block-view --ground-view  Single-block / natural-ground close-up cameras\n  --no-hierarchy --no-culling --no-ssao --no-shadows --opaque-water\n  --no-depth-pyramid --no-csm  Reference lighting paths\n  --fixed-time SECONDS  Deterministic fish and wave pose for comparisons\n  --shadow-ray-distance METRES  Exact contact-shadow radius (default 48)\n  --no-far-shadows  Exact rays at all distances\n  --shadow-type ray|hybrid|csm --hard-shadows\n  --csm-resolution 128|256|512|1024|2048\n  --resource-pack PATH.zip --texture-filter nearest|linear\n  --resource-pack-test  Verify ZIP/PNG animation, padding and GPU sampling\n  --no-atmosphere --day-cycle --sun-elevation DEGREES\n  --audio-test  Verify BGM playback\n  --no-audio  Disable BGM\n  --fly  Start in fly-camera mode (F2 toggles; Space/C rise/fall)\n  --water-test  Verify water optics and bump on Vulkan\n  --biome NAME  Start at a biome (e.g. desert, deep-ocean, jagged-peaks)\n  --mountain-view  Start overlooking a snowy mountain\n  --underwater-view  Start submerged, looking at the water surface\n  --water-view  Start above a lake for water profiling\n  --no-ssr --no-waves --no-stochastic-waves\n  --no-caustics  Disable underwater direct-light focusing\n  --no-caustic-near --no-caustic-adaptive --no-caustic-shallow\n  --caustics-test  Validate photon maps and lighting gates\n  --no-multiscale-waves --no-wave-warp  Wave comparisons\n  --no-meshes --no-rt --freeze-animation --rebuild-blas\n    Disable effects individually for comparisons\n\nWASD walk, mouse look, Space jump/swim up, Ctrl / Shift run; ImGui auto-jump\nLeft click delete, right click place; 1–7 material; F1 debug views; F11 HUD; F2 fly (Space/C rise/fall)\nClick captures mouse, Esc releases. Edits are kept in memory only.");
        return Ok(());
    }
    if args.iter().any(|a| a == "--audio-test") {
        let audio = audio::Audio::start()?;
        std::thread::sleep(std::time::Duration::from_secs(4));
        audio.report();
        anyhow::ensure!(
            audio.stats().0 > 2.0 && audio.stats().3 == 0,
            "Audio output failed"
        );
        return Ok(());
    }
    if args.iter().any(|a| a=="--resource-pack-test") {
        return smoke::resource_pack(&mut Game::new(None)?.renderer);
    }
    if args.iter().any(|a| a == "--caustics-test") {
        return smoke::caustics(&mut Game::new(None)?.renderer);
    }
    if args.iter().any(|a| a == "--water-test") {
        return smoke::water_effects(&mut Game::new(None)?.renderer);
    }
    if args
        .iter()
        .any(|s| s == "--headless" || s == "--smoke-test")
    {
        let path = args
            .iter()
            .find(|s| s.ends_with(".ppm"))
            .map_or("blockworld.ppm", String::as_str);
        return headless(
            Game::new(None).context("Initialize world and Vulkan")?,
            args.iter().any(|s| s == "--smoke-test"),
            path,
        );
    }
    let frame_limit = args
        .iter()
        .position(|s| s == "--frames")
        .map(|i| {
            args.get(i + 1)
                .context("--frames needs a number")?
                .parse::<u32>()
                .context("Invalid frame count")
        })
        .transpose()?;
    let event_loop = EventLoop::new()?;
    let mut app = App {
        audio: None,
        audio_error: None,
        game: None,
        loading: None,
        startup: None,
        startup_worker: None,
        next_loading_redraw: Instant::now(),
        startup_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        window: None,
        gui: None,
        captured: false,
        hud_visible: true,
        last: Instant::now(),
        frames: 0,
        frame_limit,
        resize_test: args.iter().any(|arg| arg == "--resize-test"),
        stream_test: args.iter().any(|arg| arg == "--stream-test"),
        frame_times: Vec::new(),
        error: None,
        average_ms: 0.0,
        total_seconds: 0.0,
        measured_frames: 0,
        gpu_sums: [0.0; 4],
        mesh_sums: [0.0; 4],
        cpu_sums: [0.0; 2],
        water_sum: 0.0,
        caustic_sum: 0.0,
        gi_sum: 0.0,
    };
    event_loop.run_app(&mut app)?;
    if let Some(e) = app.error.take() {
        return Err(e);
    }
    Ok(())
}

impl Drop for App {
    fn drop(&mut self) {
        self.startup_cancel.store(true,std::sync::atomic::Ordering::Relaxed);
        self.startup.take();
        if let Some(worker)=self.startup_worker.take() {let _=worker.join();}
    }
}
