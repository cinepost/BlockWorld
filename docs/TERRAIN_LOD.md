# Sparse voxel terrain LOD

The default range remains 8 chunks (128 m); the maximum is now 128 chunks
(2,048 m). Start with:

```sh
cargo run --release -- --view-chunks 64 --lod-pixels 8 --mountain-view --fly
```

F11 shows the HUD. **LOD voxel target (pixels)** controls detail. Smaller values
retain finer voxels. The default threshold is now 1 pixel. **LOD effective target** reports any adjustment needed to
stay within the distant-terrain memory budget. The per-level page counters are
ordered 1, 2, 4, 8 and 16 metres. These are voxel mip levels, not triangle meshes.

## SVO representation and selection

The exact world/physics/animated-mesh window stays at most 19×19 chunks, including
its guard band. A separate sparse, device-local 256 MiB pool stores distant chunks;
increasing view distance never allocates a full-resolution 257×257 volume. A
power-of-two toroidal page table uses signed chunk tags, so wrapping and negative
world coordinates cannot alias another resident chunk.

Each distant chunk is a sparse octree forest, with one potential root per 16³
subchunk. A 24-bit root-presence mask omits empty sections; root indices are
implicit popcount ranks. Each node occupies eight bytes: a relative first-child
index plus an 8-bit child mask and 8-bit aggregate material-palette index. Present
siblings are contiguous, addressed by `base + popcount(mask & preceding_bits)`.
Voxel positions are implicit in the octant path; there are no vertex positions.
Uniform regions collapse to leaves, including full solid/water sections. A page
containing one uniform section takes 12 bytes (header + leaf), versus 4,096 bytes
of block IDs. Empty pages contain only the four-byte root mask.

CPU mip truncation is disabled while the requested detail fits actual residency:
full-detail SVOs are uploaded and GPU traversal selects the visible detail. During
GPU ray traversal, a node stops before its descendants only when the conservative
projected AABB width and height are both at most the pixel threshold (default 1,
configurable in ImGui/`--lod-pixels`). The bound accounts for off-axis projection
and depth extent, rather than just side length over radial distance.
Descent and empty-child
skips occur in compute shaders, front to back; opaque hits stop traversal, so
occluded descendants are not visited. Screen rays naturally restrict primary
work to the camera view; no separate GPU node-compaction/Hi-Z occlusion prepass is
claimed. Uniform coarse leaves can still repeat fine block textures when those
texels remain significant on screen.

Aggregate appearance uses per-face, alpha-weighted texture averages in a separate
material palette. Pack tint and animated-frame interpolation update the palette
without rebuilding the tree. Mixed nodes retain a representative surface
material (grass/sand/snow preferred over mixed stone/dirt), not a full multi-material
radiance mixture. Visibility flags select averaged appearance for coarse hits;
opaque shading, shadow-denoiser correction and secondary water rays share it.
The CPU SVO cache has a separate 64 MiB LRU budget. Generation/edit preparation
remains on `terrain-lod`; physics still uses the exact near window.

The near resident window always takes priority, including its empty space.
The old dense full-height budget estimate was removed: it could silently turn a
1-pixel request into a 7-pixel storage floor despite sparse compression. The
normal residency target now remains the requested value. The SVO GPU pool is
256 MiB; only actual encoded residency/allocator pressure can relax the target,
with headroom for replacing pages. Pressure adjustments are rate limited, retain
old pages, and re-offer unresident pages without retransferring identical resident
versions/scales. The HUD reports the actual residency target. If pressure forces
CPU truncation, it uses a conservative extra level of detail. Shader stopping
always uses the user's requested pixel threshold on the available tree.

Default CPU **chunk** cache is now 512 MiB (`--cache-mb` / ImGui); the separate
CPU SVO preparation cache retains its 64 MiB limit. Pages outside the requested
window are evicted and allocations coalesced. No hidden dense-world estimate
relaxes quality merely because the framebuffer is large.

## Worker and progressive uploads

The `terrain-lod` thread generates chunks, applies runtime edits, builds mip
pyramids and selects pages. It handles the latest request at chunk boundaries and
prepares nearer chunks first. A bounded completion queue provides cancellable
backpressure. Shrinking the view cancels unnecessary work; shutdown joins the
worker even when that queue is full.

The render thread accepts at most eight pages / 1 MiB per frame. Payloads go
through a staging buffer into device-local storage; page-table publication and
transfer-to-compute barriers occur before readers in the same frame. The previous
frame fence protects allocator reuse. Terrain appears progressively without
waiting for the entire distant window, or rebuilding the near BLAS/TLAS ring.
No rendered image is read back to display the interactive frame.

Primary visibility, optical secondary rays and voxel shadow rays share the sparse
traversal. Empty page and vertical-slab skips retain exact slab crossings. Rays are clipped
to resident page bounds. Traversal now advances integer cell coordinates at absolute grid crossings;
this avoids both epsilon-skipped edge hits and kilometre-long CSM stalls. Coarse
CSM bounds include distant pages, and page changes invalidate shadow maps.
Sun-depth bounds cover the full distant world height. CPU frustum bounds on
resident pages preserve the sky-only fast path.

## Scope and future storage

This follows the broad voxel-LOD idea of [Voxy](https://github.com/MCRcortex/voxy),
using this engine's ray traversal and material buffers. It is not a port of Voxy.
`CacheKey` separates generator version, seed and signed chunk coordinates; page
levels contain scale, root-aligned bounds, a sparse root mask and packed nodes. These form the boundary
for a future disk cache. The stress-generator mode occupies a distinct generator-version namespace.
Disk persistence is not implemented yet.

Edits use per-chunk content versions, independent of the near-window/AS generation.
The journal diff detects placement, removal, changed materials and restored
entries, including tree edits spanning chunk boundaries. Only affected chunk
pyramids are invalidated; untouched RAM cache entries, queued results and GPU
pages remain valid. Already-published dirty pages stay visible while the worker
rebuilds them. Changed resident chunks take priority over background loading.
The replacement allocation and payload must be ready before the page table swaps
at a frame boundary; insufficient allocation space postpones the swap. Stale
results are rejected by that chunk's version, not a global world revision.

Chunk payload roots remain 16 m, preserving independent edits/uploads.
Uniform cross-chunk parents now accelerate complete homogeneous regions;
child page storage remains allocated.
Coverage mixtures and transition morphing remain future work. Coarse silhouettes expand conservatively, leaves
use opaque aggregate coverage, and changes in cell size can visibly pop. NPC fish use the separate 64 m player activation boxes;
static crossed foliage cards remain in the near terrain mesh window. Trees in distant
terrain remain voxel aggregates. Different LOD boundaries can produce stepped
terrain; they do not use mesh crack stitching.

Dirt retains material ID 3 (previously named soil). Foliage cards now cross at
45° and 135° to the terrain axes; their alpha coverage and shadow paths are shared.

## Validation commands

```sh
cargo test
BLOCKWORLD_VALIDATION=1 BLOCKWORLD_SYNC_VALIDATION=1 target/release/blockworld --svo-test
cargo clippy --all-targets -- -D warnings
cargo build --release
BLOCKWORLD_VALIDATION=1 BLOCKWORLD_SYNC_VALIDATION=1 target/release/blockworld --lod-test
BLOCKWORLD_VALIDATION=1 BLOCKWORLD_SYNC_VALIDATION=1 target/release/blockworld --lod-edit-test
BLOCKWORLD_VALIDATION=1 BLOCKWORLD_SYNC_VALIDATION=1 target/release/blockworld --lod-stream-test
BLOCKWORLD_VALIDATION=1 BLOCKWORLD_SYNC_VALIDATION=1 target/release/blockworld --view-chunks 64 --stream-test --resize-test --frames 600 --no-audio
```

`--lod-test` checks sparse hits at 300, 700 and 1,021 metres, exact depth, range
clipping, all five scales, a steep partial-slab crossing, 5.3 km float32 traversal
and eviction with a bounded dense window. `--lod-stream-test` waits for
all 16,352 distant pages of a 64-chunk window, checks visible far terrain and
memory bounds, then shrinks the window and verifies eviction. These headless QA
checks read visibility data for assertions; interactive presentation does not.

`--lod-edit-test` keeps 152 pages resident while a near edit uploads zero LOD
pages, then places and removes a distant block, replacing exactly one page per
change. Resident counts stay constant throughout preparation and publication.


Cross-chunk traversal prototype: complete homogeneous 32³ and 64³ parents
are rebuilt only for affected 4×4 chunk groups as pages arrive, change or leave.
Unknown pages prevent merging. A fixed 24-word record per page describes the
largest uniform parent at each section. Parent skips are disabled when they
intersect the exact near window. Child pages stay resident: this first stage
reduces traversal steps, but does not reclaim child storage. At the 128-chunk
maximum, page tags plus merge records reserve approximately 32 MiB of the
256 MiB GPU pool. The existing measured-pressure fallback remains enabled.

`--noisy-world --fly` selects a deterministic independent-voxel volume from
Y=-64 through 63, with 3/8 air and five opaque materials. It deliberately
resists homogeneous subtree collapse; use identical cameras and settings when
comparing near occupancy traversal with a future near-SVO implementation.

Toggle **Cross-chunk uniform merging** in the HUD, or launch with `--no-lod-merging`, for an identical-scene comparison. `BLOCKWORLD_LOD_TEST_VIEW=128` selects the maximum range for `--lod-stream-test`.
