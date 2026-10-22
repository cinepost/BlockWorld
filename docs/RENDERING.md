# Hierarchical visibility, transparent water, sunlight and SSAO

## Traversal and culling

`spatial::occupancy` constructs a two-level hierarchy when voxel data is uploaded.
For each 16³ section, a pair of `u32` words marks its 64 groups of 4³ blocks.
A second pair marks only opaque groups. Air sections skip whole 16 m cells;
empty groups skip 4 m cells; occupied groups resolve 1 m voxels. These masks are
derived separately from the 4096-byte CPU SubChunk so its aligned layout stays
unchanged. Deleting the last block correctly clears its occupancy at the next
upload. Metadata currently rebuilds on edits/chunk crossings, not incrementally.

Primary traversal uses any-material occupancy. Sun shadow traversal uses opaque
occupancy, making water non-blocking. The visibility shader records iteration
counts for the traversal-work debug view. Disabling hierarchy retains ordinary
one-block stepping, which the headless tests compare against every initial-frame
pixel for material, opaque depth and water length.

Six CPU frustum planes use positive-support AABB tests to produce a conservative
chunk-column mask and then a section mask. The second test only visits
children of intersecting chunk bounds. Large sections intersecting the camera/near plane remain visible.
The mask applies only to primary rays. Occupancy for off-screen sections remains
available to shadows and optical secondary rays. Primary rays already lie within the camera frustum; the
explicit mask is educational and reports spatial candidates rather than removing
mesh draws. The hierarchy is responsible for actual empty-space skipping.

Traversal starts with a ray/loaded-volume slab test. This permits rays from above
the world's maximum Y to enter the volume. Traversal retains integer cell
ownership and advances one crossed axis at a time. Crossing distances come from
absolute grid planes; fused point evaluation and conservative tie handling avoid
skipping short visible intervals at block edges. No forward epsilon is needed,
including for negative rays and kilometre-long CSM rays.

## Layered visibility and water

The 64-byte `Hit` / Rust `Visibility` record is:

| Offset | Data |
|---|---|
| 0 | Opaque normal XYZ, opaque ray distance |
| 16 | Opaque UV, material ID, accepted-hit / mesh / alpha bitfield |
| 32 | First water-interface normal XYZ, interface ray distance |
| 48 | Water path length, water valid, camera-underwater flag, traversal iterations |

Water is not a terminating hit. Each crossed water voxel contributes the exact
ray segment length, including disjoint intervals. The first opaque hit supplies
the color behind the water. When starting underwater, the first exit supplies
the visible interface. The integrated path still includes water before that exit.

Shading uses channel-dependent `exp(-extinction * length * density)` transmission,
adds a colored in-scattering approximation, and blends analytic sky reflection
with Schlick Fresnel weighting. A directional sun adds a narrow specular highlight.
A separate HDR water pass now performs screen-space terrain/mesh reflections and
precomputed wave bump, with analytic sky fallback. There are no refracted rays. The water debug view
shows integrated path length, not the full distance to the seabed.

## Sunlight

The direction comes from ImGui azimuth/elevation controls. Opaque shading combines
cool sky ambient with warm Lambert sunlight. Each sun-facing surface starts a
shadow ray slightly outside the face to avoid self-intersection. The same
hierarchy accelerates this ray, using opaque masks and ignoring camera culling.
One off-screen blocker is enough to shadow an on-screen receiver; the GPU smoke
test explicitly exercises that case and then replaces the blocker with water.
Shadow queries stop at the loaded-window boundary or the selected view radius, so unseen geometry
outside that volume does not cast shadows. Water absorbs the viewing path but
does not yet attenuate sunlight along its own light-transport path.

## Screen-space ambient occlusion

The visibility buffer makes SSAO straightforward, but ray depth is **not** the
camera's forward-axis Z. For pixel `(x,y)` reconstruct its normalized camera ray
and use `worldPosition = eye + ray * rayDistance`. Project sample positions using
camera right/up/forward dot products and the same aspect ratio / FOV as visibility.
Compare reconstructed neighbor forward depth against the projected sample depth.

A 12-point, per-pixel rotated hemisphere kernel uses metre-scale radius and a small
bias. Range attenuation rejects distant geometry. Invalid / off-screen samples
contribute no occlusion. A 3×3 normal/plane-distance bilateral filter in shading
reduces noise without smearing occlusion across different faces. Only the ambient
term is multiplied by AO; direct sun brightness is preserved.

Screen-space limitations remain: hidden surfaces cannot occlude, edges lose
samples, thin geometry can produce halos and there is no temporal stabilization.
A voxel ray-traced AO experiment could query the actual world with the same
hemisphere directions, at higher traversal cost but without screen-space omissions.

## Hardware ray tracing and hybrid extension

Triangles are not required for Vulkan hardware-assisted traversal. Two natural
representations are exposed-face triangle BLASes, or procedural AABBs whose exact
intersections are resolved by voxel traversal. BLAS granularity and TLAS instance
count are independent of triangle count: one BLAS per chunk can be one instance,
with many triangles or section AABBs inside it.

The implemented hybrid extension keeps software voxel traversal and uses triangle
ray queries for fish/foliage primary hits and sunlight shadows. It has actual mesh
BLAS/TLAS build/refit timing. Voxel-section procedural BLASes remain future work.
See [the hybrid notes](HYBRID.md) for current data flow and measurements.
Start BLAS measurements at **one 16³ subchunk per BLAS** as the smallest initial
candidate; compare grouping 2, 4 and all 24 vertical subchunks. Keep ray budgets,
loaded world and edit traces identical. Do not assume a winner from box counts.

Measure resident BLAS/TLAS memory, scratch memory, initial build time, edit/update
latency (including p95), streaming TLAS cost and secondary traversal time. Keeping
fixed procedural bounds can let edits update voxel/occupancy data without changing
bounds. Smaller BLASes trade local rebuild scope against more TLAS instances and
allocation/build overhead. The voxel-section granularity variants are future experiments; the current
triangle scene is not evidence for their optimal grouping.

Inside a compute ray-query candidate, resolve
only that candidate's interval, then report an actual block hit distance through
`rayQueryGenerateIntersectionEXT`. Candidates are conservative and unordered; do
not treat the first AABB candidate as the nearest block hit. Keep nearest-hit
handling and water intervals correct when candidates overlap or arrive out of order.

The compute ray-query path can retain the layered visibility and deferred shading
interface. Alternatively, retain software primary traversal and experiment with
hardware secondary rays. Triangle primary/shadow queries are implemented;
procedural voxel ray queries and reflected terrain rays are not. Compute fallback
for triangles keeps the example usable on software Vulkan.

Primary references:
- [Vulkan ray traversal](https://docs.vulkan.org/spec/latest/chapters/raytraversal.html)
- [Khronos ray tracing guide](https://docs.vulkan.org/guide/latest/extensions/ray_tracing.html)
- [GLSL ray-query API](https://github.khronos.org/Vulkan-Site/glslext/latest/glslext/ext/GLSL_EXT_ray_query.html)

## Native-resolution presentation and swimming

Interactive target sizes come from the acquired swapchain extent, not logical
window coordinates. Resizing waits for old work, allocates replacement visibility,
AO, mesh-hit, mesh-shadow, HDR scene-color and output buffers, updates descriptors 1/7/10/11/14/2,
and then recomputes camera aspect
and compute dispatch dimensions. World data and shader pipelines are retained.
A direct buffer-to-swapchain copy replaces the former intermediate image/blit.
Odd extents use ceil-divided workgroups and the existing shader bounds checks.
Zero extents defer work; invalid/unsupported sizes fail before replacing targets.

Swimming uses intersection of the body AABB with water cells. Eye-only immersion
previously switched gravity back on while the torso and feet were submerged,
preventing ascent out of the water. Space maintains submerged ascent and supplies
a surface jump when the head clears the water, sufficient for a one-block bank.
Movement still resolves collisions in substeps, including ceilings and bank walls.

## Runtime view radius

The default radius is 8 chunks (128 m), with a maximum of 64 (1,024 m). Material IDs are packed four bytes per storage
word; the CPU staging window uses the same one-byte-per-voxel layout.

`Params.up.w` carries the range in metres (`view_chunks * 16`) without changing
uniform layout. Voxel and polygon primary/shadow rays, miss depths, far frustum
plane, fog and depth debug normalization share it. Voxel storage, section masks
and visible-section buffers resize with the bounded near window, retaining the
fixed 16³ section format. One guard chunk covers camera movement. Distant terrain
uses sparse voxel mip pages; the near and far workers operate independently.
See [Terrain LOD](TERRAIN_LOD.md) and STREAMING.md.

## Primary occlusion

Frustum tests reject geometry outside the camera pyramid; they cannot reject
occluded sections behind the front block. Voxel DDA already stops at its first
opaque block. Running this first now bounds both software polygon AABB/triangle
tests and Vulkan ray queries to that opaque depth. Only accepted foreground
polygon pixels need the resolve pass, and only water-covered ones retrace.
The close-up smoke fixture covers the screen with one block and confirms at most
two DDA steps (air then block) per pixel and zero hidden polygon hits, on both
backends. Shadows keep complete loaded occupancy/AS data for off-screen casters.
The AO hemisphere is precomputed; each pixel rotates it once, avoiding twelve
trigonometric evaluations per pixel without changing its sample distribution.

## Water HDR composition

`shade.wgsl` writes `vec4<f32>` scene color to buffer 15, descriptor binding 14.
`water.wgsl` then reads that completed buffer and writes packed display output.
Bindings 15/16 provide the mipmapped 64² wave texture and sampler. A compute
barrier separates the passes; SSR never reads its own output. `Params` is now
464 bytes, including water optics, scene bounds and conservative mesh-distance
rejection metadata. Both WGSL and
hardware-query GLSL share the extended layout. A separate water timestamp is
reported; the shade total includes opaque lighting plus water composition.
See WATER.md for algorithms and filtering details.


## Full-resolution shading fast paths

CPU-only section bounds tightly enclose the occupied voxels, instead of always
testing the entire 16³ cube. They are prepared with occupancy on the upload worker
and move with the matching GPU world generation. GPU occupancy keeps its original
16-byte section layout. Exact occupied Y limits bound voxel traversal, with an
air-cell margin for water/air exits. Sunlight above the highest voxel can skip
voxel shadow traversal. Off-screen geometry remains available to all shadow and
optical secondary rays.

Enabled mesh AABBs and candidate triangle AABBs provide a conservative primary
frustum test. When neither voxels nor triangles intersect the camera frustum, a
sky-only compute pipeline emits the same sky, sun disc, crosshair and debug colors.
It bypasses geometry, AO, shadow and optical passes. Headless mode also initializes
visibility/AO/mesh buffers so the optimization can be compared with ordinary
rendering. A coarse section false positive is safe: it keeps the ordinary passes.

Otherwise, primary rays first reject against mesh bounds and a conservative
camera-to-mesh minimum distance before starting a ray query. Shadow rays do not
use this camera-distance rejection. The primary/resolve dispatches are skipped
when no enabled triangle can appear in the primary frustum; mesh shadows remain
active for off-screen casters.

Binding **17** is a GPU-only auxiliary `u32` buffer with two screen-sized regions:
mesh write epochs, then compact opaque/water validity flags. This adds eight bytes
per pixel and raises the storage-descriptor requirement from 13 to 14. Uniform
ABI is 464 bytes in both WGSL and hardware-query GLSL. `scene_min.xyz` and
`scene_max.xyz` contain enabled-mesh bounds; `scene_min.w` is the conservative
minimum camera distance, and `scene_max.w` is the highest occupied voxel boundary.
The original origin/dimensions padding carries the bounded voxel Y interval.

Interactive mesh passes write only actual hits. The resolve pass checks the
contiguous epoch array before reading a 48-byte mesh record. Epochs are cleared
on allocation, resize and counter wrap; this prevents stale hits from becoming
visible. Headless QA normally writes explicit misses, and separately tests the
interactive sparse behavior for image parity and stale-hit rejection.

Visibility and mesh resolve maintain compact validity flags. AO and mesh-shadow
passes can reject empty pixels without loading 64-byte visibility records.
Opaque lighting similarly avoids those records for ordinary sky pixels; water
composition directly packs opaque pixels without loading water data or evaluating
optics. Full debug views retain their visibility data. The AO bilateral filter
returns one immediately when every neighboring AO value is exactly one, which is
mathematically identical to filtering those values. No sample counts, shading
resolution, texture filtering, shadows or water effects are reduced.

See PERFORMANCE.md for native-resolution before/after timings and verification.


## Hierarchical lighting acceleration

See [LIGHTING_ACCELERATION.md](LIGHTING_ACCELERATION.md) for the packed linear
min/max depth pyramid, tiled four-level reduction, conservative directional
shadow cascades and exact-ray fallback. Binding 18 is the resolution-dependent
pyramid; binding 19 is the persistent 512 KiB cascade buffer. Both keep all
interactive rendering on the GPU. Seventeen storage-buffer descriptors are now
required. The 464-byte uniform includes acceleration flags and the light-space
basis/cascade center; push constants provide a 16-byte dispatch selector.


Binding **20** now holds actual distant-shadow depths, separate from conservative
caster bounds at 19. Exact contact rays blend to 3×3-PCF cascade lookup over the
last eight metres inside the configurable 48 m radius. Outside that radius,
voxel, polygon and water shading issue no per-pixel light rays. See
LIGHTING_ACCELERATION.md for caching, texel snapping and quality tradeoffs.
The uniform shadow-control vector extends the ABI to 464 bytes; twenty storage
bindings are required. Both cascade buffers together occupy 8.5 MiB.

Near-tied voxel intersections now resolve exposed unit-cell faces rather than
retaining the DDA's previous grid normal. AO rejects sampled geometry outside the
receiver hemisphere. This removes false self-occlusion and edge flicker without
turning off corner darkening or the depth pyramid.

Current soft-shadow passes and the double-buffered atmospheric sky are described in
[SOFT_SHADOWS_AND_SKY.md](SOFT_SHADOWS_AND_SKY.md).
