//! Compile-time geometry, like C++ constexpr. All dimensions are powers of two.
pub const BLOCK_METERS: f32 = 1.0;
pub const SUBCHUNK_BITS: usize = 4;
pub const EDGE: usize = 1 << SUBCHUNK_BITS;
pub const SUBCHUNK_VOLUME: usize = EDGE * EDGE * EDGE;
pub const SUBCHUNKS: usize = 24;
pub const BELOW_ZERO: usize = 4;
pub const MIN_Y: i32 = -(BELOW_ZERO as i32 * EDGE as i32);
pub const HEIGHT: usize = EDGE * SUBCHUNKS;
pub const MAX_Y: i32 = MIN_Y + HEIGHT as i32; // exclusive: 320
pub const REGION_EDGE: usize = 16; // change to 32 or 64 and rebuild
pub const REGION_CHUNKS: usize = REGION_EDGE * REGION_EDGE;
pub const DEFAULT_VIEW_CHUNKS: i32 = 8;
pub const MAX_VIEW_CHUNKS: i32 = 128;
// Extra chunk prevents the camera reaching an unloaded border within its ray range.
pub const LOAD_RADIUS: i32 = DEFAULT_VIEW_CHUNKS + 1;
pub const GPU_EDGE: usize = (LOAD_RADIUS as usize * 2 + 1) * EDGE;
pub const GPU_VOXELS: usize = GPU_EDGE * GPU_EDGE * HEIGHT;
pub const SEA_LEVEL: i32 = 5;
// Headless fixture dimensions; interactive targets use the swapchain extent.
pub const RENDER_WIDTH: u32 = 960;
pub const RENDER_HEIGHT: u32 = 540;
pub const REACH: f32 = 7.0;
const _: () = assert!(REGION_EDGE >= 16 && REGION_EDGE <= 64 && REGION_EDGE.is_power_of_two());
const _: () = assert!(EDGE == 16 && SUBCHUNKS == 24 && BELOW_ZERO == 4);
const _: () = assert!(BLOCK_METERS == 1.0);
// Hierarchy: 16³ sections -> 4³-block cells -> individual voxels.
pub const GPU_SECTIONS_EDGE: usize = GPU_EDGE / EDGE;
pub const GPU_SECTIONS: usize = GPU_SECTIONS_EDGE * GPU_SECTIONS_EDGE * SUBCHUNKS;
pub const BRICK_EDGE: usize = 4;
pub const BRICKS_EDGE: usize = EDGE / BRICK_EDGE;
pub const FOV_Y_DEGREES: f32 = 70.0;
const _: () = assert!(BRICKS_EDGE * BRICKS_EDGE * BRICKS_EDGE == 64);

/// Validate a dense X/Z-square window with the fixed full vertical extent.
pub fn window_edge(len: usize) -> Option<usize> {
    if !len.is_multiple_of(HEIGHT) {
        return None;
    }
    let edge = ((len / HEIGHT) as f64).sqrt() as usize;
    (edge > 0 && edge.is_multiple_of(EDGE) && edge * edge * HEIGHT == len).then_some(edge)
}

// Block atlas dimensions; wave textures have their own independent resolution.
pub const TEXTURE_EDGE: usize = 16;
pub const TEXTURE_PIXELS: usize = TEXTURE_EDGE * TEXTURE_EDGE;
pub const SNOW_LEVEL: i32 = 110;

// GPU word loads unpack four byte-sized material IDs in little-endian order.
const _: () = assert!(cfg!(target_endian = "little"));

// Exact voxel/physics window remains bounded independently of distant terrain.
pub const NEAR_VIEW_CHUNKS: i32 = 8;

// Stable material IDs 1..19; 7..11 remain reserved for polygon materials.
pub const MATERIAL_COUNT:usize=19;
