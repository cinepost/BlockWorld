//! Conservative camera frustum and a two-level, water-aware occupancy hierarchy.
use crate::{config::*, player::Player};
use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Vec3};

/// 64 cells per section; each bit describes a 4×4×4 group of blocks.
/// Separate solid masks let sunlight rays skip water without losing primary visibility.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Section {
    pub occupied: [u32; 2],
    pub opaque: [u32; 2],
}
impl Section {
    pub fn any(self) -> bool {
        self.occupied != [0; 2]
    }
}
/// Exact occupied Y range, computed off the render thread with each upload.
/// Eight-byte comparisons skip long all-air slabs without requiring aligned input.
pub fn voxel_y_bounds(blocks: &[u8]) -> [u32; 2] {
    let edge = window_edge(blocks.len()).expect("Valid voxel window");
    let plane = edge * edge;
    let mut words = blocks.as_chunks::<8>().0.iter();
    let Some(low) = words.position(|word| *word != [0u8; 8]) else {
        return [0, 0];
    };
    let high = blocks
        .as_chunks::<8>()
        .0
        .iter()
        .rposition(|word| *word != [0u8; 8])
        .unwrap();
    [(low * 8 / plane) as u32, (high * 8 / plane + 1) as u32]
}
/// CPU-only tight voxel AABBs; GPU occupancy remains the original 16-byte ABI.
#[derive(Clone, Copy)]
pub struct SectionBounds {
    pub low: [u8; 3],
    pub high: [u8; 3],
}
impl Default for SectionBounds {
    fn default() -> Self {
        Self {
            low: [16; 3],
            high: [0; 3],
        }
    }
}
#[cfg(test)]
pub fn occupancy(blocks: &[u8]) -> Vec<Section> {
    occupancy_with_bounds(blocks).0
}
pub fn occupancy_with_bounds(blocks: &[u8]) -> (Vec<Section>, Vec<SectionBounds>) {
    let edge = window_edge(blocks.len()).expect("Valid voxel window");
    let side = edge / EDGE;
    let mut sections = vec![Section::default(); side * side * SUBCHUNKS];
    let mut bounds = vec![SectionBounds::default(); sections.len()];
    for y in 0..HEIGHT {
        for z in 0..edge {
            for x in 0..edge {
                let material = blocks[x + edge * (z + edge * y)];
                if material == 0 {
                    continue;
                }
                let section = x / EDGE + side * (z / EDGE + side * (y / EDGE));
                for (axis, coordinate) in [x % EDGE, y % EDGE, z % EDGE].into_iter().enumerate() {
                    bounds[section].low[axis] = bounds[section].low[axis].min(coordinate as u8);
                    bounds[section].high[axis] =
                        bounds[section].high[axis].max(coordinate as u8 + 1);
                }
                let brick = (x % EDGE) / BRICK_EDGE
                    + BRICKS_EDGE
                        * ((z % EDGE) / BRICK_EDGE + BRICKS_EDGE * ((y % EDGE) / BRICK_EDGE));
                let bit = 1u32 << (brick % 32);
                let word = brick / 32;
                sections[section].occupied[word] |= bit;
                if material != crate::world::Block::Water as u8 {
                    sections[section].opaque[word] |= bit;
                }
            }
        }
    }
    (sections, bounds)
}
struct Plane {
    normal: Vec3,
    offset: f32,
}
pub struct Frustum {
    planes: [Plane; 6],
}
impl Frustum {
    pub fn new(player: &Player, aspect: f32, far: f32) -> Self {
        let eye = player.eye();
        let f = player.forward();
        let r = player.right();
        let u = r.cross(f);
        let v = (FOV_Y_DEGREES.to_radians() * 0.5).tan();
        let h = v * aspect;
        let plane = |n: Vec3, d: f32| Plane {
            normal: n,
            offset: -n.dot(eye) + d,
        };
        Self {
            planes: [
                plane(f, 0.0),
                plane(-f, far),
                plane(f * h + r, 0.0),
                plane(f * h - r, 0.0),
                plane(f * v + u, 0.0),
                plane(f * v - u, 0.0),
            ],
        }
    }
    /// Positive-support AABB test. Touching a plane is retained (conservative).
    pub fn intersects(&self, min: Vec3, max: Vec3) -> bool {
        let center = (min + max) * 0.5;
        let radius = (max - min) * 0.5;
        self.planes
            .iter()
            .all(|p| p.normal.dot(center) + p.offset + radius.dot(p.normal.abs()) >= -0.001)
    }
}
pub fn visible_sections(
    sections: &[Section],
    bounds: Option<&[SectionBounds]>,
    origin: IVec3,
    frustum: &Frustum,
    cull: bool,
) -> (Vec<u32>, usize, usize) {
    let side = ((sections.len() / SUBCHUNKS) as f64).sqrt() as usize;
    assert_eq!(sections.len(), side * side * SUBCHUNKS);
    let mut visible = vec![0; sections.len()];
    let mut occupied = 0;
    let mut kept = 0;
    for z in 0..side {
        for x in 0..side {
            let chunk_min = origin.as_vec3() + Vec3::new((x * EDGE) as f32, 0.0, (z * EDGE) as f32);
            let chunk_visible = !cull
                || frustum.intersects(
                    chunk_min,
                    chunk_min + Vec3::new(EDGE as f32, HEIGHT as f32, EDGE as f32),
                );
            for y in 0..SUBCHUNKS {
                let i = x + side * (z + side * y);
                if !sections[i].any() {
                    continue;
                }
                occupied += 1;
                if !chunk_visible {
                    continue;
                }
                let base = chunk_min + Vec3::Y * (y * EDGE) as f32;
                let (low, high) = if let Some(b) = bounds {
                    (
                        Vec3::new(b[i].low[0] as f32, b[i].low[1] as f32, b[i].low[2] as f32),
                        Vec3::new(
                            b[i].high[0] as f32,
                            b[i].high[1] as f32,
                            b[i].high[2] as f32,
                        ),
                    )
                } else {
                    (Vec3::ZERO, Vec3::splat(EDGE as f32))
                };
                if !cull || frustum.intersects(base + low, base + high) {
                    visible[i] = 1;
                    kept += 1;
                }
            }
        }
    }
    (visible, occupied, kept)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tight_bounds_reject_skyward_empty_section_volume() {
        let edge = 16;
        let mut blocks = vec![0u8; edge * edge * HEIGHT];
        blocks[edge * edge * (-MIN_Y) as usize] = 2;
        let (sections, bounds) = occupancy_with_bounds(&blocks);
        let origin = IVec3::new(0, MIN_Y, 0);
        let mut camera = Player::new(2.0);
        camera.pitch = std::f32::consts::FRAC_PI_2;
        let f = Frustum::new(&camera, 16.0 / 9.0, 128.0);
        assert_eq!(visible_sections(&sections, None, origin, &f, true).2, 1);
        assert_eq!(
            visible_sections(&sections, Some(&bounds), origin, &f, true).2,
            0
        );
        assert_eq!(
            visible_sections(&sections, Some(&bounds), origin, &f, false).2,
            1
        );
        let b = bounds[(-MIN_Y) as usize / 16];
        assert_eq!(b.low, [0, 0, 0]);
        assert_eq!(b.high, [1, 1, 1]);
    }
    #[test]
    fn exact_vertical_bounds_include_water_and_boundary_layers() {
        let edge = 16;
        let plane = edge * edge;
        let mut blocks = vec![0u8; plane * HEIGHT];
        assert_eq!(voxel_y_bounds(&blocks), [0, 0]);
        blocks[plane * 63 + plane - 1] = 5;
        blocks[plane * 96 + 7] = 13;
        assert_eq!(voxel_y_bounds(&blocks), [63, 97]);
        blocks[0] = 1;
        let last = blocks.len() - 1;
        blocks[last] = 12;
        assert_eq!(voxel_y_bounds(&blocks), [0, HEIGHT as u32]);
    }
    #[test]
    fn chunk_then_section_culling_matches_brute_force() {
        let side = 5;
        let origin = IVec3::new(-32, MIN_Y, -32);
        let sections = vec![
            Section {
                occupied: [u32::MAX; 2],
                opaque: [u32::MAX; 2]
            };
            side * side * SUBCHUNKS
        ];
        for (yaw, pitch) in [(0.0, 0.0), (1.5, -1.54), (3.1, 0.4)] {
            let mut player = Player::new(100.0);
            player.yaw = yaw;
            player.pitch = pitch;
            let f = Frustum::new(&player, 16.0 / 9.0, 32.0);
            let (mask, total, kept) = visible_sections(&sections, None, origin, &f, true);
            assert_eq!(total, sections.len());
            assert!(kept > 0 && kept < total);
            for y in 0..SUBCHUNKS {
                for z in 0..side {
                    for x in 0..side {
                        let min = origin.as_vec3() + Vec3::new(x as f32, y as f32, z as f32) * 16.0;
                        assert_eq!(
                            mask[x + side * (z + side * y)] != 0,
                            f.intersects(min, min + Vec3::splat(16.0))
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn conservative_frustum() {
        let mut p = Player::new(0.0);
        p.yaw = 0.0;
        p.pitch = 0.0;
        let f = Frustum::new(&p, 16.0 / 9.0, DEFAULT_VIEW_CHUNKS as f32 * EDGE as f32);
        assert!(f.intersects(Vec3::new(-1.0, 0.0, -4.0), Vec3::new(1.0, 4.0, -2.0)));
        assert!(f.intersects(p.eye() - Vec3::ONE, p.eye() + Vec3::ONE));
        assert!(!f.intersects(Vec3::new(-1.0, 0.0, 4.0), Vec3::new(1.0, 4.0, 8.0)));
        assert!(!f.intersects(Vec3::new(100.0, 0.0, -4.0), Vec3::new(104.0, 4.0, -2.0)));
        assert!(!f.intersects(Vec3::new(-1.0, 0.0, -310.0), Vec3::new(1.0, 4.0, -300.0)));
        // Large section crossing the near plane must not disappear.
        assert!(f.intersects(Vec3::splat(-8.0), Vec3::splat(8.0)));
    }
    #[test]
    fn water_and_solid_masks_update_on_edit() {
        let mut blocks = vec![0; GPU_VOXELS];
        let a = 15 + GPU_EDGE * (15 + GPU_EDGE * 15);
        let b = 16 + GPU_EDGE * (16 + GPU_EDGE * 16);
        blocks[a] = 5;
        blocks[b] = 1;
        let sections = occupancy(&blocks);
        assert_eq!(sections[0].occupied, [0, 1 << 31]);
        assert_eq!(sections[0].opaque, [0; 2]);
        let i = 1 + GPU_SECTIONS_EDGE * (1 + GPU_SECTIONS_EDGE);
        assert_eq!(sections[i].opaque, [1, 0]);
        blocks[a] = 0;
        blocks[b] = 0;
        assert!(occupancy(&blocks).iter().all(|s| !s.any()));
    }
}
