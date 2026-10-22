# Triangle fish, alpha-cutout foliage and hybrid rendering

## Scene and animation

Decoration placement uses a world-aligned sampling grid and stable coordinate hashes.
Each chunk owns up to four grass/fern plants (crossed two-sided cards, four
triangles each), and water chunks selected by a stable hash own one fish
(32 triangles). Fish anchors require water depth and nearby clearance. Sampling,
variant selection and animation phase do not depend on the upload-window origin,
object index or player distance. Crossing a chunk boundary or changing view distance
therefore retains the exact foliage vertices and fish poses in overlapping chunks.
Fish candidates keep their swimming envelope inside the owning chunk, so
neighbor-window availability cannot change their selection.
Counts scale with loaded range; the old player-ranked global caps were removed.
Fish orbit their anchors and bob slightly without AI/pathfinding.

Each fish has bind positions, UVs, two joint indices and weights per vertex.
Three joint matrices deform the body/tail through linear blend skinning. Separate
object matrices perform swimming motion. The prototype skins on CPU and writes
coherent host-visible attribute/AS-input buffers before queue submission, after
the previous frame's fence. This deliberately separates geometry deformation
(BLAS work) from object movement (TLAS work). Static foliage groups use Euclidean
16³ section coordinates, including negative world positions.

## Real Vulkan acceleration structures

Vulkan 1.2 buffer-device-address features plus `VK_KHR_acceleration_structure`,
`VK_KHR_ray_query` and `VK_KHR_deferred_host_operations` are detected and enabled
only when supported. Unsupported devices select software triangle traversal.

The scene allocates one triangle BLAS per fish and one per occupied foliage
section. Each fish BLAS is built with `ALLOW_UPDATE` and refitted each active
frame. The ImGui rebuild switch selects BUILD instead of UPDATE for fish.
Static foliage BLASes build only when the scene snapshot changes. A single TLAS
instances every mesh object, including off-screen objects needed for shadows;
its transforms/bounds update after BLAS work. Interactive rendering keeps three
complete mesh/AS slots, each with its own geometry, instances, AS storage and
scratch. A worker refits a free slot while the renderer uses a completed slot. Build barriers order BLAS → TLAS →
compute queries. BLAS storage and scratch use disjoint aligned slices in pooled
buffers; independent builds share batched commands. The interactive worker caps
batches at 16 structures to bound driver submission work.

Positions for BLAS input are local-space, tightly packed 16-byte vectors. TLAS
instances supply the same matrices used by software intersection and hit shading.
BLAS primitive indices map back into the triangle attribute buffer through each
instance's custom object index. No shader-binding table or full ray-tracing
pipeline is needed for these compute ray queries.

Disabling ray queries selects software object-AABB/triangle tests for both primary
mesh visibility and mesh shadows. AS resources remain allocated and the interactive worker keeps snapshots ready
for mode switching. The synchronous headless comparison skips unused updates.
`BLOCKWORLD_DISABLE_RAY_QUERY=1` disables the optional feature path and AS resources
entirely, exercising the unsupported-device fallback. Software triangle traversal
means compute shader intersection, not CPU per-pixel frame readback.
This is a comparison/fallback, not a claim that a CPU Vulkan implementation uses
physical RT cores.

## Pass order and water

```text
Worker: skinning + uploads → fish BLAS build/refit → TLAS update → completion semaphore
Renderer: adopt latest completed mesh/AS slot, otherwise retain previous slot
  → voxel visibility → mesh primary rays clipped by voxel depth
  → resolve nearer accepted polygons and their water paths → SSAO
  → mesh shadow queries → deferred voxel shadows + Lambert + water composite
  → direct GPU swapchain copy → ImGui → present
```

The polygon pass writes a 48-byte `MeshHit` (normal/depth, UV, material, flags,
object and primitive IDs). Its ray limit is the nearest opaque voxel depth;
hidden polygons are never accepted. Accepted foreground pixels with water
retrace only up to the polygon depth to obtain the correct attenuation. The merged 64-byte visibility
record retains the water interface while replacing the opaque/background surface
with the fish or plant when nearer. This avoids over-attenuating underwater fish
using a lake-bed depth.

## Material alpha contract

The material table has flags, an alpha cutoff and reserved AO policy data. All
textures are actual deterministic 16×16 RGBA arrays. Flags currently include:

- `ALPHA_MASK`: test texture alpha against cutoff (0.5 by default).
- `DOUBLE_SIDED`: vegetation card policy; triangle queries currently use two-sided
  intersections for fish fins too, and orient shading normals toward the ray.
- `TRANSMISSION`: identifies water; voxel traversal handles its layered transport.

Grass and fern cards use alpha masking, not ordinary blended transparency or
projected decals. Ray-query candidates interpolate UVs and confirm a hit only if
coverage passes. Software triangle traversal applies the identical test. Opaque
fish BLAS geometry can use hardware opaque-hit handling.

A hole does not write card depth, so the visible surface behind it is what SSAO
reconstructs. Sun-shadow rays repeat the coverage test. The combined visibility
`valid` field is now a bitfield: bit 0 accepted hit, bit 1 masked material, bit 2
polygon surface, bits 8+ object index plus one. Existing consumers use nonzero for
validity; the debug view distinguishes voxels, meshes and masked meshes.

## Measurements and controls

The overlay reports CPU skinning/upload time, GPU BLAS/TLAS times, primary mesh
rays and mesh shadows, alongside voxel/SSAO/shading time. Nine timestamp queries
measure actual GPU stages, including AS commands. The benchmark prints means
excluding its first 60 frames, plus first-build GPU time and CPU scene setup.
FPS includes initial asynchronous mesh preparation; initial world/Vulkan creation
occurs before the run timer. AS memory reports requested input/storage/scratch
buffer bytes, not the driver's hidden overhead or total renderer memory.

```sh
target/release/blockworld --frames 1000
target/release/blockworld --frames 1000 --no-meshes
target/release/blockworld --frames 1000 --no-rt
target/release/blockworld --frames 1000 --rebuild-blas
```

These small fish are not a substitute for measuring large NPC skeletons. Current
foliage is grouped by subchunk, but static foliage results do not establish the
best granularity for procedural voxel BLASes. Preserve 16³ as the smallest initial
voxel-BLAS candidate, then compare 2/4/24-section groups with identical edit and
streaming traces. Track p95 update latency, memory and traversal cost as well as
average FPS.

## Future voxel LOD boundary

LOD is not implemented yet. Keep simulation/edit data at full resolution and
build derived summaries; do not destructively replace world blocks with coarse
ones. A section's hierarchy can grow into 16³ → 8³ → 4³ → 2³ → 1³ mips. Every
summary should retain occupancy, opaque coverage, representative material/normal,
water fraction and world-space cell size. A dominant material alone loses foliage
holes and water/air boundaries.

After a subchunk becomes one coarse cell, this world's **chunk is a 1×24×1 column
of section cells**, not a 16³ cube: it has one section across X/Z and 24 vertically.
Chunk/region aggregation must account for that shape. Spatially adjacent chunk
columns form the next coarse grid; vertical padding or an explicitly non-power-
of-two hierarchy can handle the 24-section height. Use world-space coordinates so
camera depth and ray distances stay in metres at every LOD.

Animated entities remain separate polygon instances with their own mesh LODs;
foliage can use coverage-preserving cards/impostors and distance policies. Do not
fold swimming fish into a coarse static voxel material. Future secondary-ray LOD
must also consider shadow leaks when primary and shadow geometry resolutions differ.

Primary API reference: [Khronos alpha-tested ray queries](https://docs.vulkan.org/tutorial/latest/Building_a_Simple_Engine/Advanced_Topics/Ray_Query_Reflections_and_Transparency.html).


Primary mesh miss traffic is now sparse in windowed rendering: only hits write
48-byte records, paired with current-frame epochs in binding 17. Resolve first
checks the compact epoch array. A separate opaque/water flag region lets later
passes reject empty pixels cheaply. Scene bounds and a conservative nearest-mesh
distance reject impossible primary rays before hardware traversal; the latter is
not used for shadow rays. See RENDERING.md and PERFORMANCE.md for details.
