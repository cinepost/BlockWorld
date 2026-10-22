//! Small skeletal fish and section-grouped alpha-cutout vegetation.
use crate::config::*;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};
use std::collections::BTreeMap;
pub const MAX_OBJECTS: usize = 16384;
pub const MAX_TRIANGLES: usize = 131072;
pub const MATERIALS: usize = MATERIAL_COUNT;
pub const ALPHA_MASK: u32 = 1;
pub const DOUBLE_SIDED: u32 = 2;
pub const TRANSMISSION: u32 = 4;
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Material {
    pub flags: u32,
    pub alpha_cutoff: f32,
    pub ao_weight: f32,
    pub pad: u32,
}
pub fn materials() -> [Material; MATERIALS] {
    std::array::from_fn(|i| Material {
        flags: match i {
            4 => TRANSMISSION,
            8..=10 | 12 | 13 => ALPHA_MASK | DOUBLE_SIDED,
            _ => 0,
        },
        alpha_cutoff: 0.5,
        ao_weight: 1.0,
        pad: 0,
    })
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct Triangle {
    pub a: [f32; 4],
    pub b: [f32; 4],
    pub c: [f32; 4],
    pub uv01: [f32; 4],
    pub uv2: [f32; 2],
    pub material: u32,
    pub flags: u32,
}
impl Triangle {
    pub(crate) fn new(v: [Vec3; 3], uv: [Vec2; 3], material: u32) -> Self {
        Self {
            a: v[0].extend(0.0).to_array(),
            b: v[1].extend(0.0).to_array(),
            c: v[2].extend(0.0).to_array(),
            uv01: [uv[0].x, uv[0].y, uv[1].x, uv[1].y],
            uv2: uv[2].to_array(),
            material,
            flags: 0,
        }
    }
    pub fn points(self) -> [Vec3; 3] {
        [
            Vec3::from_array(self.a[..3].try_into().unwrap()),
            Vec3::from_array(self.b[..3].try_into().unwrap()),
            Vec3::from_array(self.c[..3].try_into().unwrap()),
        ]
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Object {
    pub world: [[f32; 4]; 4],
    pub low: [f32; 4],
    pub high: [f32; 4],
    pub info: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct MeshHit {
    pub normal_depth: [f32; 4],
    pub uv: [f32; 2],
    pub material: u32,
    pub flags: u32,
    pub ids: [u32; 4],
}
const _: () = assert!(
    std::mem::size_of::<Triangle>() == 80
        && std::mem::size_of::<Object>() == 112
        && std::mem::size_of::<MeshHit>() == 48
);
#[derive(Clone, Copy)]
pub(crate) struct SkinVertex {
    pub(crate) point: Vec3,
    pub(crate) uv: Vec2,
    pub(crate) joints: [usize; 2],
    pub(crate) weights: [f32; 2],
}
#[derive(Clone)]
pub(crate) struct Fish {
    pub(crate) anchor: Vec3,
    pub(crate) phase: f32,
    pub(crate) bind: Vec<[SkinVertex; 3]>,
    pub(crate) object: usize,
}
#[derive(Clone)]
pub struct MeshScene {
    pub objects: Vec<Object>,
    pub triangles: Vec<Triangle>,
    pub(crate) fish: Vec<Fish>,
    pub plants: usize,
}
fn skin_vertex(point: Vec3) -> SkinVertex {
    let u = ((0.35 - point.x) / 1.1).clamp(0.0, 1.0) * 2.0;
    let joint = (u.floor() as usize).min(1);
    let weight = (u - joint as f32).clamp(0.0, 1.0);
    SkinVertex {
        point,
        uv: Vec2::new((point.x + 1.1) / 1.9, (point.y + 0.4) / 0.8),
        joints: [joint, joint + 1],
        weights: [1.0 - weight, weight],
    }
}
fn fish_mesh() -> Vec<[SkinVertex; 3]> {
    let rings = [
        (0.65, 0.07, 0.06),
        (0.20, 0.25, 0.18),
        (-0.35, 0.18, 0.12),
        (-0.65, 0.05, 0.04),
    ];
    let mut triangles = Vec::new();
    let ring = |i: usize, k: usize| {
        let (x, y, z) = rings[i];
        match k % 4 {
            0 => Vec3::new(x, y, 0.0),
            1 => Vec3::new(x, 0.0, z),
            2 => Vec3::new(x, -y, 0.0),
            _ => Vec3::new(x, 0.0, -z),
        }
    };
    let mut push = |a, b, c| triangles.push([skin_vertex(a), skin_vertex(b), skin_vertex(c)]);
    for i in 0..3 {
        for k in 0..4 {
            push(ring(i, k), ring(i + 1, k), ring(i + 1, k + 1));
            push(ring(i, k), ring(i + 1, k + 1), ring(i, k + 1));
        }
    }
    for k in 0..4 {
        push(Vec3::new(0.74, 0.0, 0.0), ring(0, k), ring(0, k + 1));
    }
    push(
        Vec3::new(-0.6, 0.0, 0.0),
        Vec3::new(-1.05, 0.36, 0.0),
        Vec3::new(-0.96, -0.36, 0.0),
    );
    push(
        Vec3::new(0.15, 0.18, 0.0),
        Vec3::new(-0.25, 0.43, 0.0),
        Vec3::new(-0.4, 0.14, 0.0),
    );
    push(
        Vec3::new(0.12, -0.04, 0.12),
        Vec3::new(-0.28, -0.18, 0.42),
        Vec3::new(-0.36, -0.08, 0.1),
    );
    push(
        Vec3::new(0.12, -0.04, -0.12),
        Vec3::new(-0.28, -0.18, -0.42),
        Vec3::new(-0.36, -0.08, -0.1),
    );
    triangles
}
fn cards(center: Vec3, width: f32, height: f32, material: u32) -> Vec<Triangle> {
    let mut out = Vec::new();
    for d in [Vec3::new(1.0, 0.0, 1.0).normalize(), Vec3::new(-1.0, 0.0, 1.0).normalize()] {
        let a = center - d * width * 0.5;
        let b = center + d * width * 0.5;
        let c = b + Vec3::Y * height;
        let e = a + Vec3::Y * height;
        out.push(Triangle::new(
            [a, b, c],
            [
                Vec2::new(0.0, 1.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(1.0, 0.0),
            ],
            material,
        ));
        out.push(Triangle::new(
            [a, c, e],
            [
                Vec2::new(0.0, 1.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(0.0, 0.0),
            ],
            material,
        ));
    }
    out
}
fn decoration_hash(x: i32, z: i32) -> u32 {
    let n = (x as u32).wrapping_mul(0x9e3779b9) ^ (z as u32).wrapping_mul(0x85ebca6b) ^ 42;
    let n = (n ^ (n >> 16)).wrapping_mul(0x7feb352d);
    n ^ (n >> 15)
}
impl MeshScene {
    pub fn empty() -> Self {
        Self {
            objects: vec![],
            triangles: vec![],
            fish: vec![],
            plants: 0,
        }
    }
    pub fn fish_count(&self) -> usize {
        self.fish.iter().filter(|f| self.objects[f.object].info[3]&crate::entities::INACTIVE==0).count()
    }
    pub fn from_voxels(blocks: &[u8], origin: glam::IVec3) -> Self {
        let mut scene = Self::empty();
        let edge = window_edge(blocks.len()).expect("Valid voxel window");
        let get = |x: i32, y: i32, z: i32| {
            let q = glam::IVec3::new(x, y, z) - origin;
            if q.x < 0
                || q.z < 0
                || q.y < 0
                || q.x >= edge as i32
                || q.z >= edge as i32
                || q.y >= HEIGHT as i32
            {
                0
            } else {
                blocks[q.x as usize + edge * (q.z as usize + edge * q.y as usize)]
            }
        };
        // Sampling, selection, appearance and animation are keyed to owning world chunks.
        // No origin-relative grid or player-ranked global cap may move existing entities.
        let min_x = origin.x.div_euclid(EDGE as i32);
        let min_z = origin.z.div_euclid(EDGE as i32);
        let max_x = (origin.x + edge as i32 - 1).div_euclid(EDGE as i32);
        let max_z = (origin.z + edge as i32 - 1).div_euclid(EDGE as i32);
        let mut groups: BTreeMap<(i32, i32, i32), Vec<Triangle>> = BTreeMap::new();
        for cz in min_z..=max_z {
            for cx in min_x..=max_x {
                let mut water = Vec::new();
                let mut plants = Vec::new();
                for z in cz * EDGE as i32..(cz + 1) * EDGE as i32 {
                    for x in cx * EDGE as i32..(cx + 1) * EDGE as i32 {
                        if x.rem_euclid(3) != 0 || z.rem_euclid(3) != 0 {
                            continue;
                        }
                        if x < origin.x
                            || z < origin.z
                            || x >= origin.x + edge as i32
                            || z >= origin.z + edge as i32
                        {
                            continue;
                        }
                        for y in (MIN_Y..MAX_Y).rev() {
                            let m = get(x, y, z);
                            if m == 0 {
                                continue;
                            }
                            let key = decoration_hash(x, z);
                            // Keep the whole swimming envelope inside its owning chunk.
                            if (2..=13).contains(&x.rem_euclid(EDGE as i32))
                                && (2..=13).contains(&z.rem_euclid(EDGE as i32))
                                && m == 5
                                && get(x, y - 2, z) == 5
                                && [(-2, 0), (2, 0), (0, -2), (0, 2)]
                                    .iter()
                                    .all(|&(dx, dz)| get(x + dx, y - 1, z + dz) == 5)
                            {
                                water.push((
                                    key,
                                    Vec3::new(x as f32 + 0.5, y as f32 - 0.35, z as f32 + 0.5),
                                ));
                            }
                            if matches!(m,2|18|19) && get(x, y + 1, z) == 0 {
                                plants.push((
                                    key,
                                    Vec3::new(x as f32 + 0.5, y as f32 + 1.01, z as f32 + 0.5),
                                ));
                            }
                            break;
                        }
                    }
                }
                water.sort_by_key(|v| v.0);
                plants.sort_by_key(|v| v.0);
                if decoration_hash(cx, cz).is_multiple_of(3) {
                    if let Some(&(key, anchor)) = water.first() {
                        scene.add_fish_material(anchor, (key % 6283) as f32 * 0.001, 7 + (key & 1));
                    }
                }
                for (key, p) in plants.into_iter().take(4) {
                    let owner = (
                        (p.x.floor() as i32).div_euclid(16),
                        (p.y.floor() as i32).div_euclid(16),
                        (p.z.floor() as i32).div_euclid(16),
                    );
                    let fern = key.is_multiple_of(3);
                    groups.entry(owner).or_default().extend(cards(
                        p,
                        if fern { 1.1 } else { 0.8 },
                        if fern { 1.15 } else { 0.85 },
                        if fern { 10 } else { 9 },
                    ));
                    scene.plants += 1;
                }
            }
        }
        for tris in groups.into_values() {
            scene.add_object(tris, Mat4::IDENTITY, 1);
        }
        scene.update(0.0);
        scene
    }
    fn add_object(&mut self, triangles: Vec<Triangle>, transform: Mat4, kind: u32) -> usize {
        let index = self.objects.len();
        assert!(index < MAX_OBJECTS && self.triangles.len() + triangles.len() <= MAX_TRIANGLES);
        self.objects.push(Object {
            world: transform.to_cols_array_2d(),
            low: [0.0; 4],
            high: [0.0; 4],
            info: [self.triangles.len() as u32, triangles.len() as u32, kind, 0],
        });
        self.triangles.extend(triangles);
        self.bounds(index);
        index
    }
    pub fn add_fish(&mut self, anchor: Vec3, phase: f32) {
        self.add_fish_material(anchor, phase, 7);
    }
    fn add_fish_material(&mut self, anchor: Vec3, phase: f32, material: u32) {
        let bind = fish_mesh();
        let triangles = bind
            .iter()
            .map(|t| Triangle::new(t.map(|v| v.point), t.map(|v| v.uv), material))
            .collect();
        let object = self.add_object(triangles, Mat4::from_translation(anchor), 0);
        self.fish.push(Fish {
            anchor,
            phase,
            bind,
            object,
        });
    }
    pub fn add_card_fixture(&mut self, center: Vec3) {
        self.add_object(cards(center, 2.0, 2.0, 9), Mat4::IDENTITY, 1);
        self.plants += 1;
    }
    pub fn update(&mut self, time: f32) {
        crate::entities::animate(self,time,None);
    }
    pub(crate) fn bounds(&mut self, index: usize) {
        let o = self.objects[index];
        let matrix = Mat4::from_cols_array_2d(&o.world);
        let mut low = Vec3::splat(f32::INFINITY);
        let mut high = Vec3::splat(f32::NEG_INFINITY);
        for t in &self.triangles[o.info[0] as usize..(o.info[0] + o.info[1]) as usize] {
            for v in t.points() {
                let p = matrix.transform_point3(v);
                low = low.min(p);
                high = high.max(p);
            }
        }
        self.objects[index].low = low.extend(0.0).to_array();
        self.objects[index].high = high.extend(0.0).to_array();
    }
    pub fn positions(&self) -> Vec<[f32; 4]> {
        self.triangles
            .iter()
            .flat_map(|t| [t.a, t.b, t.c])
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decorations_stay_in_world_space_across_window_changes() {
        let manifest = |cx: i32, cz: i32, radius: i32| {
            let edge = (radius * 2 + 1) as usize * EDGE;
            let o = glam::IVec3::new((cx - radius) * 16, MIN_Y, (cz - radius) * 16);
            let mut b = vec![0u8; edge * edge * HEIGHT];
            // A fixed lake and grass bank exercise both decorations, regardless of seed/terrain.
            for z in 0..edge {
                for x in 0..edge {
                    for y in 0..=5 {
                        b[x + edge * (z + edge * (y - MIN_Y) as usize)] = if z as i32 + o.z < 0 {
                            5
                        } else if y == 5 {
                            2
                        } else {
                            3
                        };
                    }
                }
            }

            let mut scene = MeshScene::from_voxels(&b, o);
            scene.update(1.25);
            // Compare actual foliage vertices and animated fish geometry in the common interior.
            let mut records = Vec::new();
            for object in &scene.objects {
                let world = Mat4::from_cols_array_2d(&object.world);
                let low = Vec3::from_slice(&object.low);
                let high = Vec3::from_slice(&object.high);
                if low.x < -32.0 || high.x >= 16.0 || low.z < -16.0 || high.z >= 16.0 {
                    continue;
                }
                let triangles = &scene.triangles
                    [object.info[0] as usize..(object.info[0] + object.info[1]) as usize];
                for t in triangles {
                    for v in t.points() {
                        let v = world.transform_point3(v);
                        records.push((v.x.to_bits(), v.y.to_bits(), v.z.to_bits(), t.material));
                    }
                }
            }
            records.sort_unstable();
            records
        };
        let a = manifest(0, 0, 3);
        assert!(
            a.iter().any(|v| v.3 == 7 || v.3 == 8),
            "Common area must include fish"
        );
        assert!(
            a.iter().any(|v| v.3 == 9 || v.3 == 10),
            "Common area must include foliage"
        );
        for (x, z, r) in [(-1, 0, 3), (1, 1, 3), (0, -1, 2), (0, 0, 4)] {
            assert_eq!(a, manifest(x, z, r), "Window {x},{z} radius {r}");
        }
    }
    #[test]
    fn skinning_keeps_topology_and_moves_tail() {
        let mut scene = MeshScene::empty();
        scene.add_fish(Vec3::new(0.0, 4.0, 0.0), 0.0);
        scene.update(0.0);
        let old = scene.triangles.clone();
        scene.update(0.5);
        assert_eq!(old.len(), scene.triangles.len());
        assert!(old.iter().zip(&scene.triangles).any(|(a, b)| a.a != b.a));
        for t in &scene.triangles {
            for v in t.points() {
                assert!(v.is_finite());
            }
        }
        for t in fish_mesh() {
            for v in t {
                assert!((v.weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
            }
        }
    }
    #[test]
    fn material_contract() {
        let m = materials();
        assert_eq!(m[4].flags, TRANSMISSION);
        assert!(m[8].flags & ALPHA_MASK != 0);
        assert_eq!(m[6].flags, 0);
    }
}
