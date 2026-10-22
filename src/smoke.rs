//! Headless integration checks exercise the real Vulkan buffers and compute passes.
use crate::{
    config::*,
    player::Player,
    renderer::{RenderSettings, Renderer, Visibility},
};
use anyhow::{ensure, Result};
use glam::{IVec3, Vec3};

pub fn traversal(r: &mut Renderer, player: &Player, origin: IVec3) -> Result<()> {
    traversal_parity(r, player, origin, true)
}
fn traversal_parity(
    r: &mut Renderer,
    player: &Player,
    origin: IVec3,
    require_gain: bool,
) -> Result<()> {
    let saved = r.settings;
    r.settings.ssao = false;
    r.settings.meshes = false;
    r.settings.shadows = false;
    r.render(player, origin, 0)?;
    let fast = r.visibility().to_vec();
    let counts = r.section_counts;
    r.settings.hierarchy = false;
    r.settings.frustum = false;
    r.render(player, origin, 0)?;
    let mut fast_work = 0.0;
    let mut slow_work = 0.0;
    for (a, b) in fast.iter().zip(r.visibility()) {
        ensure!(
            a.material == b.material && a.valid == b.valid,
            "Hierarchy/frustum changed material visibility"
        );
        ensure!(
            (a.normal_depth[3] - b.normal_depth[3]).abs() < 0.003,
            "Hierarchy changed hit depth"
        );
        ensure!(
            (a.water[0] - b.water[0]).abs() < 0.003 && a.water[1] == b.water[1],
            "Hierarchy changed water path"
        );
        fast_work += a.water[3] as f64;
        slow_work += b.water[3] as f64;
    }
    ensure!(fast_work <= slow_work, "Hierarchy increased traversal work");
    if require_gain {
        ensure!(fast_work < slow_work, "Hierarchy did not skip empty space");
    }
    println!(
        "Traversal parity passed: {:.1}% fewer primary steps; {}/{} occupied sections in frustum",
        100.0 * (slow_work - fast_work) / slow_work.max(1.0),
        counts[1],
        counts[0]
    );
    r.settings = saved;
    Ok(())
}
fn put(blocks: &mut [u8], origin: IVec3, p: IVec3, material: u8) {
    let q = p - origin;
    assert!(q.x >= 0 && q.y >= 0 && q.z >= 0);
    blocks[q.x as usize + GPU_EDGE * (q.z as usize + GPU_EDGE * q.y as usize)] = material;
}
fn center(r: &Renderer) -> usize {
    (r.height / 2 * r.width + r.width / 2) as usize
}
fn shot(r: &mut Renderer, p: &Player, o: IVec3, mode: u32) -> Result<(u32, Visibility)> {
    let i = center(r);
    let pixel = r.render(p, o, mode)?[i];
    Ok((pixel, r.visibility()[i]))
}
pub fn lighting(r: &mut Renderer) -> Result<()> {
    let saved = r.settings;
    r.settings = RenderSettings::default();
    r.settings.ssao = false;
    let origin = IVec3::new(-(GPU_EDGE as i32) / 2, MIN_Y, -(GPU_EDGE as i32) / 2);
    let mut blocks = vec![0; GPU_VOXELS];
    let mut p = Player::new(100.0);
    p.yaw = 0.0;
    p.pitch = 0.0;
    put(&mut blocks, origin, IVec3::new(0, 101, -3), 5);
    put(&mut blocks, origin, IVec3::new(0, 101, -5), 1);
    r.upload(&blocks)?;
    let (transparent, h) = shot(r, &p, origin, 0)?;
    ensure!(
        h.material == 1 && h.water[1] == 1.0,
        "Water failed to preserve opaque background"
    );
    ensure!(
        (h.water[0] - 1.0).abs() < 0.01,
        "Wrong one-metre water thickness"
    );
    ensure!(
        (h.water_normal_depth[3] - 2.5).abs() < 0.01,
        "Wrong water surface depth"
    );
    r.settings.water = false;
    let (opaque, h) = shot(r, &p, origin, 0)?;
    ensure!(
        h.material == 5 && opaque != transparent,
        "Water toggle did not change composition"
    );
    r.settings.water = true;
    put(&mut blocks, origin, IVec3::new(0, 101, 0), 5);
    r.upload(&blocks)?;
    let (_, h) = shot(r, &p, origin, 0)?;
    ensure!(
        h.water[2] == 1.0 && (h.water_normal_depth[3] - 0.5).abs() < 0.01,
        "Underwater interface missing"
    );
    ensure!(
        (h.water[0] - 1.5).abs() < 0.01,
        "Underwater path length is wrong"
    );
    println!("Water passed: opaque layer, surface depth, thickness, underwater exit, transparency toggle");

    // A receiver faces the camera; the blocker is behind the camera in culled sections.
    blocks.fill(0);
    put(&mut blocks, origin, IVec3::new(0, 101, -5), 1);
    r.settings.sun_azimuth = 60.0;
    r.settings.sun_elevation = 20.0;
    r.upload(&blocks)?;
    let (lit, _) = shot(r, &p, origin, 7)?;
    ensure!(lit == 0xffffff, "Unobstructed sun is blocked");
    for z in 14..17 {
        for y in 114..117 {
            for x in 32..35 {
                put(&mut blocks, origin, IVec3::new(x, y, z), 1);
            }
        }
    }
    r.upload(&blocks)?;
    let (shadow, _) = shot(r, &p, origin, 7)?;
    ensure!(
        shadow == 0,
        "Off-screen solid blocker did not shadow the receiver"
    );
    r.settings.csm = false;
    let (reference, _) = shot(r, &p, origin, 7)?;
    ensure!(
        reference == shadow,
        "Conservative cascades lost off-screen caster"
    );
    r.settings.csm = true;
    r.settings.frustum = false;
    let (unculled, _) = shot(r, &p, origin, 7)?;
    ensure!(
        unculled == shadow,
        "Frustum mask incorrectly affects shadow rays"
    );
    r.settings.frustum = true;
    r.settings.shadows = false;
    let (disabled, _) = shot(r, &p, origin, 7)?;
    ensure!(disabled == lit, "Shadow toggle did not restore sunlight");
    r.settings.shadows = true;
    for z in 14..17 {
        for y in 114..117 {
            for x in 32..35 {
                put(&mut blocks, origin, IVec3::new(x, y, z), 5);
            }
        }
    }
    r.upload(&blocks)?;
    let (water_sun, _) = shot(r, &p, origin, 7)?;
    ensure!(
        water_sun == lit,
        "Water-only occupancy incorrectly blocks sunlight"
    );
    println!("Sunlight passed: off-screen shadow caster, culling independence, transparent water, shadow toggle");

    blocks.fill(0);
    for x in -6..=6 {
        for z in -10..=4 {
            put(&mut blocks, origin, IVec3::new(x, 99, z), 1);
        }
        for y in 100..=104 {
            put(&mut blocks, origin, IVec3::new(x, y, -4), 1);
        }
    }
    p.pitch = -0.45;
    r.settings.ssao = true;
    r.settings.shadows = false;
    r.settings.ao_strength = 2.0;
    r.upload(&blocks)?;
    r.render(&p, origin, 5)?;
    let ao = r.ambient_occlusion();
    ensure!(
        ao.iter().all(|v| v.is_finite() && *v >= 0.15 && *v <= 1.0),
        "Invalid SSAO value"
    );
    let accelerated_ao = ao.to_vec();
    r.settings.depth_pyramid = false;
    r.render(&p, origin, 5)?;
    let ao_error = accelerated_ao
        .iter()
        .zip(r.ambient_occlusion())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    ensure!(ao_error < 0.00001, "Depth pyramid changed AO: {ao_error}");
    r.settings.depth_pyramid = true;
    let ao = r.ambient_occlusion();
    let minimum = ao.iter().copied().fold(1.0f32, f32::min);
    ensure!(minimum < 0.9, "SSAO failed to darken a floor/wall corner");
    // The wall's near face is z=-3. Radius 1 must reach floor receivers
    // 0.65..0.9 m away, while receivers beyond 1.05 m remain unoccluded.
    let f=p.forward();let right=p.right();let up=right.cross(f);
    let tangent=(FOV_Y_DEGREES.to_radians()*0.5).tan();
    let mut metre_band=0;let mut outside=0;
    for (i,h) in r.visibility().iter().enumerate() {
        if h.valid==0 || h.normal_depth[1]<0.99 {continue;}
        let x=(i as u32%r.width) as f32;let y=(i as u32/r.width) as f32;
        let ray=(f+right*((x+0.5)/r.width as f32*2.0-1.0)*tangent*r.width as f32/r.height as f32
            -up*((y+0.5)/r.height as f32*2.0-1.0)*tangent).normalize();
        let point=p.eye()+ray*h.normal_depth[3];let distance=point.z+3.0;
        if distance>0.65 && distance<0.9 && ao[i]<0.99 {metre_band+=1;}
        if distance>1.05 {outside+=1;ensure!((ao[i]-1.0).abs()<0.00001,"AO exceeds its one-metre radius");}
    }
    ensure!(metre_band>10 && outside>100,"AO metre footprint not covered: {metre_band}/{outside}");
    r.settings.ao_strength=8.0;
    r.render(&p,origin,5)?;
    ensure!(r.ambient_occlusion().iter().zip(&accelerated_ao).all(|(a,b)| *a<=*b+0.00001),"AO strength is not monotonic");
    r.settings.ao_strength=2.0;

    ensure!(r.ambient_occlusion().iter().any(|v| *v > 0.99), "SSAO darkened every sample");
    r.settings.ssao = false;
    r.render(&p, origin, 5)?;
    ensure!(
        r.ambient_occlusion().iter().all(|v| *v == 1.0),
        "SSAO toggle failed"
    );
    println!("SSAO passed: corner darkening (min {minimum:.3}), finite bounds, disable toggle");
    // A camera on section boundaries and pitched up must not lose near-plane geometry.
    p.feet = Vec3::new(0.0, 100.0, 0.0);
    p.pitch = 0.3;
    traversal_parity(r, &p, origin, false)?;
    blocks.fill(0);
    put(&mut blocks, origin, IVec3::new(0, 319, -3), 1);
    p = Player::new(320.0);
    p.yaw = 0.0;
    p.pitch = -((p.eye().y - 319.5) / 3.0).atan();
    r.upload(&blocks)?;
    let (_, h) = shot(r, &p, origin, 0)?;
    ensure!(
        h.material == 1,
        "Ray from above world failed to enter the volume"
    );
    println!("World-boundary entry passed");
    r.settings = saved;
    Ok(())
}

pub fn resizing(r: &mut Renderer) -> Result<()> {
    let original = (r.width, r.height);
    let origin = IVec3::new(-(GPU_EDGE as i32) / 2, MIN_Y, -(GPU_EDGE as i32) / 2);
    let mut blocks = vec![0; GPU_VOXELS];
    put(&mut blocks, origin, IVec3::new(0, 101, -3), 1);
    r.upload(&blocks)?;
    let mut p = Player::new(100.0);
    p.yaw = 0.0;
    p.pitch = 0.0;
    for (width, height) in [
        (321, 239),
        (800, 450),
        (1, 33),
        (33, 1),
        (17, 9),
        (1, 1),
        original,
    ] {
        r.resize_targets(width, height)?;
        let pixels = r.render(&p, origin, 0)?;
        ensure!(
            pixels.len() == width as usize * height as usize,
            "Output buffer has old dimensions"
        );
        ensure!(
            r.visibility().len() == pixels_count(width, height)
                && r.ambient_occlusion().len() == pixels_count(width, height),
            "Visibility/AO has old dimensions"
        );
        ensure!(
            r.visibility()[center(r)].material == 1,
            "Resizing changed the center ray or descriptor bindings"
        );
        if r.settings.depth_pyramid && r.settings.ssao {
            validate_depth_pyramid(r)?;
        }
    }
    r.resize_targets(0, 0)?;
    ensure!(
        (r.width, r.height) == original,
        "Minimized window destroyed render targets"
    );
    ensure!(
        r.resize_targets(u32::MAX, u32::MAX).is_err(),
        "Impossible extent accepted"
    );
    ensure!(
        (r.width, r.height) == original,
        "Rejected extent changed render dimensions"
    );
    println!(
        "Resize passed: odd dimensions, aspect changes, 1x1, restore, zero and oversized extents"
    );
    Ok(())
}
fn pixels_count(width: u32, height: u32) -> usize {
    width as usize * height as usize
}

/// Exercise both triangle backends with exact replay times, not wall-clock animation.
pub fn polygons(r: &mut Renderer) -> Result<()> {
    use crate::meshes::MeshScene;
    let saved = r.settings;
    let replay = r.animation_time;
    r.settings = RenderSettings::default();
    r.settings.animate = false;
    r.animation_time = Some(0.0);
    let origin = IVec3::new(-56, MIN_Y, -56);
    let mut blocks = vec![0; GPU_VOXELS];
    for z in -6..=-3 {
        for y in 100..=103 {
            for x in -3..=3 {
                put(&mut blocks, origin, IVec3::new(x, y, z), 5);
            }
        }
    }
    put(&mut blocks, origin, IVec3::new(0, 101, -8), 1);
    r.upload(&blocks)?;
    let mut scene = MeshScene::empty();
    scene.add_fish(Vec3::new(-0.1, 101.5, -4.5), 0.0);
    r.set_mesh_scene(scene)?;
    let mut p = Player::new(100.0);
    p.yaw = 0.0;
    p.pitch = 0.0;
    let mut first = None;
    let mut changed = false;
    for time in [0.0, 0.45] {
        r.animation_time = Some(time);
        r.settings.hardware = true;
        r.render(&p, origin, 0)?;
        let accelerated = r.mesh_hits().to_vec();
        let h = r.visibility()[center(r)];
        ensure!(
            (h.valid & 4) != 0 && h.material == 7,
            "Animated fish did not enter the combined visibility buffer"
        );
        ensure!(
            h.water[0] > 0.0 && h.water[0] < 3.0,
            "Water attenuation did not stop at fish depth"
        );
        ensure!(
            accelerated.iter().any(|h| h.flags != 0),
            "Fish mesh produced no visible triangles"
        );
        if let Some(previous) = &first {
            let previous: &Vec<crate::meshes::MeshHit> = previous;
            changed = previous
                .iter()
                .zip(&accelerated)
                .any(|(a, b)| a.normal_depth != b.normal_depth);
        } else {
            first = Some(accelerated.clone());
        }
        r.settings.hardware = false;
        r.render(&p, origin, 0)?;
        let mut edge_differences = 0;
        for (a, b) in accelerated.iter().zip(r.mesh_hits()) {
            if a.material != b.material {
                edge_differences += 1;
                continue;
            }
            if a.flags != 0 {
                ensure!(
                    (a.normal_depth[3] - b.normal_depth[3]).abs() < 0.005,
                    "Ray-query/software triangle depth mismatch"
                );
            }
        }
        ensure!(
            edge_differences <= 8,
            "Triangle backends disagree away from silhouette edges: {edge_differences} pixels"
        );
    }
    ensure!(
        changed,
        "Skeletal/instance animation did not change actual visibility"
    );
    // Replay fixed fish from several submerged positions, including a voxel/chunk boundary.
    r.animation_time = Some(0.0);
    r.scene.update(0.0);
    for hardware in [false, true] {
        if hardware && !r.ray_query_supported() {
            continue;
        }
        r.settings.hardware = hardware;
        let target = Vec3::from_slice(&r.scene.objects[0].world[3]);
        for eye in [
            Vec3::new(0.5, 101.5, -3.2),
            Vec3::new(-1.1, 101.5, -3.6),
            Vec3::new(1.4, 101.7, -4.0),
        ] {
            let direction = (target - eye).normalize();
            let mut camera = Player::new(eye.y - 1.62);
            camera.feet.x = eye.x;
            camera.feet.z = eye.z;
            camera.yaw = direction.x.atan2(-direction.z);
            camera.pitch = direction.y.asin();
            r.render(&camera, origin, 0)?;
            let hit = r.visibility()[center(r)];
            ensure!(
                hit.material == 7 && hit.valid & 4 != 0,
                "Fish disappeared underwater at {eye:?}, hardware {hardware}"
            );
            ensure!(
                hit.water[2] > 0.5 && hit.water[0] > 0.0,
                "Underwater fish lost water path"
            );
        }
    }
    r.animation_time = Some(0.45);
    if r.ray_query_supported() {
        r.settings.hardware = true;
        r.settings.rebuild_blas = true;
        r.render(&p, origin, 0)?;
        let rebuild = r.mesh_hits()[center(r)];
        r.settings.rebuild_blas = false;
        r.render(&p, origin, 0)?;
        let refit = r.mesh_hits()[center(r)];
        ensure!(
            rebuild.material == refit.material
                && (rebuild.normal_depth[3] - refit.normal_depth[3]).abs() < 0.005,
            "BLAS rebuild and refit differ"
        );
    }
    println!("Fish passed: skeletal deformation, instance movement, water clipping, multiple underwater viewpoints; ray-query/software and BLAS-mode comparisons {}",if r.ray_query_supported() {"passed"}else{"unavailable (software fallback checked)"});

    blocks.fill(0);
    put(&mut blocks, origin, IVec3::new(0, 101, -5), 1);
    r.upload(&blocks)?;
    let mut scene = MeshScene::empty();
    scene.add_card_fixture(Vec3::new(0.5, 100.62, -3.0));
    r.set_mesh_scene(scene)?;
    for hardware in [false, true] {
        if hardware && !r.ray_query_supported() {
            continue;
        }
        r.settings.hardware = hardware;
        r.settings.alpha_test = true;
        r.render(&p, origin, 0)?;
        ensure!(
            r.visibility()[center(r)].material == 1,
            "Alpha hole hid opaque background"
        );
        ensure!(
            r.visibility()
                .iter()
                .any(|h| h.material == 9 && h.valid & 6 == 6),
            "Accepted foliage lacks mesh/alpha flags"
        );
        r.settings.alpha_test = false;
        r.render(&p, origin, 0)?;
        ensure!(
            r.visibility()[center(r)].material == 9,
            "Alpha toggle did not restore full card coverage"
        );
    }
    // Put the card off-camera, centered on the sunlight ray from the receiver.
    r.settings.alpha_test = true;
    r.settings.sun_azimuth = 60.0;
    r.settings.sun_elevation = 20.0;
    let f = p.forward();
    let ray = (f + p.right()
        * ((1.0 / r.width as f32) * r.width as f32 / r.height as f32)
        * (FOV_Y_DEGREES.to_radians() * 0.5).tan()
        - p.right().cross(f) * (1.0 / r.height as f32) * (FOV_Y_DEGREES.to_radians() * 0.5).tan())
    .normalize();
    let receiver = p.eye() + ray * ((-4.0 - p.eye().z) / ray.z);
    let az = 60.0f32.to_radians();
    let el = 20.0f32.to_radians();
    let sun = Vec3::new(az.sin() * el.cos(), el.sin(), az.cos() * el.cos());
    let target = receiver + sun * (1.0 / sun.z);
    let mut scene = MeshScene::empty();
    scene.add_card_fixture(target - Vec3::Y);
    r.set_mesh_scene(scene)?;
    for hardware in [false, true] {
        if hardware && !r.ray_query_supported() {
            continue;
        }
        r.settings.hardware = hardware;
        r.settings.alpha_test = true;
        let (clear, h) = shot(r, &p, origin, 7)?;
        ensure!(h.material == 1, "Shadow fixture card entered primary ray");
        r.settings.alpha_test = false;
        let (blocked, _) = shot(r, &p, origin, 7)?;
        ensure!(
            clear == 0xffffff && blocked == 0,
            "Alpha cutout was not respected by mesh sun-shadow rays ({clear:x}/{blocked:x})"
        );
    }
    println!("Foliage passed: primary alpha holes, visibility flags, shadow alpha holes on all supported backends");
    r.settings = saved;
    r.animation_time = replay;
    Ok(())
}

/// Resize world buffers both ways and verify the range clips opaque and polygon hits alike.
pub fn view_distance(r: &mut Renderer) -> Result<()> {
    use crate::meshes::MeshScene;
    let saved = r.settings;
    let replay = r.animation_time;
    let extent = (r.width, r.height);
    r.resize_targets(65, 49)?;
    r.settings = RenderSettings::default();
    r.settings.shadows = false;
    r.settings.ssao = false;
    r.animation_time = Some(0.0);
    let mut camera = Player::new(100.0);
    camera.yaw = 0.0;
    camera.pitch = 0.0;
    let maximum = r.max_view_chunks().min(NEAR_VIEW_CHUNKS);
    for view in [1, maximum, 2, maximum, 1] {
        let radius = view + 1;
        let edge = ((radius * 2 + 1) as usize) * EDGE;
        let origin = IVec3::new(-radius * EDGE as i32, MIN_Y, -radius * EDGE as i32);
        let mut blocks = vec![0; edge * edge * HEIGHT];
        let distance = if view == 1 {
            10
        } else {
            view * EDGE as i32 - 3
        };
        let point = IVec3::new(0, 101, -distance);
        let q = point - origin;
        blocks[q.x as usize + edge * (q.z as usize + edge * q.y as usize)] = 1;
        r.upload(&blocks)?;
        r.set_mesh_scene(MeshScene::empty())?;
        r.settings.view_chunks = view;
        let (_, h) = shot(r, &camera, origin, 0)?;
        ensure!(h.material == 1, "Expanded window failed at {view} chunks");
        if view > 1 {
            r.settings.view_chunks = 1;
            let (_, h) = shot(r, &camera, origin, 0)?;
            ensure!(h.valid == 0, "Voxel hit beyond selected range");
        }
        let mut scene = MeshScene::empty();
        scene.add_fish(Vec3::new(-0.1, 101.62, -distance as f32 + 1.5), 0.0);
        r.set_mesh_scene(scene)?;
        for hardware in [false, true] {
            if hardware && !r.ray_query_supported() {
                continue;
            }
            r.settings.hardware = hardware;
            r.settings.view_chunks = view;
            let (_, h) = shot(r, &camera, origin, 0)?;
            ensure!(
                h.material == 7,
                "Polygon range mismatch, {view} chunks hardware {hardware}"
            );
            if view > 1 {
                r.settings.view_chunks = 1;
                let (_, h) = shot(r, &camera, origin, 0)?;
                ensure!(h.valid == 0, "Polygon hit beyond selected range");
            }
        }
    }
    r.settings = saved;
    r.animation_time = replay;
    r.resize_targets(extent.0, extent.1)?;
    println!("View-distance passed: 1 -> {maximum} -> 2 -> {maximum} -> 1 chunks; GPU world/occupancy buffers and both triangle backends");
    Ok(())
}

/// Compare grazing cube-edge rays with a double-precision slab reference.
pub fn voxel_edges(r:&mut Renderer)->Result<()> {
    use crate::meshes::MeshScene;
    let saved=r.settings;let extent=(r.width,r.height);
    r.resize_targets(1,1)?;r.settings=RenderSettings::default();
    r.settings.shadows=false;r.settings.ssao=false;r.settings.meshes=false;
    let edge=5*EDGE;let mut checks=0;let mut failures=Vec::new();
    for shift in [0,512,-1024,10000] {
        let origin=IVec3::new(shift-32,MIN_Y,shift-32);
        for adjacent in [false,true] {
            let cubes=if adjacent {vec![IVec3::new(shift,100,shift-1),IVec3::new(shift+1,100,shift-1)]}else{vec![IVec3::new(shift,100,shift-1)]};
            let mut blocks=vec![0u8;edge*edge*HEIGHT];
            for cube in &cubes {let q=*cube-origin;blocks[q.x as usize+edge*(q.z as usize+edge*q.y as usize)]=1;}
            r.upload(&blocks)?;r.set_mesh_scene(MeshScene::empty())?;
            for delta in [-0.01,-0.001,0.00001,0.00003,0.0001,0.0003,0.001,0.01] {
                let mut camera=Player::new(100.38);
                camera.feet=Vec3::new(shift as f32+0.5,102.0-1.62,shift as f32+2.0);
                let target=Vec3::new(shift as f32+1.0-delta,101.0-delta,shift as f32-0.5);
                let direction=(target-camera.eye()).normalize();
                camera.yaw=direction.x.atan2(-direction.z);camera.pitch=direction.y.asin();
                let ray=camera.forward().as_dvec3().normalize();let eye=camera.eye().as_dvec3();
                let mut expected=f64::INFINITY;
                for cube in &cubes {
                    let low=cube.as_dvec3();let high=low+glam::DVec3::ONE;
                    let mut near=0.0f64;let mut far=128.0f64;
                    for axis in 0..3 {
                        if ray[axis].abs()<1e-20 {if eye[axis]<low[axis] || eye[axis]>=high[axis] {far= -1.0;}}
                        else {let a=(low[axis]-eye[axis])/ray[axis];let b=(high[axis]-eye[axis])/ray[axis];near=near.max(a.min(b));far=far.min(a.max(b));}
                    }
                    if far>near+0.000001 {expected=expected.min(near);}
                }
                for hierarchy in [false,true] {
                    r.settings.hierarchy=hierarchy;
                    let (_,hit)=shot(r,&camera,origin,0)?;checks+=1;
                    if (expected.is_finite() && (hit.valid==0 || (hit.normal_depth[3] as f64-expected).abs()>0.002)) || (!expected.is_finite() && hit.valid!=0) {
                        failures.push(format!("shift {shift}, adjacent {adjacent}, delta {delta}, hierarchy {hierarchy}, hit {}, depth {}, expected {expected}",hit.valid,hit.normal_depth[3]));
                    }
                }
            }
        }
    }
    r.settings=saved;r.resize_targets(extent.0,extent.1)?;
    ensure!(failures.is_empty(),"Grazing block-edge holes: {}/{} rays failed; {}",failures.len(),checks,failures.first().cloned().unwrap_or_default());
    println!("Voxel edge coverage passed: {checks} grazing/shared-edge rays, hierarchy on/off, positive/negative/large coordinates");Ok(())
}

/// Exercise distant pages independently from near-world allocation.
pub fn lod(r:&mut Renderer)->Result<()> {
    use crate::meshes::MeshScene;
    let saved=r.settings;let extent=(r.width,r.height);
    r.resize_targets(65,49)?;r.settings=RenderSettings::default();
    r.settings.ssao=false;r.settings.shadows=false;r.settings.meshes=false;
    let radius=NEAR_VIEW_CHUNKS+1;
    let edge=(radius*2+1) as usize*EDGE;
    let origin=IVec3::new(-radius*16,MIN_Y,-radius*16);
    r.upload(&vec![0;edge*edge*HEIGHT])?;r.set_mesh_scene(MeshScene::empty())?;
    let mut camera=Player::new(100.0);camera.yaw=0.0;camera.pitch=0.0;
    for (distance,scale) in [(300,1),(700,2),(MAX_VIEW_CHUNKS*16-3,1),(512,4),(512,8),(512,16)] {
        let block=IVec3::new(0,101,-distance);
        r.lod.as_mut().unwrap().fixture((0,block.z.div_euclid(16)),block,scale,12);
        r.settings.view_chunks=MAX_VIEW_CHUNKS;
        let (_,h)=shot(r,&camera,origin,0)?;
        let expected=(distance.div_euclid(scale as i32)*scale as i32-scale as i32) as f32+0.5;
        ensure!(h.material==12 && (h.normal_depth[3]-expected).abs()<0.02,"Sparse LOD page missing at {distance}m: material {}, depth {}, steps {}, sky {}",h.material,h.normal_depth[3],h.water[3],r.sky_only);
        r.settings.lod_pixels=0.0001;
        r.lod.as_mut().unwrap().fixture_svo((0,block.z.div_euclid(16)),block,scale,12);
        let (_,svo)=shot(r,&camera,origin,0)?;
        ensure!(svo.material==h.material && (svo.normal_depth[3]-h.normal_depth[3]).abs()<0.02,"SVO/dense page traversal mismatch at {distance}m");
        // Do not allow earlier fixtures to occlude the next target.
        let world=crate::world::World::new(42);
        r.settings.view_chunks=1;
        r.prepare_lods(&camera,&world)?;
        let (_,h)=shot(r,&camera,origin,0)?;
        ensure!(h.valid==0,"Evicted LOD page remained visible");
    }
    // The occupied slab starts inside a 16m skip cell; a steep ray must
    // stop at that exact slab crossing instead of skipping the whole page.
    camera.feet.y=500.0;
    let ray=(Vec3::new(0.5,100.5,-298.0)-camera.eye()).normalize();camera.pitch=ray.y.asin();
    let block=IVec3::new(0,100,-300);
    r.lod.as_mut().unwrap().fixture((0,block.z.div_euclid(16)),block,4,12);
    r.settings.view_chunks=64;
    let (_,h)=shot(r,&camera,origin,0)?;
    let expected=(-296.0-camera.eye().z)/ray.z;
    ensure!(h.material==12 && (h.normal_depth[3]-expected).abs()<0.03,"LOD slab skip crossed occupied space");
    let world=crate::world::World::new(42);r.settings.view_chunks=1;
    r.prepare_lods(&camera,&world)?;r.render(&camera,origin,0)?;
    // CSM rays can begin several kilometres away. At these ray distances a
    // fixed 0.0001 epsilon rounds to zero and stalls on negative-facing planes.
    camera.feet=Vec3::new(0.5,100.0,5000.5);camera.pitch=0.0;
    let block=IVec3::new(0,101,-300);
    r.lod.as_mut().unwrap().fixture((0,block.z.div_euclid(16)),block,1,12);
    r.settings.view_chunks=512; // QA-only long ray, beyond the UI's 64-chunk cap.
    let (_,h)=shot(r,&camera,origin,0)?;
    ensure!(h.material==12 && (h.normal_depth[3]-5299.5).abs()<0.02 && h.water[3]<64.0,"Long-distance float32 traversal failed to advance");
    r.settings.view_chunks=1;r.prepare_lods(&camera,&world)?;r.render(&camera,origin,0)?;
    r.settings=saved;r.resize_targets(extent.0,extent.1)?;
    println!("Sparse LOD passed: all five scales, 300/700/1021m pages, exact depth/slab entry, 5.3km precision, eviction, bounded near window");
    Ok(())
}

pub fn lod_streaming(r:&mut Renderer)->Result<()> {
    use crate::{meshes::MeshScene,world::World};
    let saved=r.settings;let extent=(r.width,r.height);
    r.resize_targets(161,91)?;r.settings=RenderSettings::default();
    r.settings.ssao=false;r.settings.shadows=false;r.settings.meshes=false;
    r.settings.view_chunks=std::env::var("BLOCKWORLD_LOD_TEST_VIEW").ok().and_then(|v|v.parse::<i32>().ok()).unwrap_or(64).clamp(9,MAX_VIEW_CHUNKS);
    let radius=NEAR_VIEW_CHUNKS+1;let edge=(radius*2+1) as usize*EDGE;
    let origin=IVec3::new(-radius*16,MIN_Y,-radius*16);
    r.upload(&vec![0;edge*edge*HEIGHT])?;r.set_mesh_scene(MeshScene::empty())?;
    let world=World::new(42);let mut camera=Player::new(180.0);camera.yaw=0.0;camera.pitch= -0.22;
    let expected=(r.settings.view_chunks as usize*2+1).pow(2)-(NEAR_VIEW_CHUNKS as usize*2+1).pow(2);
    let residency_height=std::env::var("BLOCKWORLD_LOD_TEST_HEIGHT").ok().and_then(|v|v.parse::<u32>().ok()).unwrap_or(r.height);
    let start=std::time::Instant::now();let mut first=None;
    loop {
        r.lod.as_mut().unwrap().prepare(camera.eye(),r.settings.view_chunks,residency_height,r.settings.lod_pixels,&world)?;
        r.render(&camera,origin,0)?;
        let lod=r.lod.as_ref().unwrap();let pages:usize=lod.levels.iter().sum();
        if pages>0 && first.is_none(){first=Some(start.elapsed());}
        if pages==expected {break;}
        ensure!(start.elapsed().as_secs()<if r.settings.view_chunks>64 {180} else {60},"LOD streaming timed out at {pages}/{expected} pages");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let lod=r.lod.as_ref().unwrap();
    ensure!(lod.bytes<256*1024*1024 && r.world_edge==edge,"LOD range expanded the dense window or exceeded its budget");
    if residency_height==1080 {ensure!(lod.effective_pixels==r.settings.lod_pixels && lod.levels[0]==expected,"1080p one-pixel test was silently coarsened");}
    let distant=r.visibility().iter().filter(|h|h.valid!=0 && h.normal_depth[3]>160.0).count();
    ensure!(distant>50,"No distant terrain after LOD streaming");
    println!("LOD residency target {:.2}px at {}px height",lod.effective_pixels,residency_height);
    println!("LOD streaming passed: {expected} pages in {:.2}s, first page {:.3}s, levels {:?}, {:.2} MiB, {distant} distant pixels",start.elapsed().as_secs_f32(),first.unwrap().as_secs_f32(),lod.levels,lod.bytes as f32/1048576.0);
    let merged=r.visibility().to_vec();
    r.settings.lod_merging=false;r.render(&camera,origin,0)?;
    for (a,b) in merged.iter().zip(r.visibility().iter()) {
        ensure!(a.material==b.material && (a.normal_depth[3]-b.normal_depth[3]).abs()<0.02,"Cross-chunk merge changed primary visibility");
    }
    let merged_steps:f64=merged.iter().map(|h|h.water[3] as f64).sum();
    let reference_steps:f64=r.visibility().iter().map(|h|h.water[3] as f64).sum();
    println!("Cross-chunk merged/unmerged visibility parity passed; primary steps {merged_steps:.0}/{reference_steps:.0}");
    r.settings.view_chunks=1;r.prepare_lods(&camera,&world)?;r.render(&camera,origin,0)?;
    ensure!(r.lod.as_ref().unwrap().bytes==0,"Distant pages not evicted on range reduction");
    r.settings=saved;r.resize_targets(extent.0,extent.1)?;Ok(())
}

/// An unrelated edit cannot purge resident terrain; a dirty chunk swaps alone.
pub fn lod_edits(r:&mut Renderer)->Result<()> {
    use crate::{meshes::MeshScene,world::{World,Block}};
    let saved=r.settings;let extent=(r.width,r.height);
    r.resize_targets(161,121)?;r.settings=RenderSettings::default();
    r.settings.ssao=false;r.settings.shadows=false;r.settings.meshes=false;
    r.settings.view_chunks=10;r.settings.lod_pixels=1.0;
    let origin=IVec3::new(-144,MIN_Y,-144);
    r.upload(&vec![0;GPU_EDGE*GPU_EDGE*HEIGHT])?;r.set_mesh_scene(MeshScene::empty())?;
    let mut world=World::new(42);let mut camera=Player::new(200.0);
    camera.yaw=std::f32::consts::FRAC_PI_2;camera.pitch=0.0;
    let expected=152usize;let start=std::time::Instant::now();
    loop {
        r.prepare_lods(&camera,&world)?;r.render(&camera,origin,0)?;
        if r.lod.as_ref().unwrap().levels.iter().sum::<usize>()==expected {break;}
        ensure!(start.elapsed().as_secs()<5,"Initial edit-test pages timed out");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let initial=r.lod.as_ref().unwrap().uploaded;
    let epoch=r.lod.as_ref().unwrap().epoch;
    world.edits.insert(IVec3::new(0,199,0),Block::Wood);
    for _ in 0..10 {
        r.prepare_lods(&camera,&world)?;
        let lod=r.lod.as_ref().unwrap();
        ensure!(lod.levels.iter().sum::<usize>()==expected && lod.uploaded==initial && lod.epoch==epoch,"Unrelated edit purged or reuploaded distant pages");
        r.render(&camera,origin,0)?;
    }
    let point=IVec3::new(160,201,0);
    for (material,want,changes) in [(Block::Wood,6,1),(Block::Air,0,2)] {
        world.edits.insert(point,material);let deadline=std::time::Instant::now();
        loop {
            // Chunk contents, rather than the source of an entity/player edit,
            // determine dirtiness.
            r.prepare_lods(&camera,&world)?;
            let lod=r.lod.as_ref().unwrap();
            ensure!(lod.levels.iter().sum::<usize>()==expected,"Dirty chunk rebuild removed still-visible pages");
            ensure!(lod.uploaded<=initial+changes,"Unchanged chunks were rebuilt");
            r.render(&camera,origin,0)?;
            if r.lod.as_ref().unwrap().uploaded==initial+changes {break;}
            ensure!(deadline.elapsed().as_secs()<5,"Changed LOD chunk failed to publish");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        ensure!(r.visibility()[center(r)].material==want,"Replacement LOD did not reflect edit");
    }
    r.settings.view_chunks=1;r.prepare_lods(&camera,&world)?;r.render(&camera,origin,0)?;
    r.settings=saved;r.resize_targets(extent.0,extent.1)?;
    println!("LOD edits passed: 152 resident pages retained; near edit uploaded zero pages; distant place/delete replaced exactly one page each");Ok(())
}

/// A single face fills the screen. Hidden fish must never survive the primary ray pass.
pub fn close_up(r: &mut Renderer) -> Result<()> {
    use crate::meshes::MeshScene;
    let saved = r.settings;
    let extent = (r.width, r.height);
    r.resize_targets(65, 49)?;
    r.settings = RenderSettings::default();
    r.animation_time = Some(0.0);
    let radius = 2;
    let edge = 5 * EDGE;
    let origin = IVec3::new(-radius * 16, MIN_Y, -radius * 16);
    let mut blocks = vec![0; edge * edge * HEIGHT];
    let point = IVec3::new(0, 99, 0) - origin;
    blocks[point.x as usize + edge * (point.z as usize + edge * point.y as usize)] = 2;
    r.upload(&blocks)?;
    let mut scene = MeshScene::empty();
    scene.add_fish(Vec3::new(-0.1, 98.0, 0.5), 0.0);
    r.set_mesh_scene(scene)?;
    let mut camera = Player::new(100.05 - 1.62);
    camera.pitch = -std::f32::consts::FRAC_PI_2;
    for hardware in [false, true] {
        if hardware && !r.ray_query_supported() {
            continue;
        }
        r.settings.hardware = hardware;
        r.render(&camera, origin, 0)?;
        ensure!(
            r.visibility()
                .iter()
                .all(|h| h.material == 2 && h.normal_depth[3] < 0.2 && h.water[3] <= 2.0),
            "Close-up must resolve one block within two primary steps"
        );
        ensure!(
            r.mesh_hits().iter().all(|h| h.flags == 0),
            "Hidden polygon survived voxel-depth clipping"
        );
    }
    r.settings = saved;
    r.resize_targets(extent.0, extent.1)?;
    println!("Close-up passed: one block / two primary steps per pixel; hidden polygons rejected on both backends");
    Ok(())
}

/// Exercise the actual worker and generation ownership, not a synchronous imitation.
pub fn streaming(r: &mut Renderer) -> Result<()> {
    use crate::{
        streaming::{Completed, Request, Streamer},
        world::{Block, World},
    };
    use std::{
        collections::HashMap,
        thread,
        time::{Duration, Instant},
    };
    let extent = (r.width, r.height);
    r.resize_targets(65, 49)?;
    r.settings = RenderSettings::default();
    r.settings.view_chunks = 1;
    r.settings.ssao = false;
    let mut worker = Streamer::new(r.stream_context(), World::new(42))?;
    let mut edits = HashMap::new();
    let mut evictions = 0;
    for (index, cx) in [0, 8, 16, 0].into_iter().enumerate() {
        let generation = index as u64 + 1;
        let target = IVec3::new(cx * 16, 101, -3);
        edits.insert(target, Block::Wood);
        worker.request(Request {
            generation,
            center: (cx, 0),
            view: 1,
            budget: 1,
            edits: edits.clone(),
            time: 0.0,
            players: vec![crate::entities::ActivePlayer{id:0,position:Vec3::new(cx as f32*16.0,100.0,0.0)}],
        })?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let packet = loop {
            ensure!(Instant::now() < deadline, "World streaming timed out");
            match worker.poll() {
                Some(Completed::World(packet)) => break *packet,
                Some(Completed::Error(e)) => anyhow::bail!(e),
                _ => thread::sleep(Duration::from_millis(1)),
            }
        };
        ensure!(
            packet.cache.chunks == 25,
            "Pinned working set must be retained under a tiny budget"
        );
        evictions += packet.cache.evicted;
        ensure!(
            packet.world.get(IVec3::new(0, 101, -3))
                == if cx == 0 { Block::Wood } else { Block::Air },
            "Eviction/reload edit state mismatch"
        );
        let origin = packet.gpu.origin;
        worker.retire_world(r.adopt_world(packet.gpu));
        worker.recycle(r.adopt_mesh(packet.frame, packet.ready));
        let mut camera = Player::new(100.0);
        camera.feet.x = cx as f32 * 16.0 + 0.5;
        camera.yaw = 0.0;
        camera.pitch = 0.0;
        let (_, hit) = shot(r, &camera, origin, 0)?;
        ensure!(hit.material == 6, "Asynchronous world upload lost edit");
        // Rotate through more than the number of ring slots, including refit/rebuild.
        for frame in 0..5 {
            let mut players=vec![crate::entities::ActivePlayer{id:0,position:camera.feet}];
            if let Some(fish)=r.scene.objects.iter().find(|o| o.info[2]==0) {
                players.push(crate::entities::ActivePlayer{id:1,position:Vec3::from_slice(&fish.world[3])});
            }
            worker.animate(generation, 0.1 + frame as f32 * 0.05, frame == 3, players)?;
            let before = r.snapshot_time;
            r.render(&camera, origin, 0)?;
            ensure!(
                r.snapshot_time == before,
                "Pending AS work mutated active shader data"
            );
            let deadline = Instant::now() + Duration::from_secs(30);
            let (mesh, ready) = loop {
                ensure!(Instant::now() < deadline, "AS ring timed out");
                match worker.poll() {
                    Some(Completed::Animation(Some(packet))) => break *packet,
                    Some(Completed::Error(e)) => anyhow::bail!(e),
                    _ => thread::sleep(Duration::from_millis(1)),
                }
            };
            ensure!(mesh.generation == generation, "Stale AS generation");
            worker.recycle(r.adopt_mesh(mesh, ready));
            r.render(&camera, origin, 0)?;
            if let Some(fish) = r.scene.objects.iter().find(|o| o.info[2] == 0 && o.info[3]&crate::entities::INACTIVE==0) {
                let target = Vec3::from_slice(&fish.world[3]);
                let eye = target + Vec3::new(0.0, 0.05, 1.2);
                let mut swimmer = Player::new(eye.y - 1.62);
                swimmer.feet.x = eye.x;
                swimmer.feet.z = eye.z;
                swimmer.yaw = 0.0;
                swimmer.pitch = (-0.05f32).atan2(1.2);
                r.settings.hardware = true;
                r.render(&swimmer, origin, 0)?;
                let accelerated = r.mesh_hits().to_vec();
                ensure!(
                    accelerated.iter().any(|h| h.flags != 0),
                    "Async ring fish produced no geometry"
                );
                r.settings.hardware = false;
                r.render(&swimmer, origin, 0)?;
                let mismatches = accelerated
                    .iter()
                    .zip(r.mesh_hits())
                    .filter(|(a, b)| {
                        a.material != b.material
                            || (a.flags != 0
                                && (a.normal_depth[3] - b.normal_depth[3]).abs() > 0.005)
                    })
                    .count();
                ensure!(
                    mismatches <= 8,
                    "AS ring and shader geometry mismatch: {mismatches} pixels"
                );
                r.settings.hardware = true;
            }
        }
    }
    ensure!(evictions >= 50, "Distant chunks were not evicted");
    drop(worker);
    r.resize_targets(extent.0, extent.1)?;
    println!("Async streaming passed: world uploads, tiny-budget eviction, edit replay, three-slot AS refit/rebuild, previous-snapshot reuse");
    Ok(())
}

/// Controlled pool and a wall visible both directly and through its screen-space reflection.
pub fn water_effects(r: &mut Renderer) -> Result<()> {
    use crate::meshes::MeshScene;
    let saved = r.settings;
    let replay = r.water_time;
    let extent = (r.width, r.height);
    r.resize_targets(161, 97)?;
    r.settings = RenderSettings::default();
    r.settings.meshes = false;
    r.settings.ssao = false;
    r.settings.shadows = false;
    r.settings.waves = false;
    r.water_time = Some(0.0);
    let origin = IVec3::new(-(GPU_EDGE as i32) / 2, MIN_Y, -(GPU_EDGE as i32) / 2);
    let mut blocks = vec![0; GPU_VOXELS];
    for z in -24..=12 {
        for x in -18..=18 {
            put(&mut blocks, origin, IVec3::new(x, 98, z), 4);
            for y in 99..=100 {
                put(&mut blocks, origin, IVec3::new(x, y, z), 5);
            }
        }
    }
    for y in 101..=109 {
        for x in -9..=9 {
            put(
                &mut blocks,
                origin,
                IVec3::new(x, y, -9),
                if x < 0 { 6 } else { 1 },
            );
        }
    }
    r.upload(&blocks)?;
    r.set_mesh_scene(MeshScene::empty())?;
    let mut camera = Player::new(103.0 - 1.62);
    camera.feet.z = 6.5;
    camera.yaw = 0.0;
    camera.pitch = -0.28;
    let confidence = r.render(&camera, origin, 10)?.to_vec();
    let reflected = confidence.iter().filter(|&&v| v & 255 > 20).count();
    ensure!(
        reflected > 10,
        "SSR fixture produced no reflected wall: {reflected} pixels"
    );
    let with = r.render(&camera, origin, 0)?.to_vec();
    r.settings.ssr = false;
    let without = r.render(&camera, origin, 0)?.to_vec();
    ensure!(
        with.iter()
            .zip(&without)
            .zip(&confidence)
            .filter(|((a, b), c)| a != b && **c & 255 > 20)
            .count()
            > 10,
        "SSR toggle failed to change reflected color"
    );
    r.settings.waves = true;
    r.water_time = Some(0.0);
    let first = r.render(&camera, origin, 11)?.to_vec();
    let depths = r
        .visibility()
        .iter()
        .map(|h| (h.normal_depth, h.water_normal_depth, h.water))
        .collect::<Vec<_>>();
    r.water_time = Some(1.25);
    let second = r.render(&camera, origin, 11)?.to_vec();
    ensure!(
        first.iter().zip(&second).filter(|(a, b)| a != b).count() > 50,
        "Scrolling wave layers did not change water normals"
    );
    ensure!(
        depths
            .iter()
            .zip(r.visibility())
            .all(|((opaque, water, path), h)| *opaque == h.normal_depth
                && *water == h.water_normal_depth
                && *path == h.water),
        "Bump mapping changed geometric visibility or attenuation lengths"
    );
    r.water_time = Some(0.0);
    ensure!(
        first == r.render(&camera, origin, 11)?,
        "Wave replay time is not deterministic"
    );
    r.settings.stochastic_waves=false;
    let tiled=r.render(&camera,origin,11)?.to_vec();
    ensure!(first.iter().zip(&tiled).filter(|(a,b)|a!=b).count()>50,"Stochastic tiling did not change wave normals");
    r.settings.stochastic_waves=true;
    ensure!(first==r.render(&camera,origin,11)?,"Stochastic wave toggle did not restore deterministic normals");
    r.water_time=Some(0.0001);
    let close=r.render(&camera,origin,11)?.to_vec();
    let channel_error=|a:u32,b:u32| [0,8,16].into_iter().map(|shift|((a>>shift)&255).abs_diff((b>>shift)&255)).max().unwrap();
    ensure!(first.iter().zip(&close).all(|(a,b)|channel_error(*a,*b)<=2),"Stochastic wave animation has discontinuous random phases");
    for (h,&pixel) in r.visibility().iter().zip(&close) {
        if h.water[1]>0.5 && h.water_normal_depth[3]>0.001 && h.water_normal_depth[3]<r.settings.view_chunks as f32*16.0 {
            let n=Vec3::new(((pixel>>16)&255) as f32,((pixel>>8)&255) as f32,(pixel&255) as f32)/127.5-Vec3::ONE;
            ensure!((n.length_squared()-1.0).abs()<0.04,"Invalid stochastic water normal");
        }
    }
    r.water_time=Some(0.0);
    r.settings.wave_warp=0.0;
    let unwarped=r.render(&camera,origin,11)?.to_vec();
    ensure!(first.iter().zip(&unwarped).filter(|(a,b)|a!=b).count()>50,"Domain warp did not change wave normals");
    r.settings.wave_warp=1.5;
    r.settings.multiscale_waves=false;
    let three_bands=r.render(&camera,origin,11)?.to_vec();
    ensure!(first.iter().zip(&three_bands).filter(|(a,b)|a!=b).count()>50,"Six-octave mode did not differ from three bands");
    r.settings.multiscale_waves=true;
    ensure!(first==r.render(&camera,origin,11)?,"Wave controls did not restore exact replay");
    let pose=(camera.feet,camera.yaw,camera.pitch);let view=r.settings.view_chunks;
    camera.feet=Vec3::new(0.5,200.0,0.5);camera.yaw=0.0;camera.pitch= -1.5;
    r.settings.view_chunks=16;r.settings.wave_distance=32.0;
    let far_macro=r.render(&camera,origin,11)?.to_vec();
    let far_coverage=r.visibility().iter().filter(|h|h.water[1]>0.5 && h.water_normal_depth[3]>32.0).count();
    ensure!(far_coverage>10,"Far-wave test has insufficient water coverage");
    ensure!(r.visibility().iter().zip(&far_macro).filter(|(h,p)|h.water[1]>0.5 && h.water_normal_depth[3]>32.0 && **p!=0x7fff7f).count()>10,"Distant macro swells became flat");
    r.water_time=Some(1.0/60.0);
    let far_next=r.render(&camera,origin,11)?.to_vec();
    ensure!(far_macro.iter().zip(&far_next).all(|(a,b)|channel_error(*a,*b)<=2),"Distant macro-wave normals flicker between frames");
    r.water_time=Some(0.0);
    r.settings.wave_distance=512.0;
    let far_detail=r.render(&camera,origin,11)?.to_vec();
    ensure!(far_macro.iter().zip(&far_detail).filter(|(a,b)|a!=b).count()>10,"Camera-distance wave blend did not affect far water");
    (camera.feet,camera.yaw,camera.pitch)=pose;r.settings.view_chunks=view;r.settings.wave_distance=128.0;
    r.settings.waves = false;
    let flat = r.render(&camera, origin, 11)?.to_vec();
    ensure!(
        r.visibility()
            .iter()
            .zip(&flat)
            .filter(|(h, _)| h.water[1] > 0.5 && h.water_normal_depth[1] > 0.9)
            .all(|(_, pixel)| *pixel == 0x7fff7f),
        "Disabled bump did not restore flat normals"
    );
    if let Some(path) = std::env::var_os("BLOCKWORLD_WATER_CAPTURE") {
        use std::io::Write;
        let path = std::path::PathBuf::from(path);
        r.resize_targets(480, 270)?;
        r.settings.ssr = true;
        r.settings.waves = true;
        r.water_time = Some(1.25);
        for (mode, output) in [
            (0, path.clone()),
            (10, path.with_extension("ssr.ppm")),
            (11, path.with_extension("normals.ppm")),
        ] {
            let pixels = r.render(&camera, origin, mode)?;
            let mut file = std::io::BufWriter::new(std::fs::File::create(output)?);
            write!(file, "P6\n480 270\n255\n")?;
            for &v in pixels {
                file.write_all(&[(v >> 16) as u8, (v >> 8) as u8, v as u8])?;
            }
        }
        r.settings.waves = false;
    }
    r.settings.ssr = true;
    r.resize_targets(97, 65)?;
    ensure!(
        r.render(&camera, origin, 10)?.iter().any(|&v| v & 255 > 20),
        "SSR lost resized HDR/depth buffers"
    );
    // Nothing above the surface: reflection rays must fall back to the sky.
    for y in 101..=109 {
        for x in -9..=9 {
            put(&mut blocks, origin, IVec3::new(x, y, -9), 0);
        }
    }
    r.upload(&blocks)?;
    r.set_mesh_scene(MeshScene::empty())?;
    ensure!(
        r.render(&camera, origin, 10)?.iter().all(|&v| v == 0),
        "SSR reflected missing/off-screen geometry"
    );
    // Geometry outside the old 32m SSR range: tall snow ridge and a voxel tree.
    for y in 111..=129 {
        for x in -18..=-8 {
            put(&mut blocks, origin, IVec3::new(x, y, -48), 12);
        }
    }
    for y in 101..=120 {
        put(&mut blocks, origin, IVec3::new(13, y, -42), 6);
    }
    for y in 118..=122 {
        for z in -44..=-40 {
            for x in 10..=16 {
                put(&mut blocks, origin, IVec3::new(x, y, z), 13);
            }
        }
    }
    r.upload(&blocks)?;
    r.set_mesh_scene(MeshScene::empty())?;
    let distant = r.render(&camera, origin, 14)?.to_vec();
    ensure!(
        distant.iter().filter(|&&v| v & 255 == 12).count() > 5,
        "Distant snowy ridge missing from reflections"
    );
    ensure!(
        distant.iter().any(|&v| v & 255 == 13 || v & 255 == 6),
        "Distant tree missing from reflections"
    );
    r.settings.frustum = false;
    ensure!(
        distant == r.render(&camera, origin, 14)?,
        "Reflections used primary-camera culling"
    );
    r.settings.frustum = true;
    camera.feet.y = 100.0 - 1.62;
    camera.pitch = 0.85;
    let fresnel = r.render(&camera, origin, 12)?.to_vec();
    let mut tir = 0;
    let mut window = 0;
    for (i, h) in r.visibility().iter().enumerate() {
        if h.water[2] < 0.5 || h.water_normal_depth[1] > -0.9 || h.water_normal_depth[3] <= 0.001 {
            continue;
        }
        let xy = Vec3::new(
            (i as u32 % r.width) as f32 + 0.5,
            (i as u32 / r.width) as f32 + 0.5,
            0.0,
        );
        let sx = xy.x / r.width as f32 * 2.0 - 1.0;
        let sy = xy.y / r.height as f32 * 2.0 - 1.0;
        let f = camera.forward();
        let right = camera.right();
        let up = right.cross(f);
        let tangent = (FOV_Y_DEGREES.to_radians() * 0.5).tan();
        let ray = (f + right * sx * (r.width as f32 / r.height as f32) * tangent
            - up * sy * tangent)
            .normalize();
        let critical = (1.0 - 1.0 / r.settings.water_ior.powi(2)).sqrt();
        if ray.y < critical - 0.02 {
            ensure!(fresnel[i] & 255 == 255, "Missing total internal reflection");
            tir += 1;
        }
        if ray.y > 0.98 {
            ensure!(
                fresnel[i] & 255 < 10,
                "Normal-incidence water/air Fresnel incorrect"
            );
            window += 1;
        }
    }
    ensure!(
        tir > 20 && window > 5,
        "Missing underwater Snell-window coverage {tir}/{window}"
    );
    ensure!(
        r.render(&camera, origin, 14)?.iter().any(|&v| v & 255 == 4),
        "Underwater reflection did not see submerged bed"
    );
    if let Some(path) = std::env::var_os("BLOCKWORLD_WATER_CAPTURE") {
        use std::io::Write;
        let path = std::path::PathBuf::from(path);
        r.resize_targets(480, 270)?;
        for (mode, output) in [
            (0, path.with_extension("underwater.ppm")),
            (12, path.with_extension("fresnel.ppm")),
        ] {
            let pixels = r.render(&camera, origin, mode)?;
            let mut file = std::io::BufWriter::new(std::fs::File::create(output)?);
            write!(file, "P6\n480 270\n255\n")?;
            for &v in pixels {
                file.write_all(&[(v >> 16) as u8, (v >> 8) as u8, v as u8])?;
            }
        }
        r.resize_targets(97, 65)?;
    }
    r.settings.water_ior = 1.0;
    ensure!(
        r.render(&camera, origin, 12)?.iter().all(|&v| v == 0),
        "Equal media must not reflect"
    );
    r.settings.water_ior = 1.333;
    camera.pitch = -0.08;
    r.settings.underwater_density = 0.2;
    let clear = r.render(&camera, origin, 0)?.to_vec();
    r.settings.underwater_density = 4.0;
    let murky = r.render(&camera, origin, 0)?.to_vec();
    ensure!(
        clear.iter().zip(&murky).filter(|(a, b)| a != b).count() > 100,
        "Underwater distance fog did not affect submerged view"
    );
    ensure!(
        r.visibility()
            .iter()
            .any(|h| h.water[2] > 0.5 && h.valid != 0 && h.water_normal_depth[3] == 0.0),
        "Bed incorrectly classified as water/air interface"
    );
    r.settings = saved;
    r.water_time = replay;
    r.resize_targets(extent.0, extent.1)?;
    println!("Water effects passed: {reflected} reflected-wall pixels, color toggle, sky fallback, distant hills/trees, underwater Fresnel/TIR/refraction, medium fog, scrolling/replayed bump normals, unchanged visibility, resized HDR targets; {:.0}x anisotropy",r.wave_anisotropy());
    Ok(())
}

/// Exercise byte unpacking across a GPU word boundary and the new material IDs.
pub fn block_materials(r: &mut Renderer) -> Result<()> {
    use crate::meshes::MeshScene;
    let saved = r.settings;
    let extent = (r.width, r.height);
    r.resize_targets(1, 1)?;
    r.settings = RenderSettings::default();
    r.settings.ssao = false;
    r.settings.shadows = false;
    r.settings.meshes = false;
    let edge = 5 * EDGE;
    let origin = IVec3::new(-32, MIN_Y, -32);
    let mut blocks = vec![0u8; edge * edge * HEIGHT];
    let ids = [1u8, 12, 13, 14, 15, 16, 17, 18, 19];
    for (i, &id) in ids.iter().enumerate() {
        let p = IVec3::new(i as i32 - 2, 101, -3) - origin;
        blocks[p.x as usize + edge * (p.z as usize + edge * p.y as usize)] = id;
    }
    r.upload(&blocks)?;
    r.set_mesh_scene(MeshScene::empty())?;
    for (i, &id) in ids.iter().enumerate() {
        let mut camera = Player::new(100.0);
        camera.feet.x = i as f32 - 1.5;
        camera.yaw = 0.0;
        camera.pitch = 0.0;
        let (pixel, h) = shot(r, &camera, origin, 3)?;
        ensure!(
            h.material == id as u32,
            "Packed block ID mismatch: expected {id}, got {}",
            h.material
        );
        ensure!(pixel != 0, "Missing 16x16 material texture {id}");
    }
    r.settings = saved;
    r.resize_targets(extent.0, extent.1)?;
    println!("Block materials passed: packed byte/word boundaries, snow/leaves, five biome materials, 16x16 atlas");
    Ok(())
}

/// Empty-frame culling must match ordinary shading and initialize QA buffers.
pub fn empty_frame(r: &mut Renderer) -> Result<()> {
    use crate::meshes::MeshScene;
    let saved = r.settings;
    let extent = (r.width, r.height);
    r.resize_targets(129, 73)?;
    r.settings = RenderSettings::default();
    let edge = 5 * EDGE;
    let origin = IVec3::new(-32, MIN_Y, -32);
    let mut blocks = vec![0u8; edge * edge * HEIGHT];
    let q = IVec3::new(0, 99, 0) - origin;
    blocks[q.x as usize + edge * (q.z as usize + edge * q.y as usize)] = 2;
    r.upload(&blocks)?;
    let mut scene = MeshScene::empty();
    scene.add_fish(Vec3::new(0.0, 98.0, 0.0), 0.0);
    r.set_mesh_scene(scene)?;
    let mut camera = Player::new(MAX_Y as f32 + 8.0);
    camera.pitch = std::f32::consts::FRAC_PI_2;
    for mode in 0..15 {
        r.settings.frustum = true;
        let fast = r.render(&camera, origin, mode)?.to_vec();
        ensure!(r.sky_only, "Empty-frame fast path was not selected");
        ensure!(
            r.visibility()
                .iter()
                .all(|h| h.valid == 0 && h.water == [0.0; 4]),
            "Stale visibility in empty frame"
        );
        ensure!(
            r.mesh_hits().iter().all(|h| h.flags == 0),
            "Stale mesh hits in empty frame"
        );
        ensure!(
            r.ambient_occlusion().iter().all(|&v| v == 1.0),
            "Stale AO in empty frame"
        );
        r.settings.frustum = false;
        ensure!(
            fast == r.render(&camera, origin, mode)?,
            "Empty-frame output differs in mode {mode}"
        );
        ensure!(
            !r.sky_only,
            "Disabled culling still selected empty-frame path"
        );
    }
    r.settings = saved;
    r.resize_targets(extent.0, extent.1)?;
    println!("Empty-frame passed: sky/debug parity, valid visibility/AO/mesh outputs, geometry behind camera");
    Ok(())
}

/// Sparse interactive writes must not resolve a previous frame's fish after it is disabled.
pub fn sparse_mesh(r: &mut Renderer) -> Result<()> {
    use crate::meshes::MeshScene;
    let saved = r.settings;
    let extent = (r.width, r.height);
    let replay = r.animation_time;
    r.resize_targets(129, 73)?;
    r.settings = RenderSettings::default();
    r.settings.frustum = false;
    r.animation_time = Some(0.0);
    let origin = IVec3::new(-32, MIN_Y, -32);
    r.upload(&vec![0u8; 80 * 80 * HEIGHT])?;
    let mut scene = MeshScene::empty();
    scene.add_fish(Vec3::new(-0.1, 101.62, -3.0), 0.0);
    r.set_mesh_scene(scene)?;
    let mut camera = Player::new(100.0);
    camera.yaw = 0.0;
    camera.pitch = 0.0;
    for hardware in [false, true] {
        if hardware && !r.ray_query_supported() {
            continue;
        }
        r.settings.hardware = hardware;
        r.settings.fish = true;
        r.sparse_mesh_validation = false;
        let reference = r.render(&camera, origin, 0)?.to_vec();
        r.sparse_mesh_validation = true;
        ensure!(
            reference == r.render(&camera, origin, 0)?,
            "Sparse mesh writes changed shading"
        );
        ensure!(
            r.mesh_hits()
                .iter()
                .any(|h| h.flags != 0 && h.ids[3] == r.mesh_epoch),
            "No current sparse mesh hits"
        );
        r.settings.fish = false;
        let without = r.render(&camera, origin, 0)?.to_vec();
        ensure!(
            r.mesh_hits()
                .iter()
                .any(|h| h.flags != 0 && h.ids[3] != r.mesh_epoch),
            "Fixture must retain stale sparse data"
        );
        ensure!(
            r.visibility().iter().all(|h| h.valid & 4 == 0),
            "Stale sparse mesh data became visible"
        );
        r.sparse_mesh_validation = false;
        ensure!(
            without == r.render(&camera, origin, 0)?,
            "Sparse misses differ from full miss writes"
        );
    }
    r.settings = saved;
    r.animation_time = replay;
    r.resize_targets(extent.0, extent.1)?;
    println!("Sparse mesh passed: shaded parity, per-frame validity, stale-hit rejection on all supported backends");
    Ok(())
}

/// Compare accelerated lighting with exact rays on real terrain, not just isolated fixtures.
pub fn acceleration(r: &mut Renderer, player: &Player, origin: IVec3) -> Result<()> {
    let saved = r.settings;
    let extent = (r.width, r.height);
    let animation = r.animation_time;
    let water = r.water_time;
    r.resize_targets(161, 97)?;
    r.animation_time = Some(0.25);
    r.water_time = Some(0.25);
    r.settings.animate = false;
    r.settings.far_shadows = false;
    let mut p = Player::new(player.feet.y);
    p.feet = player.feet;
    for (yaw, pitch, azimuth, elevation) in [
        (player.yaw, player.pitch, 35.0, 55.0),
        (1.7, -0.6, 140.0, 12.0),
        (-2.1, -0.4, 270.0, -35.0),
        (0.3, -1.4, 0.0, 90.0),
    ] {
        p.yaw = yaw;
        p.pitch = pitch;
        r.settings.sun_azimuth = azimuth;
        r.settings.sun_elevation = elevation;
        for hardware in [false, true] {
            r.settings.hardware = hardware;
            for offset in [0.0, 0.1, 4.1] {
                p.feet.x = player.feet.x + offset;
                r.animation_time = Some(0.25 + offset);
                r.settings.csm = true;
                r.settings.depth_pyramid = true;
                let fast = r.render(&p, origin, 0)?.to_vec();
                let ao = r.ambient_occlusion().to_vec();
                validate_depth_pyramid(r)?;
                r.settings.csm = false;
                r.settings.depth_pyramid = false;
                let reference = r.render(&p, origin, 0)?;
                ensure!(fast==reference,"Accelerated lighting differs: yaw {yaw}, light {azimuth}/{elevation}, offset {offset}, hardware {hardware}");
                let error = ao
                    .iter()
                    .zip(r.ambient_occlusion())
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0f32, f32::max);
                ensure!(error < 0.00001, "Depth-pyramid AO differs: {error}");
            }
        }
    }
    r.settings = saved;
    r.animation_time = animation;
    r.water_time = water;
    r.resize_targets(extent.0, extent.1)?;
    println!("Lighting acceleration passed: exact image/AO parity on terrain, cached and moved cascades, grazing/vertical/downward lights, both mesh backends");
    Ok(())
}

fn validate_depth_pyramid(r: &Renderer) -> Result<()> {
    let data = r.depth_bounds();
    for (b, h) in data.iter().zip(r.visibility()) {
        ensure!(
            (b[0] <= b[1]) == (h.valid != 0),
            "Depth base validity differs from resolved visibility"
        );
    }
    let (mut w, mut h, mut offset) = (r.width as usize, r.height as usize, 0usize);
    while w > 1 || h > 1 {
        let (nw, nh, next) = (w.div_ceil(2), h.div_ceil(2), offset + w * h);
        for y in 0..nh {
            for x in 0..nw {
                let mut bounds = [1e30f32, -1e30f32];
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (sx, sy) = (x * 2 + dx, y * 2 + dy);
                        if sx < w && sy < h {
                            let b = data[offset + sx + sy * w];
                            bounds[0] = bounds[0].min(b[0]);
                            bounds[1] = bounds[1].max(b[1]);
                        }
                    }
                }
                ensure!(
                    data[next + x + y * nw] == bounds,
                    "Incorrect min/max reduction at {w}x{h} texel {x},{y}"
                );
            }
        }
        w = nw;
        h = nh;
        offset = next;
    }
    ensure!(
        offset + 1 == data.len(),
        "Pyramid allocation/level offsets disagree"
    );
    Ok(())
}

/// An isolated flat plane has no geometry in its outward AO hemisphere.
pub fn ao_planar(r: &mut Renderer) -> Result<()> {
    let saved = r.settings;
    let extent = (r.width, r.height);
    r.settings = RenderSettings::default();
    r.settings.meshes = false;
    r.settings.shadows = false;
    let edge = 5 * EDGE;
    let origin = IVec3::new(-32, MIN_Y, -32);
    let mut blocks = vec![0u8; edge * edge * HEIGHT];
    let plane = (99 - MIN_Y) as usize;
    blocks[plane * edge * edge..(plane + 1) * edge * edge].fill(1);
    r.upload(&blocks)?;
    let mut camera = Player::new(100.0);
    let mut worst = 0.0f32;
    for (width, height) in [(321, 239), (967, 523)] {
        r.resize_targets(width, height)?;
        for pitch in [-0.08, -0.3, -0.8] {
            camera.pitch = pitch;
            camera.yaw = 0.63;
            for shift in [0.0, 0.001, 0.03] {
                camera.feet.x = 0.5 + shift;
                for pyramid in [false, true] {
                    r.settings.depth_pyramid = pyramid;
                    r.render(&camera, origin, 5)?;
                    let count = r.visibility().iter().filter(|h| h.valid != 0).count();
                    ensure!(count > 100, "Flat-plane AO fixture is empty");
                    let error = r
                        .visibility()
                        .iter()
                        .zip(r.ambient_occlusion())
                        .filter(|(h, _)| h.valid != 0)
                        .map(|(_, a)| 1.0 - a)
                        .fold(0.0f32, f32::max);
                    worst = worst.max(error);
                    ensure!(
                        r.visibility()
                            .iter()
                            .filter(|h| h.valid != 0)
                            .all(|h| h.normal_depth[0] == 0.0
                                && h.normal_depth[1] == 1.0
                                && h.normal_depth[2] == 0.0),
                        "An internal voxel face replaced the floor normal"
                    );
                    ensure!(error<0.00001,"Coplanar self-occlusion: {error}, {width}x{height}, pitch {pitch}, shift {shift}, pyramid {pyramid}");
                }
            }
        }
    }
    r.settings = saved;
    r.resize_targets(extent.0, extent.1)?;
    println!("Planar AO passed: grazing views, subpixel motion, odd extents, both depth paths; max error {worst}");
    Ok(())
}

/// Force map-only shadows even on a near receiver, to exercise the distant path.
pub fn far_shadows(r: &mut Renderer) -> Result<()> {
    use crate::meshes::MeshScene;
    let saved = r.settings;
    let extent = (r.width, r.height);
    let animation = r.animation_time;
    r.resize_targets(129, 97)?;
    r.settings = RenderSettings::default();
    r.settings.ssao = false;
    r.settings.animate = false;
    r.settings.shadow_ray_distance = 0.0;
    r.animation_time = Some(0.0);
    let origin = IVec3::new(-56, MIN_Y, -56);
    let mut blocks = vec![0u8; GPU_VOXELS];
    put(&mut blocks, origin, IVec3::new(0, 101, -5), 1);
    let mut camera = Player::new(100.0);
    camera.yaw = 0.0;
    camera.pitch = 0.0;
    r.settings.sun_azimuth = 60.0;
    r.settings.sun_elevation = 20.0;
    r.upload(&blocks)?;
    r.set_mesh_scene(MeshScene::empty())?;
    let (lit, _) = shot(r, &camera, origin, 7)?;
    ensure!(
        lit == 0xffffff,
        "CSM depth incorrectly shadows an isolated receiver: {lit:x}"
    );
    for z in 14..17 {
        for y in 114..117 {
            for x in 32..35 {
                put(&mut blocks, origin, IVec3::new(x, y, z), 1);
            }
        }
    }
    r.upload(&blocks)?;
    let (blocked, _) = shot(r, &camera, origin, 7)?;
    ensure!(
        blocked < 0x404040,
        "Map-only voxel shadow lost off-screen blocker: {blocked:x}"
    );
    let builds = r.far_depth_updates;
    r.render(&camera, origin, 7)?;
    ensure!(
        builds == r.far_depth_updates,
        "Static shadow depths rebuilt unnecessarily"
    );
    r.settings.shadow_ray_distance = 64.0;
    let (near, _) = shot(r, &camera, origin, 7)?;
    r.settings.far_shadows = false;
    let (reference, _) = shot(r, &camera, origin, 7)?;
    ensure!(
        near == reference && reference == 0,
        "Near contact shadows differ from exact reference"
    );
    r.settings.far_shadows = true;
    r.settings.shadow_ray_distance = 0.0;
    for z in 14..17 {
        for y in 114..117 {
            for x in 32..35 {
                put(&mut blocks, origin, IVec3::new(x, y, z), 0);
            }
        }
    }
    r.upload(&blocks)?;
    let (edited, _) = shot(r, &camera, origin, 7)?;
    ensure!(edited == lit, "Edited voxel shadow depth remained stale");
    let receiver = Vec3::new(0.5, camera.eye().y, -4.0);
    let az = 60.0f32.to_radians();
    let el = 20.0f32.to_radians();
    let sun = Vec3::new(az.sin() * el.cos(), el.sin(), az.cos() * el.cos());
    let target = receiver + sun * (1.0 / sun.z);
    let mut scene = MeshScene::empty();
    scene.add_card_fixture(target - Vec3::Y);
    r.set_mesh_scene(scene)?;
    for hardware in [false, true] {
        if hardware && !r.ray_query_supported() {
            continue;
        }
        r.settings.hardware = hardware;
        r.settings.alpha_test = true;
        let (cutout, h) = shot(r, &camera, origin, 7)?;
        ensure!(h.material == 1, "Off-screen CSM card entered primary ray");
        r.settings.alpha_test = false;
        let (solid, _) = shot(r, &camera, origin, 7)?;
        ensure!(
            cutout > solid,
            "Far mesh depth ignored alpha coverage ({cutout:x}/{solid:x}), hardware {hardware}"
        );
    }
    // A grazing floor must remain lit under PCF, including pixel-center plane correction.
    r.set_mesh_scene(MeshScene::empty())?;
    blocks.fill(0);
    let plane = (99 - MIN_Y) as usize;
    blocks[plane * GPU_EDGE * GPU_EDGE..(plane + 1) * GPU_EDGE * GPU_EDGE].fill(1);
    r.upload(&blocks)?;
    camera.pitch = -0.35;
    camera.yaw = 0.63;
    r.settings.sun_elevation = 12.0;
    let pixels = r.render(&camera, origin, 7)?.to_vec();
    ensure!(
        pixels
            .iter()
            .zip(r.visibility())
            .filter(|(_, h)| h.valid != 0)
            .all(|(v, _)| *v == 0xffffff),
        "Grazing receiver-plane CSM acne"
    );
    r.settings = saved;
    r.animation_time = animation;
    r.resize_targets(extent.0, extent.1)?;
    println!("Far shadows passed: map-only voxel/off-screen blockers, cache reuse, edit invalidation, exact near contacts, alpha-tested polygon depths on supported backends, grazing receiver-plane PCF");
    Ok(())
}

/// New stochastic paths are tested separately from deterministic legacy parity.
pub fn soft_sky(r: &mut Renderer) -> Result<()> {
    use crate::meshes::MeshScene;
    let saved = r.settings;
    let extent = (r.width, r.height);
    let reference = r.reference_validation;
    let animation = r.animation_time;
    let water = r.water_time;
    r.reference_validation = false;
    r.settings = RenderSettings::default();
    r.settings.atmosphere = false;
    r.settings.animate = false;
    r.settings.meshes = false;
    r.settings.ssao = false;
    r.settings.disk_radius = 2.0;
    r.settings.shadow_type = 0;
    r.animation_time = Some(0.0);
    r.water_time = Some(0.0);
    r.resize_targets(161, 121)?;
    let edge = 5 * EDGE;
    let origin = IVec3::new(-32, MIN_Y, -32);
    let mut blocks = vec![0u8; edge * edge * HEIGHT];
    let plane = (99 - MIN_Y) as usize;
    blocks[plane * edge * edge..(plane + 1) * edge * edge].fill(1);
    for y in 103..107 {
        for z in -2..3 {
            for x in -2..3 {
                let q = IVec3::new(x, y, z) - origin;
                blocks[q.x as usize + edge * (q.z as usize + edge * q.y as usize)] = 1;
            }
        }
    }
    r.upload(&blocks)?;
    r.set_mesh_scene(MeshScene::empty())?;
    let mut p = Player::new(108.0);
    p.feet = Vec3::new(-8.0, 108.0, 12.0);
    let target = Vec3::new(-2.0, 100.0, -2.0);
    let dir = (target - p.eye()).normalize();
    p.yaw = dir.x.atan2(-dir.z);
    p.pitch = dir.y.asin();
    let mut edges = 0usize;
    for _ in 0..12 {
        r.render(&p, origin, 7)?;
        ensure!(
            r.shadow_samples()
                .iter()
                .all(|s| s.iter().all(|v| v.is_finite()) && (0.0..=1.0).contains(&s[2])),
            "Invalid denoised shadow"
        );
        edges = r.shadow_samples().iter().filter(|s| s[1] > 0.5).count();
    }
    ensure!(edges > 0, "Disk-shadow fixture has no penumbra");
    let accumulated = r
        .shadow_history_data()
        .as_chunks::<2>()
        .0
        .iter()
        .any(|h| h[1][2] >= 4.0);
    ensure!(accumulated, "Penumbra history did not accumulate");
    let partial = r
        .shadow_samples()
        .iter()
        .filter(|s| s[1] > 0.5 && s[2] > 0.01 && s[2] < 0.99)
        .count();
    ensure!(partial > 0, "Disk light did not produce partial shadows");
    // A mode change must reset accumulation; CSM mode remains query-free in shading.
    for mode in [1, 2, 0] {
        r.settings.shadow_type = mode;
        r.render(&p, origin, 7)?;
        let history = r.shadow_history_data();
        ensure!(
            history
                .as_chunks::<2>()
                .0
                .iter()
                .filter(|h| h[1][3].to_bits() == r.shadow_frame())
                .all(|h| h[1][2] == 1.0),
            "Mode change reused incompatible temporal history"
        );
    }
    // Nearby CSM softness is determined by caster/receiver separation, never
    // the camera's far distance. Empty center texels used to produce detached
    // ring copies whose radius changed when this setting changed.
    r.settings.shadow_type = 2;
    r.settings.disk_radius = 0.266;
    r.settings.view_chunks = 8;
    r.render(&p, origin, 7)?;
    let nearby = r.shadow_samples().to_vec();
    r.settings.view_chunks = 24;
    r.render(&p, origin, 7)?;
    let mut max_error = 0.0f32;
    let mut shadowed = 0usize;
    for ((a, b), h) in nearby.iter().zip(r.shadow_samples()).zip(r.visibility()) {
        if h.valid != 0 {
            max_error = max_error.max((a[0] - b[0]).abs());
            shadowed += usize::from(b[0] < 0.99);
        }
    }
    ensure!(shadowed > 0, "CSM ringing fixture has no shadow");
    ensure!(max_error < 0.0001, "CSM penumbra depends on view distance: {max_error}");
    r.settings.view_chunks = 8;
    r.settings.disk_radius = 2.0;
    r.settings.shadow_type = 0;
    // Exercise matching disk samples across overlapping voxel/polygon penumbrae.
    let mut cards = MeshScene::empty();
    for x in [-4.0, -2.0, 0.0] {
        cards.add_card_fixture(Vec3::new(x, 101.0, -3.0));
    }
    r.set_mesh_scene(cards)?;
    r.settings.meshes = true;
    for hardware in [false, true] {
        if hardware && !r.ray_query_supported() {
            continue;
        }
        r.settings.hardware = hardware;
        for _ in 0..4 {
            r.render(&p, origin, 7)?;
        }
        ensure!(
            r.shadow_samples()
                .iter()
                .all(|s| s.iter().all(|v| v.is_finite()) && (0.0..=1.0).contains(&s[0])),
            "Overlapping disk shadow visibility is invalid"
        );
    }
    // Routine animated map refreshes must not discard unrelated receiver history.
    r.settings.animate = true;
    let depth_builds = r.far_depth_updates[1];
    let snapshot_time = r.snapshot_time;
    for frame in 1..7 {
        r.animation_time = Some(frame as f32 * 0.3);
        r.snapshot_time = frame as f32 * 0.3;
        r.render(&p, origin, 7)?;
        ensure!(r.shadow_history_data().as_chunks::<2>().0.iter()
            .any(|h| h[1][3].to_bits()==r.shadow_frame() && h[1][2]>=4.0),
            "Animated CSM refresh discarded penumbra history");
    }
    ensure!(r.far_depth_updates[1]>depth_builds, "Animated refresh regression did not rebuild depths");
    r.snapshot_time = snapshot_time;
    r.settings.animate = false;
    // Resolution changes exercise allocation, descriptor rebinding and indexing.
    r.settings.shadow_type = 2;
    for resolution in [128,256,1024,2048,512] {
        r.settings.csm_resolution = resolution;
        r.render(&p, origin, 7)?;
        ensure!(r.shadow_samples().iter().all(|v| v[0].is_finite() && (0.0..=1.0).contains(&v[0])), "Invalid configurable CSM output");
    }
    r.settings.shadow_type = 0;
    p.feet.x += 10.0;
    r.render(&p, origin, 7)?;
    ensure!(
        r.shadow_samples()
            .iter()
            .all(|s| s.iter().all(|v| v.is_finite())),
        "Camera motion corrupted shadow history"
    );
    // Empty sky isolates the atmosphere and makes readback checks independent of terrain.
    p = Player::new(MAX_Y as f32 + 10.0);
    p.pitch = std::f32::consts::FRAC_PI_2;
    r.settings.atmosphere = true;
    r.settings.soft_shadows = false;
    r.render(&p, origin, 0)?;
    let bank = r.sky_cache.active * crate::sky::BANK_ENTRIES;
    let sky = r.sky_data();
    ensure!(
        sky[bank..bank + 524288]
            .iter()
            .all(|p| p.iter().all(|v| v.is_finite()) && p[..3].iter().all(|v| *v >= 0.0)),
        "Invalid Rayleigh/Mie sky radiance"
    );
    // RGB horizontal sky irradiance plus compensated direct must match the
    // incoming disk budget (outside the small twilight guard at the horizon).
    let lighting = bank + 557184 + 1280;
    ensure!(sky[lighting..lighting+10].iter().all(|v| v.iter().all(|x| x.is_finite())), "Invalid sky SH coefficients");
    let height = if r.sky_cache.build_sun.y < -0.02 {-r.sky_cache.build_sun.y} else {r.sky_cache.build_sun.y.max(0.0)};
    let budget = if r.sky_cache.build_sun.y < -0.02 {0.035} else {22.0};
    for (channel, _) in sky[lighting][..3].iter().enumerate() {
        let diffuse = std::f32::consts::PI*(sky[lighting][channel]*0.2820948
            +sky[lighting+2][channel]*(0.48860252*2.0/3.0)
            +sky[lighting+6][channel]*(0.63078314*0.25));
        let direct = sky[lighting+9][channel];
        ensure!(direct>=0.0 && (diffuse+direct*height-budget*height).abs()<0.002, "Sky/direct energy budget mismatch");
    }
    // Compare the cached convolution against direct sky quadrature for the
    // upward receiver. SH truncation should not materially change its energy.
    for (channel, _) in sky[lighting][..3].iter().enumerate() {
        let raw_horizontal: f32 = (0..128).map(|row| sky[bank+557184+row*10+9][channel]).sum();
        let raw_constant: f32 = (0..128).map(|row| sky[bank+557184+row*10][channel]).sum();
        let scale = sky[lighting][channel]/raw_constant.max(1e-9);
        let convolved = std::f32::consts::PI*(sky[lighting][channel]*0.2820948
            +sky[lighting+2][channel]*(0.48860252*2.0/3.0)
            +sky[lighting+6][channel]*(0.63078314*0.25));
        ensure!((convolved-raw_horizontal*scale).abs()<0.15*(raw_horizontal*scale).max(0.001), "Sky SH convolution differs from quadrature");
    }
    let importance = bank + 524288;
    let mut integral = 0.0f64;
    for row in 0..128 {
        let mut last = 0.0;
        for x in 0..256 {
            let v = sky[importance + row * 256 + x];
            ensure!(
                v[2] >= last && v[2] <= 1.00001,
                "Non-monotonic conditional sky CDF"
            );
            last = v[2];
            integral += v[1] as f64 * v[3] as f64;
        }
        ensure!((last - 1.0).abs() < 0.0001, "Sky row CDF is not normalized");
    }
    ensure!(
        (integral - 1.0).abs() < 0.001,
        "Sky solid-angle PDF integral {integral}"
    );
    ensure!(
        (sky[importance + 32768 + 127][1] - 1.0).abs() < 0.0001,
        "Marginal sky CDF is not normalized"
    );
    r.settings.day_cycle = true;
    r.settings.day_seconds = 120.0;
    r.water_time = Some(60.0);
    let builds = r.sky_cache.rebuilds;
    r.render(&p, origin, 0)?;
    ensure!(
        r.sky_cache.rebuilds > builds,
        "Day/night atmosphere cache did not rebuild"
    );
    ensure!(
        r.sky_data()
            .iter()
            .skip(r.sky_cache.active * crate::sky::BANK_ENTRIES)
            .take(524288)
            .all(|v| v.iter().all(|x| x.is_finite())),
        "Night sky is not finite"
    );
    r.settings = saved;
    r.animation_time = animation;
    r.water_time = water;
    r.reference_validation = reference;
    r.resize_targets(extent.0, extent.1)?;
    println!("Soft shadows / sky passed: {edges} classified edge pixels, {partial} partial shadows, temporal accumulation/reset, animated-refresh continuity, configurable CSM, motion rejection, all modes, finite atmosphere, normalized solid-angle PDF and conditional/marginal CDFs, day/night cache updates");
    Ok(())
}

/// The same stripe boundary must be sharp up close and bilinear when minified.
pub fn texture_filtering(r:&mut Renderer)->Result<()> {
    use image::{ImageEncoder,RgbaImage,Rgba};use std::io::Write;
    let path=std::env::temp_dir().join(format!("blockworld-minmag-{}.zip",std::process::id()));
    let mut image=RgbaImage::new(16,16);
    for y in 0..16 {for x in 0..16 {image.put_pixel(x,y,Rgba(if x%2==0 {[255,0,0,255]}else{[0,0,255,255]}));}}
    let mut bytes=Vec::new();image::codecs::png::PngEncoder::new(&mut bytes).write_image(image.as_raw(),16,16,image::ExtendedColorType::Rgba8)?;
    let mut zip=zip::ZipWriter::new(std::fs::File::create(&path)?);
    zip.start_file("assets/minecraft/textures/block/stone.png",zip::write::SimpleFileOptions::default())?;zip.write_all(&bytes)?;zip.finish()?;
    let saved=r.settings;let extent=(r.width,r.height);r.settings=RenderSettings::default();
    r.settings.shadows=false;r.settings.ssao=false;r.settings.meshes=false;
    r.load_resource_pack(&path)?;
    let edge=5*EDGE;let origin=IVec3::new(-32,MIN_Y,-32);let mut blocks=vec![0u8;edge*edge*HEIGHT];
    let q=IVec3::new(0,101,-3)-origin;blocks[q.x as usize+edge*(q.z as usize+edge*q.y as usize)]=1;
    r.upload(&blocks)?;r.set_mesh_scene(crate::meshes::MeshScene::empty())?;
    let mut camera=Player::new(100.0);camera.feet.x=0.25;camera.yaw=0.0;camera.pitch=0.0;
    for (size,minified) in [(129,false),(17,true),(129,false)] {
        r.resize_targets(size,size)?;let (color,hit)=shot(r,&camera,origin,3)?;
        ensure!(hit.material==1,"Filtering fixture missed its block");
        if minified {ensure!(((color>>16)&255).abs_diff(128)<=1 && (color&255).abs_diff(128)<=1 && ((color>>8)&255)==0,"Minification must be linear purple, got {color:06x}");}
        else {ensure!(color==0xff0000,"Magnification must retain the nearest red texel, got {color:06x}");}
    }
    r.settings.texture_linear=false;r.resize_targets(17,17)?;
    ensure!(shot(r,&camera,origin,3)?.0==0xff0000,"Nearest minification override failed");
    r.reset_resource_pack()?;r.settings=saved;r.resize_targets(extent.0,extent.1)?;std::fs::remove_file(path)?;
    println!("Texture min/mag passed: nearest red when magnified, linear purple when minified, resize transitions, nearest override");Ok(())
}

/// Elevated caster exposes wide disk penumbrae and conservative-map misses.
pub fn shadow_quality(r:&mut Renderer)->Result<()> {
    let saved=r.settings;let extent=(r.width,r.height);let animation=r.animation_time;let reference=r.reference_validation;
    r.reference_validation=false;
    r.resize_targets(161,121)?;r.settings=RenderSettings::default();
    r.settings.meshes=false;r.settings.ssao=false;r.settings.atmosphere=false;
    r.settings.animate=false;r.settings.sun_azimuth=0.0;r.settings.sun_elevation=55.0;
    r.settings.csm_resolution=1024;r.settings.shadow_type=2;r.settings.penumbra_samples=4;
    r.animation_time=Some(0.0);
    let edge=9*EDGE;let origin=IVec3::new(-64,MIN_Y,-64);
    let mut blocks=vec![0u8;edge*edge*HEIGHT];
    let plane=(99-MIN_Y) as usize;
    blocks[plane*edge*edge..(plane+1)*edge*edge].fill(1);
    for y in 120..126 {for z in -3..3 {for x in -3..3 {
        let q=IVec3::new(x,y,z)-origin;blocks[q.x as usize+edge*(q.z as usize+edge*q.y as usize)]=1;
    }}}
    r.upload(&blocks)?;r.set_mesh_scene(crate::meshes::MeshScene::empty())?;
    let mut camera=Player::new(112.38);camera.feet=Vec3::new(0.0,112.38,-16.0);
    camera.yaw=0.0;camera.pitch= -std::f32::consts::FRAC_PI_2;
    let mut widths=Vec::new();
    for radius in [0.45,1.0,2.0] {
        r.settings.disk_radius=radius;
        for _ in 0..20 {r.render(&camera,origin,7)?;}
        let partial=r.shadow_samples().iter().zip(r.visibility()).filter(|(s,h)|h.valid!=0 && h.normal_depth[1]>0.9 && s[2]>0.02 && s[2]<0.98).count();
        widths.push(partial);
    }
    r.settings.shadow_type=1;r.settings.shadow_ray_distance=48.0;
    for _ in 0..32 {r.render(&camera,origin,7)?;}
    let elevation=55.0f64.to_radians();let light=glam::DVec3::new(0.0,elevation.sin(),elevation.cos());
    let right=light.cross(glam::DVec3::Y).normalize();let up=right.cross(light);
    let radius=2.0f64.to_radians().tan();let mut penumbra=0;let mut unclassified=0;let mut hard=0;
    for (i,(sample,hit)) in r.shadow_samples().iter().zip(r.visibility()).enumerate() {
        if hit.valid==0 || hit.normal_depth[1]<0.9 {continue;}
        let x=(i%r.width as usize) as f32;let y=(i/r.width as usize) as f32;
        let xy=glam::Vec2::new((x+0.5)/r.width as f32*2.0-1.0,(y+0.5)/r.height as f32*2.0-1.0);
        let f=camera.forward();let camera_right=camera.right();let camera_up=camera_right.cross(f);
        let ray=(f+camera_right*xy.x*(r.width as f32/r.height as f32)*(FOV_Y_DEGREES.to_radians()*0.5).tan()-camera_up*xy.y*(FOV_Y_DEGREES.to_radians()*0.5).tan()).normalize();
        let point=(camera.eye()+ray*hit.normal_depth[3]+Vec3::Y*0.003).as_dvec3();
        let mut blocked=0;
        for n in 0..128 {
            let a=n as f64*2.399963229728653;let radial=radius*((n as f64+0.5)/128.0).sqrt();
            let direction=(light+radial*(right*a.cos()+up*a.sin())).normalize();
            let low=glam::DVec3::new(-3.0,120.0,-3.0);let high=glam::DVec3::new(3.0,126.0,3.0);
            let mut near=0.0f64;let mut far=1000.0f64;
            for axis in 0..3 {let a=(low[axis]-point[axis])/direction[axis];let b=(high[axis]-point[axis])/direction[axis];near=near.max(a.min(b));far=far.min(a.max(b));}
            blocked+=usize::from(far>near);
        }
        let visibility=1.0-blocked as f32/128.0;
        if visibility>0.15 && visibility<0.85 {
            penumbra+=1;unclassified+=usize::from(sample[1]<0.5);hard+=usize::from(sample[2]<=0.02 || sample[2]>=0.98);
        }
    }
    r.settings=saved;r.animation_time=animation;r.reference_validation=reference;r.resize_targets(extent.0,extent.1)?;
    println!("Shadow quality: CSM partial areas {:?}; physical penumbra {}, unclassified {}, hard spots {}",widths,penumbra,unclassified,hard);
    ensure!(widths[2]>widths[0]*3/2 && widths[1]>widths[0],"CSM softness plateau above .45 degrees: {widths:?}");
    ensure!(penumbra>20 && unclassified==0 && hard<=penumbra/20,"Hybrid map classification leaves hard penumbra spots: {unclassified}/{penumbra} unclassified, {hard} hard");
    Ok(())
}

/// A synthetic, redistributable ZIP fixture tests actual PNG/ZIP/atlas/GPU plumbing.
pub fn resource_pack(r:&mut Renderer)->Result<()> {
    use image::{ImageEncoder,RgbaImage,Rgba};use std::io::Write;
    let path=std::env::temp_dir().join(format!("blockworld-pack-gpu-{}.zip",std::process::id()));
    let mut zip=zip::ZipWriter::new(std::fs::File::create(&path)?);
    for (name,width,height,kind) in [("stone",16,32,0),("oak_leaves",16,16,1),("short_grass",16,16,3),("water_still",16,32,0),("grass_block_side",16,16,2)] {
        let mut im=RgbaImage::new(width,height);
        for y in 0..height {for x in 0..width {
            let color=if kind==3 {[0,255,0,if (4..12).contains(&x){0}else{255}]}else if kind==1 {[0,255,0,if x<8{0}else{255}]} else if kind==2 {if y<8 {[0,255,0,255]}else{[255,0,0,255]}} else if y<16 {[255,0,0,255]} else {[0,0,255,255]};
            im.put_pixel(x,y,Rgba(color));
        }}
        let mut bytes=Vec::new();image::codecs::png::PngEncoder::new(&mut bytes).write_image(im.as_raw(),width,height,image::ExtendedColorType::Rgba8)?;
        let filename=format!("assets/minecraft/textures/block/{name}.png");
        zip.start_file(&filename,zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated))?;zip.write_all(&bytes)?;
        if height>width {zip.start_file(format!("{filename}.mcmeta"),zip::write::SimpleFileOptions::default())?;zip.write_all(br#"{"animation":{"frametime":2,"interpolate":true}}"#)?;}
    }
    zip.finish()?;
    let saved=r.settings;let extent=(r.width,r.height);let water=r.water_time;
    r.settings=RenderSettings::default();r.settings.meshes=false;r.settings.shadows=false;r.settings.ssao=false;
    r.load_resource_pack(&path)?;r.resize_targets(1,1)?;
    let edge=5*EDGE;let origin=IVec3::new(-32,MIN_Y,-32);let mut blocks=vec![0u8;edge*edge*HEIGHT];
    let q=IVec3::new(0,101,-5)-origin;blocks[q.x as usize+edge*(q.z as usize+edge*q.y as usize)]=1;
    r.upload(&blocks)?;r.set_mesh_scene(crate::meshes::MeshScene::empty())?;
    let mut p=Player::new(100.0);p.feet.x=0.25;p.yaw=0.0;p.pitch=0.0;
    r.water_time=Some(0.0);let (red,hit)=shot(r,&p,origin,3)?;
    ensure!(hit.material==1 && red==0xff0000,"Atlas frame zero is not red: {red:x}");
    r.water_time=Some(0.1);let (blue,_)=shot(r,&p,origin,3)?;
    ensure!(blue==0x0000ff,"Animated texture did not advance: {blue:x}");
    r.water_time=Some(0.05);let (blend,_)=shot(r,&p,origin,3)?;
    ensure!(((blend>>16)&255).abs_diff(127)<=1 && (blend&255).abs_diff(127)<=1 && ((blend>>8)&255)==0,"Interpolated texture is not purple: {blend:x}");
    r.water_time=Some(0.0);r.settings.texture_linear=true;
    for x in [0.00001,0.99999] {p.feet.x=x;let (color,_)=shot(r,&p,origin,3)?;ensure!(color==0xff0000,"Atlas edge bleeds: {color:x}");}
    r.settings.texture_linear=false;
    let face=IVec3::new(0,101,-3)-origin;let face_index=face.x as usize+edge*(face.z as usize+edge*face.y as usize);
    blocks[face_index]=2;r.upload(&blocks)?;p.feet.x=0.25;
    p.feet.y=101.75-1.62;let (top,_)=shot(r,&p,origin,3)?;
    p.feet.y=101.25-1.62;let (bottom,_)=shot(r,&p,origin,3)?;
    ensure!(top==0x00ff00 && bottom==0xff0000,"Imported voxel side V orientation is inverted: {top:x}/{bottom:x}");
    p.feet.y=100.0;
    let q=IVec3::new(0,101,-3)-origin;blocks[q.x as usize+edge*(q.z as usize+edge*q.y as usize)]=13;r.upload(&blocks)?;
    p.feet.x=0.25;let (_,hole)=shot(r,&p,origin,3)?;p.feet.x=0.75;let (_,leaf)=shot(r,&p,origin,3)?;
    ensure!(hole.material==1 && leaf.material==13 && leaf.valid&2!=0,"Voxel pack alpha holes lost: {} / {}",hole.material,leaf.material);
    // The imported card's alpha must agree between software and ray-query candidates.
    r.settings.meshes=true;let mut scene=crate::meshes::MeshScene::empty();scene.add_card_fixture(glam::Vec3::new(0.5,100.0,-2.0));r.set_mesh_scene(scene)?;
    r.resize_targets(65,49)?;
    // Center-hole grass coverage remains visible with either card orientation.
    // Avoid an exactly coplanar crossed-card/texel-boundary ray: either adjacent
    // texel is legal under the two intersection backends' floating-point error.
    p.feet.x=0.57;
    let mut reference_mask:Option<Vec<bool>>=None;
    for hardware in [false,true] {
        if hardware && !r.ray_query_supported(){continue;}r.settings.hardware=hardware;
        r.settings.alpha_test=true;r.render(&p,origin,0)?;let masked=r.visibility().iter().filter(|h|h.material==9).count();
        let mask:Vec<_>=r.visibility().iter().map(|h|h.material==9).collect();
        if let Some(reference)=&reference_mask {let differences=reference.iter().zip(&mask).filter(|(a,b)|a!=b).count();ensure!(differences==0,"Pack alpha coverage differs at {differences} pixels between ray-query and software backends");}else{reference_mask=Some(mask);}
        r.settings.alpha_test=false;r.render(&p,origin,0)?;let solid=r.visibility().iter().filter(|h|h.material==9).count();
        ensure!(masked>0 && solid>masked,"Pack alpha card coverage is invalid on backend {hardware}: {masked}/{solid}");
    }
    // Optical water must ignore the imported red/blue animated water albedo.
    r.settings.meshes=false;r.settings.waves=false;blocks.fill(0);
    for z in -5..5 {for x in -5..5 {for y in 100..105 {
        let q=IVec3::new(x,y,z)-origin;
        blocks[q.x as usize+edge*(q.z as usize+edge*q.y as usize)]=if y==100 {12} else {5};
    }}}
    r.upload(&blocks)?;p.feet=Vec3::new(0.5,107.0,0.5);p.yaw=0.0;p.pitch= -1.2;
    r.water_time=Some(0.0);let red_water=r.render(&p,origin,0)?.to_vec();
    r.water_time=Some(0.1);let blue_water=r.render(&p,origin,0)?.to_vec();
    ensure!(red_water==blue_water,"Optical water still samples resource-pack water albedo");
    r.reset_resource_pack()?;r.settings=saved;r.water_time=water;r.resize_targets(extent.0,extent.1)?;std::fs::remove_file(path)?;
    println!("Resource packs passed: Deflate ZIP/PNG, animated/interpolated GPU frames, padded linear borders, voxel alpha holes, both polygon backends, generated fallback restoration");Ok(())
}

/// Exercise the final GPU display transform independently of terrain/lighting.
pub fn tone_mapping(r: &mut Renderer) -> Result<()> {
    let saved=r.settings;let reference=r.reference_validation;let extent=(r.width,r.height);
    r.settings=RenderSettings::default();r.reference_validation=false;
    r.settings.atmosphere=false;r.settings.meshes=false;r.settings.soft_shadows=false;
    r.settings.ssao=false;r.settings.shadows=false;
    r.settings.saturation=1.0; // Isolate curve tests from the gameplay default.
    r.upload(&vec![0;GPU_VOXELS])?;r.resize_targets(1,1)?;
    let mut camera=Player::new(MAX_Y as f32+10.0);camera.pitch=std::f32::consts::FRAC_PI_2;
    let origin=IVec3::new(0,MIN_Y,0);
    let curve=|x:f32| ((x*(0.15*x+0.05)+0.004)/(x*(0.15*x+0.5)+0.06))-0.02/0.3;
    let mut last=[0u32;3];
    for ev in [-4.0f32,0.0,4.0] {
        r.settings.exposure_ev=ev;
        let pixel=r.render(&camera,origin,0)?[0];
        for (channel,input) in [0.26f32,0.51,0.77].iter().enumerate() {
            let mapped=(curve(input*ev.exp2()*2.0)/curve(11.2)).clamp(0.0,1.0);
            let display=if mapped<=0.0031308 {mapped*12.92} else {1.055*mapped.powf(1.0/2.4)-0.055};
            let actual=255-((pixel>>(16-channel*8))&255); // center crosshair inverts display color
            ensure!((actual as f32-display*255.0).abs()<1.5,"GPU filmic/exposure/transfer mismatch");
            ensure!(actual>=last[channel],"Exposure is not monotonic");last[channel]=actual;
        }
    }
    r.settings.exposure_ev=0.0;
    let curve_rgb=[0.26f32,0.51,0.77].map(|x| curve(x*2.0)/curve(11.2));
    let luminance=curve_rgb[0]*0.2126+curve_rgb[1]*0.7152+curve_rgb[2]*0.0722;
    for saturation in [0.0,1.0,1.15,2.0] {
        r.settings.saturation=saturation;let pixel=r.render(&camera,origin,0)?[0];
        for (channel,value) in curve_rgb.iter().enumerate() {
            let x=(luminance+(*value-luminance)*saturation).clamp(0.0,1.0);
            let display=if x<=0.0031308 {x*12.92} else {1.055*x.powf(1.0/2.4)-0.055};
            let actual=255-((pixel>>(16-channel*8))&255);
            ensure!((actual as f32-display*255.0).abs()<1.5,"Hable saturation mismatch");
        }
    }
    r.settings.saturation=1.0;
    let debug=r.render(&camera,origin,2)?[0];r.settings.exposure_ev = -8.0;
    ensure!(r.render(&camera,origin,2)?[0]==debug,"Tone mapping changed a diagnostic view");
    r.settings.tone_mapping=false;
    let pixel=r.render(&camera,origin,0)?[0];
    ensure!((255-((pixel>>16)&255)).abs_diff((0.26*255.0) as u32)<=1,"Tone-map bypass changed raw color");
    r.settings=saved;r.reference_validation=reference;r.resize_targets(extent.0,extent.1)?;
    println!("Tone mapping passed: Uncharted curve, sRGB transfer, -4/0/+4 EV, diagnostic bypass and raw bypass");
    Ok(())
}

/// Ground bounce must illuminate a downward-facing surface and respond to albedo.
pub fn ground_bounce(r: &mut Renderer) -> Result<()> {
    let saved=r.settings;let reference=r.reference_validation;let extent=(r.width,r.height);
    r.settings=RenderSettings::default();r.reference_validation=false;
    r.settings.meshes=false;r.settings.soft_shadows=false;r.settings.ssao=false;
    r.settings.shadows=false;r.settings.tone_mapping=false;
    r.resize_targets(1,1)?;
    let origin=IVec3::new(-(GPU_EDGE as i32)/2,MIN_Y,-(GPU_EDGE as i32)/2);
    let mut blocks=vec![0u8;GPU_VOXELS];
    let mut camera=Player::new(100.0);camera.feet.x=0.5;camera.feet.z=0.5;
    camera.pitch=std::f32::consts::FRAC_PI_2;
    put(&mut blocks,origin,IVec3::new(0,104,0),6);
    let floor=(99-MIN_Y) as usize*GPU_EDGE*GPU_EDGE;
    let mut brightness=Vec::new();
    for material in [1u8,12u8] {
        blocks[floor..floor+GPU_EDGE*GPU_EDGE].fill(material);r.upload(&blocks)?;
        r.settings.ground_bounce=false;let off=r.render(&camera,origin,0)?[0];
        r.settings.ground_bounce=true;let on=r.render(&camera,origin,0)?[0];
        ensure!(r.visibility()[0].normal_depth[1]< -0.99,"Bounce receiver is not downward facing");
        let light=|pixel:u32| (255-((pixel>>16)&255))+(255-((pixel>>8)&255))+(255-(pixel&255));
        ensure!(light(on)>light(off)+3,"Ground bounce did not brighten receiver");
        let bank=r.sky_cache.active*crate::sky::BANK_ENTRIES;
        let rho=r.sky_data()[bank+558602];let radiance=r.sky_data()[bank+558603];
        ensure!(rho[3]>0.0 && rho[..3].iter().all(|v| (0.0..=0.90001).contains(v)),"Invalid average ground reflectance {rho:?}");
        ensure!(radiance[..3].iter().all(|v| v.is_finite() && *v>0.0),"Invalid bounced ground radiance");
        brightness.push(radiance[0]+radiance[1]+radiance[2]);
    }
    ensure!(brightness[1]>brightness[0],"Snow did not reflect more energy than stone");
    blocks[floor..floor+GPU_EDGE*GPU_EDGE].fill(0);r.upload(&blocks)?;r.render(&camera,origin,0)?;
    let bank=r.sky_cache.active*crate::sky::BANK_ENTRIES;
    ensure!(r.sky_data()[bank+558603][..3].iter().all(|v| *v==0.0),"Missing ground still emits bounce light");
    r.settings=saved;r.reference_validation=reference;r.resize_targets(extent.0,extent.1)?;
    println!("Ground bounce passed: downward receiver, toggle, stone/snow energy response, missing-ground reset");
    Ok(())
}

pub fn entity_visibility(r:&mut Renderer)->Result<()> {
    use crate::entities::{ActivePlayer,EntitySystem};
    let saved=r.settings;let extent=(r.width,r.height);
    r.settings=RenderSettings::default();r.settings.ssao=false;r.settings.shadows=false;
    r.settings.animate=false;r.settings.foliage=false;r.settings.frustum=false;
    r.resize_targets(161,121)?;r.upload(&vec![0;GPU_VOXELS])?;
    let origin=IVec3::new(-(GPU_EDGE as i32)/2,MIN_Y,-(GPU_EDGE as i32)/2);
    let camera=Player::new(100.0);
    for hardware in [false,true] {
        if hardware && !r.ray_query_supported() {continue;}
        r.settings.hardware=hardware;
        let mut scene=crate::meshes::MeshScene::empty();scene.add_fish(Vec3::new(0.0,101.0,-5.0),0.0);
        let mut entities=EntitySystem::from_scene(&scene);
        entities.update(&mut scene,&[ActivePlayer{id:0,position:camera.feet}],0.0,0.0);
        r.set_mesh_scene(scene.clone())?;r.render(&camera,origin,9)?;
        ensure!(r.mesh_hits().iter().any(|h| h.flags!=0),"Active fish disappeared");
        entities.update(&mut scene,&[ActivePlayer{id:0,position:camera.feet+Vec3::X*100.0}],0.0,1.0);
        r.set_mesh_scene(scene)?;r.render(&camera,origin,9)?;
        ensure!(r.mesh_hits().iter().all(|h| h.flags==0),"Inactive fish still rendered (hardware={hardware})");
    }
    r.settings=saved;r.resize_targets(extent.0,extent.1)?;
    let players=r.active_players.clone();
    r.settings=RenderSettings::default();r.settings.animate=false;r.settings.ssao=false;r.settings.shadows=false;r.settings.frustum=false;
    r.resize_targets(321,241)?;
    for hardware in [false,true] {
        if hardware && !r.ray_query_supported() {continue;}
        r.settings.hardware=hardware;
        let mut scene=crate::meshes::MeshScene::empty();
        scene.add_fish(Vec3::new(0.0,101.0,-40.0),0.0);
        // Near fish keep the scene broad-phase box intersecting the center ray.
        scene.add_fish(Vec3::new(-5.0,101.0,-30.0),0.0);
        scene.add_fish(Vec3::new(5.0,101.0,-30.0),0.0);
        r.active_players=vec![ActivePlayer{id:1,position:Vec3::new(0.0,101.0,-40.0)}];
        r.set_entity_mesh_scene(scene)?;r.render(&camera,origin,9)?;
        ensure!(r.mesh_hits().iter().any(|h| h.flags!=0 && h.ids[0]==0),"Remote-player volume did not retain fish");
        r.active_players.retain(|p| p.id==0);r.render(&camera,origin,9)?;
        ensure!(r.mesh_hits().iter().all(|h| h.flags==0 || h.ids[0]!=0),"Old snapshot rendered an out-of-range fish");
    }
    r.active_players=players;r.settings=saved;r.resize_targets(extent.0,extent.1)?;
    println!("Entity visibility passed: 64m activation, software rejection and hardware instance masks");
    Ok(())
}

pub fn svo(r:&mut Renderer)->Result<()> {
    let saved=r.settings;let extent=(r.width,r.height);
    r.settings=RenderSettings::default();r.settings.meshes=false;r.settings.ssao=false;r.settings.shadows=false;
    r.resize_targets(161,97)?;
    // Empty residency must not inflate 1px just because a dense-world estimate
    // would exceed the budget at a fullscreen-sized framebuffer.
    let empty_world=crate::world::World::new(42);let probe=Player::new(100.0);
    r.lod.as_mut().unwrap().prepare(probe.eye(),64,1080,1.0,&empty_world)?;
    ensure!(r.lod.as_ref().unwrap().effective_pixels==1.0,"Synthetic memory estimate relaxed 1px target");
    r.settings.view_chunks=1;r.prepare_lods(&probe,&empty_world)?;r.render(&probe,IVec3::ZERO,0)?;r.settings.view_chunks=8;
    let origin=IVec3::new(-(GPU_EDGE as i32)/2,MIN_Y,-(GPU_EDGE as i32)/2);
    r.upload(&vec![0;GPU_VOXELS])?;r.set_mesh_scene(crate::meshes::MeshScene::empty())?;
    for (x,y) in [(160,100),(-160,-20)] {
        let block=IVec3::new(x,y,0);
        r.lod.as_mut().unwrap().fixture_svo((x.div_euclid(16),0),block,1,12);
        let mut camera=Player::new(y as f32-1.0);camera.feet.x=x as f32+0.5;camera.feet.z=40.0;camera.yaw=0.0;camera.pitch=0.0;
        r.settings.lod_pixels=0.0001;let (_,fine)=shot(r,&camera,origin,3)?;
        ensure!(fine.material==12 && (fine.normal_depth[3]-39.0).abs()<0.01,"Fine SVO hit lost: {x}/{y}");
        r.settings.lod_pixels=2.5;let (_,subpixel)=shot(r,&camera,origin,3)?;
        ensure!((subpixel.normal_depth[3]-39.0).abs()<0.01 && subpixel.valid&8!=0,"Subpixel base voxel did not use its average appearance");
        r.settings.lod_pixels=64.0;let (pixel,coarse)=shot(r,&camera,origin,3)?;
        ensure!(coarse.valid&8!=0 && (coarse.normal_depth[3]-24.0).abs()<0.01,"GPU projected-size stop did not select parent");
        let metadata=r.resource_pack.as_ref().unwrap().metadata(0.0,true);
        let average=metadata[(12-1)*72+4*12+11];
        for shift in [0,8,16] {ensure!(((pixel>>shift)&255).abs_diff((average>>shift)&255)<=1,"SVO aggregate did not use texture-average palette");}
        let world=crate::world::World::new(42);r.settings.view_chunks=1;r.prepare_lods(&camera,&world)?;r.render(&camera,origin,0)?;r.settings.view_chunks=8;
    }
    r.settings=saved;r.resize_targets(extent.0,extent.1)?;
    println!("SVO passed: signed-coordinate exact hits, GPU pixel-threshold parent selection, averaged appearance palette");Ok(())
}

/// Independent GI experiment: primary geometry is identical, history is bounded,
/// setting changes/edits reject stale lighting, and the original path is restored.
pub fn vxgi(r:&mut Renderer)->Result<()> {
    let saved=r.settings;let extent=(r.width,r.height);
    r.resize_targets(65,49)?;r.settings=RenderSettings::default();
    r.settings.meshes=false;r.settings.shadows=false;r.settings.atmosphere=false;
    r.settings.water=false;r.settings.animate=false;r.settings.ssao=false;
    let origin=IVec3::new(-(GPU_EDGE as i32)/2,MIN_Y,-(GPU_EDGE as i32)/2);
    let mut blocks=vec![0;GPU_VOXELS];
    for z in -20..10 {for x in -12..13 {put(&mut blocks,origin,IVec3::new(x,100,z),1);}}
    for y in 101..110 {for z in -20..10 {put(&mut blocks,origin,IVec3::new(-5,y,z),12);}}
    r.upload(&blocks)?;r.set_mesh_scene(crate::meshes::MeshScene::empty())?;
    let mut camera=Player::new(104.0);camera.yaw=0.0;camera.pitch= -0.7;
    let reference=r.render(&camera,origin,0)?.to_vec();let visibility=r.visibility().to_vec();
    r.settings.vxgi=true;r.settings.gi_range=32.0;
    for _ in 0..20 {r.render(&camera,origin,0)?;}
    let count=|r:&Renderer| {
        let pixels=r.width.div_ceil(2) as usize*r.height.div_ceil(2) as usize;
        let bank=(r.shadow_frame()&1) as usize;
        (0..pixels).map(|i|r.gi_history_data()[(bank*pixels+i)*2+1][3].to_bits()&255).max().unwrap()
    };
    ensure!(count(r)>8,"GI reprojection did not accumulate stationary surfaces");
    let gi=r.render(&camera,origin,0)?.to_vec();
    for (a,b) in visibility.iter().zip(r.visibility()) {
        ensure!(a.material==b.material && a.normal_depth==b.normal_depth,"GI changed primary visibility");
    }
    ensure!(gi!=reference,"GI path did not change lighting");
    for v in r.gi_history_data().as_chunks::<2>().0 {
        ensure!(v[1][..3].iter().all(|c|c.is_finite() && *c>=0.0 && *c<=32.0),"Invalid GI irradiance");
    }
    r.settings.gi_range=24.0;r.render(&camera,origin,0)?;
    ensure!(count(r)==1,"GI settings reused stale history");
    r.settings.gi_temporal=false;
    for _ in 0..3 {r.render(&camera,origin,0)?;ensure!(count(r)==1,"GI temporal toggle ignored");}
    r.settings.gi_temporal=true;r.upload(&blocks)?;r.render(&camera,origin,0)?;
    ensure!(count(r)==1,"GI edit/upload failed to reject stale history");
    camera.feet.x+=5.0;r.render(&camera,origin,0)?;
    ensure!(count(r)<=2,"GI disocclusion reused unrelated history");
    camera.feet.x-=5.0;r.settings.vxgi=false;
    ensure!(r.render(&camera,origin,0)?==reference.as_slice(),"Original ambient path changed after GI toggle");
    // GI pages include the normally excluded exact near ring, asynchronously.
    r.settings.vxgi=true;r.settings.view_chunks=1;
    let world=crate::world::World::new(42);let started=std::time::Instant::now();
    loop {
        r.prepare_lods(&camera,&world)?;r.render(&camera,origin,0)?;
        if r.lod.as_ref().unwrap().levels.iter().sum::<usize>()==9 {break;}
        ensure!(started.elapsed().as_secs()<10,"Near GI SVO pages did not stream");
    }
    r.settings.vxgi=false;r.prepare_lods(&camera,&world)?;r.render(&camera,origin,0)?;
    ensure!(r.lod.as_ref().unwrap().levels.iter().sum::<usize>()==0,"GI-only near pages were not evicted");
    r.settings.vxgi=true;let started=std::time::Instant::now();
    loop {
        r.prepare_lods(&camera,&world)?;r.render(&camera,origin,0)?;
        if r.lod.as_ref().unwrap().levels.iter().sum::<usize>()==9 {break;}
        ensure!(started.elapsed().as_secs()<10,"GI re-enable failed to reoffer near pages");
    }
    r.resize_targets(1,1)?;r.render(&camera,origin,0)?;
    r.settings=saved;r.resize_targets(extent.0,extent.1)?;
    println!("VXGI passed: changed indirect lighting, exact visibility, temporal accumulation/rejection, toggles, near SVO streaming/eviction, odd/1px resize");Ok(())
}

/// Real compute photon maps, underwater receivers, and negative lighting controls.
pub fn caustics(r: &mut Renderer) -> Result<()> {
    let saved=r.settings;let replay=r.water_time;let extent=(r.width,r.height);
    r.resize_targets(257,161)?;
    r.settings=RenderSettings{meshes:false,ssao:false,shadows:false,atmosphere:false,tone_mapping:false,animate:false,..RenderSettings::default()};
    r.water_time=Some(1.25);
    let origin=IVec3::new(-(GPU_EDGE as i32)/2,MIN_Y,-(GPU_EDGE as i32)/2);
    let mut blocks=vec![0;GPU_VOXELS];
    for z in -36..=36 {for x in -36..=36 {
        put(&mut blocks,origin,IVec3::new(x,98,z),4);
        for y in 99..=104 {put(&mut blocks,origin,IVec3::new(x,y,z),5);}
    }}
    r.upload(&blocks)?;
    r.set_mesh_scene(crate::meshes::MeshScene::empty())?;
    let mut camera=Player::new(103.0-1.62);camera.feet.z=4.5;camera.pitch= -1.25;
    let on=r.render(&camera,origin,0)?.to_vec();
    let map=r.caustic_data().to_vec();let map_ms=r.caustic_ms;
    let occupied=map[..65536].iter().filter(|&&v|v!=0).count();
    ensure!(occupied>10000,"Missing water-source coverage: {occupied}");
    for layer in 1..=4 {
        let plane=&map[layer*65536..(layer+1)*65536];
        let total: u64=plane.iter().map(|&v|v as u64).sum();
        let mean=total as f64/(occupied as f64*8192.0);
        ensure!((0.85..1.15).contains(&mean),"Photon energy not conserved at depth layer {layer}: {mean}");
        ensure!(plane.iter().any(|&v|v>8500),"No focusing at depth layer {layer}");
    }
    ensure!(map[65536..131072]!=map[131072..196608],"Depth slices are identical");
    r.settings.caustics=false;
    let off=r.render(&camera,origin,0)?.to_vec();
    let changed=on.iter().zip(&off).filter(|(a,b)|a!=b).count();
    ensure!(changed>1000,"Caustics did not affect submerged floor: {changed}");
    if let Some(path)=std::env::var_os("BLOCKWORLD_CAUSTIC_CAPTURE") {
        let path=std::path::PathBuf::from(path);
        for (pixels,name) in [(&on,path.clone()),(&off,path.with_extension("off.png"))] {
            image::RgbImage::from_fn(r.width,r.height,|x,y| {let c=pixels[(x+y*r.width) as usize];image::Rgb([(c>>16) as u8,(c>>8) as u8,c as u8])}).save(name)?;
        }
        image::RgbImage::from_fn(1024,256,|x,y| {let layer=x/256;let v=map[((layer+1)*65536+x%256+256*y) as usize] as f32/8192.0;let c=(v*100.0).clamp(0.0,255.0) as u8;image::Rgb([c,c,c])}).save(path.with_extension("maps.png"))?;
    }

    r.settings.caustics=true;
    ensure!(on==r.render(&camera,origin,0)?,"Caustic replay is not deterministic");
    ensure!(map==r.caustic_data(),"Fixed-time photon map is not deterministic");
    r.water_time=Some(2.25);
    r.render(&camera,origin,0)?;
    ensure!(map[65536..].iter().zip(&r.caustic_data()[65536..]).filter(|(a,b)|a!=b).count()>1000,"Wave animation did not animate photon maps");
    r.water_time=Some(1.25);
    r.settings.water_ior=1.0;r.render(&camera,origin,0)?;
    for layer in 1..=4 {for y in 100..156 {for x in 100..156 {
        ensure!(r.caustic_data()[layer*65536+x+256*y].abs_diff(8192)<=2,"Equal-IOR media produced spurious focusing");
    }}}
    r.settings.water_ior=RenderSettings::default().water_ior;
    // Every disabled optical input must bypass both map generation and lighting.
    for gate in 0..5 {
        match gate {0=>r.settings.ssr=false,1=>r.settings.waves=false,2=>r.settings.water=false,3=>r.settings.wave_strength=0.0,_=>r.settings.caustic_strength=0.0}
        let bypass=r.render(&camera,origin,0)?.to_vec();
        r.settings.caustics=false;
        ensure!(bypass==r.render(&camera,origin,0)?,"Disabled water gate {gate} leaked caustics");
        r.settings.caustics=true;r.settings.ssr=true;r.settings.waves=true;r.settings.water=true;
        r.settings.wave_strength=RenderSettings::default().wave_strength;r.settings.caustic_strength=1.0;
    }
    r.settings.sun_intensity=0.0;
    let ambient_only=r.render(&camera,origin,0)?.to_vec();r.settings.caustics=false;
    ensure!(ambient_only==r.render(&camera,origin,0)?,"Caustics leaked into ambient lighting");
    r.settings.sun_intensity=RenderSettings::default().sun_intensity;r.settings.caustics=true;
    // Outside local fade radius, ordinary direct lighting must be exactly restored.
    r.settings.caustic_distance=8.0;camera.feet.y=128.0;camera.pitch= -1.5;
    let far=r.render(&camera,origin,0)?.to_vec();r.settings.caustics=false;
    ensure!(far==r.render(&camera,origin,0)?,"Far-distance caustics did not fade to ordinary lighting");
    // Two-metre pool: measure each refinement independently at the same pose.
    for z in -36..=36 {for x in -36..=36 {for y in 101..=104 {put(&mut blocks,origin,IVec3::new(x,y,z),0);}}}
    r.upload(&blocks)?;r.resize_targets(641,401)?;
    camera.feet.y=100.3-1.62;camera.feet.z=4.5;camera.pitch= -1.0;
    r.settings.caustic_distance=64.0;r.settings.caustics=true;
    let mut previous=Vec::new();
    for (name,near,adaptive,shallow) in [("legacy",false,false,false),("near",true,false,false),("adaptive",true,true,false),("shallow",true,true,true)] {
        r.settings.caustic_near=near;r.settings.caustic_adaptive=adaptive;r.settings.caustic_shallow=shallow;
        let pixels=r.render(&camera,origin,0)?.to_vec();
        if !previous.is_empty() {ensure!(pixels.iter().zip(&previous).filter(|(a,b)|a!=b).count()>500,"Caustic refinement {name} did not affect shallow receivers");}
        previous=pixels.clone();
        if near && shallow {
            let data=r.caustic_data();let source=data[5*65536..6*65536].iter().filter(|&&v|v!=0).count();
            ensure!(source==65536,"Fine source map lost pool coverage");
            for layer in 1..=5 {
                let energy=data[(5+layer)*65536..(6+layer)*65536].iter().map(|&v|v as f64/8192.0).sum::<f64>()/source as f64;
                ensure!((0.85..1.15).contains(&energy),"Adaptive near depth {layer} lost photon energy: {energy}");
            }
        }
        if let Some(path)=std::env::var_os("BLOCKWORLD_CAUSTIC_CAPTURE") {
            let path=std::path::PathBuf::from(path);
            image::RgbImage::from_fn(r.width,r.height,|x,y| {let c=pixels[(x+y*r.width) as usize];image::Rgb([(c>>16) as u8,(c>>8) as u8,c as u8])}).save(path.with_extension(format!("{name}.png")))?;
            let data=r.caustic_data();let base=if near {5*65536} else {0};
            image::RgbImage::from_fn(256,256,|x,y| {
                let i=(x+y*256) as usize;
                let density=if shallow {data[base+4*65536+i] as f32/8192.0} else {(data[base+65536+i]+data[base+2*65536+i]) as f32/16384.0};
                let c=(density*100.0).clamp(0.0,255.0) as u8;image::Rgb([c,c,c])
            }).save(path.with_extension(format!("{name}.density.png")))?;
        }
    }
    // Tiny time changes must not expose hard footprint-support boundaries.
    r.water_time=Some(1.2501);let continuous=r.render(&camera,origin,0)?.to_vec();
    ensure!(previous.iter().zip(&continuous).all(|(&a,&b)|[0,8,16].into_iter().all(|shift|((a>>shift)&255).abs_diff((b>>shift)&255)<=8)),"Adaptive footprint animation has a discontinuity");
    r.water_time=Some(1.25);
    let original_x=camera.feet.x;camera.feet.x=1.999;r.render(&camera,origin,0)?;
    let before_snap=r.caustic_data().to_vec();camera.feet.x=2.001;r.render(&camera,origin,0)?;
    for layer in 1..=5 {
        let mut maximum=0;
        for y in 96..144 {for x in 96..144 {
            maximum=maximum.max(before_snap[(5+layer)*65536+x+16+256*y].abs_diff(r.caustic_data()[(5+layer)*65536+x+256*y]));
        }}
        ensure!(maximum<=16,"Fine map changed at origin snap, depth {layer}: {maximum}/8192");
    }
    camera.feet.x=original_x;
    // No density contrast at equal IOR, including the adaptive near footprint.
    r.settings.water_ior=1.0;r.render(&camera,origin,0)?;
    for layer in 1..=5 {for y in 100..156 {for x in 100..156 {
        ensure!(r.caustic_data()[(5+layer)*65536+x+256*y].abs_diff(8192)<=16,"Adaptive equal-IOR footprint gained contrast");
    }}}
    r.settings.water_ior=RenderSettings::default().water_ior;
    // A roof must reject its entire source column, including the hidden pool.
    for z in -36..=36 {for x in -36..=36 {put(&mut blocks,origin,IVec3::new(x,106,z),1);}}
    r.upload(&blocks)?;r.settings.caustics=true;camera.feet.y=101.38;r.settings.caustic_distance=64.0;
    r.render(&camera,origin,0)?;
    ensure!(r.caustic_data().iter().all(|&v|v==0),"Covered pool generated photons");
    println!("Caustics passed: {occupied} source texels, {changed} underwater pixels, four energy-conserving depth slices, equal-IOR flat flux, deterministic animated waves, optical gates, far fade, covered-pool rejection, staged fine/adaptive/shallow comparisons and near-map energy. GPU map {map_ms:.3} ms");
    r.settings=saved;r.water_time=replay;r.resize_targets(extent.0,extent.1)?;
    Ok(())
}
