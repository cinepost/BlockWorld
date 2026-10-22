Current soft disk sampling, temporal filtering, sky caches and mode controls are
documented in [SOFT_SHADOWS_AND_SKY.md](SOFT_SHADOWS_AND_SKY.md).

# Depth pyramids and conservative directional shadow cascades

Both paths run entirely on the GPU. ImGui exposes **AO depth pyramid** and
**Conservative CSM**, enabled by default. Use `--no-depth-pyramid` and `--no-csm`
for the reference paths. The HUD separates pyramid construction and cascade
updates from the existing pass timings, and counts voxel/polygon cache rebuilds.
AO timing includes pyramid construction; visibility timing includes cascade updates.

## AO: linear min/max depth

Binding 18 contains tightly packed mip levels of `vec2<f32>` camera-forward depth.
The first component is the minimum opaque depth; the second is the maximum.
Empty pixels use `(1e30, -1e30)`, so sky never becomes an occluder. Dimensions use
ceil division, including odd-sized and 1×1 targets. Water's opaque background,
alpha-tested foliage and polygon fish all contribute through resolved visibility.

Visibility writes level zero while the hit is already in registers; mesh resolve
updates it only for accepted foreground triangles. A separate full-screen read of
64-byte hit records is unnecessary. The portable `depth_pyramid.comp` kernel
reduces 16×16 source tiles into four mip levels using workgroup shared memory.
A compute barrier separates batches. At 2560×1440 this needs three dispatches.
GLSL is used here because Naga 24 emits a workgroup-array layout decoration
rejected by newer Vulkan validation implementations; ray-query support is not
required by this shader.

AO first bounds the entire rotated kernel against the root minimum. If all taps
must be unoccluding, the pixel returns one. Otherwise, large screen displacements
consult mip 2 or 4, then exact level-zero depth when necessary. A minimum depth
can prove that an entire footprint is unoccluding. Averaged depth cannot make
that guarantee and is never used. Ambiguous samples retain the original exact
hit reconstruction, range weighting and threshold. The bilateral filter remains
at full resolution. This preserves quality rather than switching AO to half
resolution or reducing its twelve samples.

Pyramid memory is approximately `width × height × 8 × 4/3` bytes (37.5 MiB at
2560×1440), with small ceil-edge overhead. It resizes with the render targets.

## CSM: conservative shadow-ray rejection

The conservative rejection path (retained for nearby exact rays) uses binding 19,
which contains four 128×128 light-space cascades for each of two caster
classes: opaque voxels and polygons. The cascades have radii 16, 48, 144 and
`max(432, 1.5 × view_distance_metres)` metres. Their common center moves on a
four-metre grid. The light basis supports arbitrary directional-light azimuth
and elevation, so the same mechanism can serve a future sun/moon cycle; the game
still exposes one directional light, not two simultaneous celestial lights.

These are **conservative caster-depth bounds**, not a final sampled/PCF shadow
answer. Compute projects occupied 4³ voxel bricks and polygon object AABBs into
every touched texel, atomically keeping the greatest possible depth toward the
light. Closed bounds expand slightly at raster edges. Ordered-float encoding
supports positive and negative light-space depths. The entire loaded world,
including off-screen casters, contributes; camera frustum masks never enter this
build. Water-only bricks do not cast shadows.

A texel whose upper bound is behind the receiver proves the entire light ray
clear, skipping the corresponding voxel or polygon query. An ambiguous texel
retains the exact existing ray, including alpha cutouts. For voxels, an additional
cache test eight metres along the light ray can prove its distant portion clear;
then only the exact local contact-shadow segment is traversed. Receivers outside
all cascades use the reference ray. No cache lookup can directly declare a point
shadowed. This deliberately favors conservative false positives over light leaks,
shadow acne, foliage silhouettes or filtering artifacts.

The two caster classes remain separate, so an uncertain voxel result need not
force a polygon query. Water shading and both hardware/software polygon shadow
backends use the same rejection rule. Changing light direction/intensity, loaded
world revision/origin, cascade center or view coverage invalidates the cache.
Block uploads and asynchronous world adoption increment its world revision.

Polygon maps use AABBs expanded by one metre. CPU metadata checks that every
current object still fits inside its previously rasterized envelope. A new,
removed, or escaping object rebuilds the polygon maps; animation inside an
envelope safely reuses them. This is a containment proof, not a stale shadow
approximation. Switching mesh activation also invalidates the cache. Polygon
queries always use the currently published coherent geometry/AS snapshot.
The maps consume 512 KiB total.

The intentionally coarse bounds can leave many rays unresolved around dense
foliage or near a surface. Cache construction and lookup costs can outweigh the
saved rays in some views. The independent controls and timings are retained for
measurement. A later refinement can store tighter voxel surface/triangle bounds
or guaranteed full-coverage intervals without changing the exact fallback.

## Synchronization and checks

Transfer clears precede cascade atomic writes; compute barriers make completed
cascades visible before consumers. Visibility/resolve precede pyramid generation,
and each mip batch precedes the next. Persistent buffers reuse the render fence;
streamed world and AS ownership remain unchanged. No rendered image or depth
readback is used by the interactive display.

The uniform ABI is 464 bytes in Rust, WGSL and GLSL. Bindings 18/19 raise the
storage-buffer descriptor requirement to twenty. Eighteen 64-bit GPU timestamps
are read for diagnostics; headless QA additionally maps buffers for comparisons.

Smoke tests compare accelerated/reference images and AO on real terrain across
cached and moved cascades, grazing/vertical/downward light directions, moving
fish and both mesh backends. Odd-sized resize tests validate every mip against
CPU min/max reduction, including validity and allocation offsets. Existing tests
cover block edits, off-screen blockers, transparent water, alpha-cutout shadows,
underwater optics, asynchronous world/AS adoption and empty-frame rendering.


## Contact rays nearby, filtered CSM at distance

The **Hybrid** shadow mode is selected by default, with **Exact shadow rays (m)** set
to 48. The last eight metres inside that radius blend exact and mapped shadows;
at/beyond the radius there are no per-pixel directional shadow rays. Set
`--shadow-ray-distance 24` to try a shorter radius, or `--no-far-shadows` to keep
exact rays at every distance. Selecting **Ray Traced** disables mapped distant visibility.
This cutoff applies to voxel, hardware/software polygon, and water lighting.

Binding **20** adds separate 8 MiB first-hit depth maps: four
configurable maps (512² by default) for voxels and four for alpha-tested polygon geometry. It is distinct
from binding 19's upper bounds, which cannot declare an ambiguous point shadowed.
`csm_depth.wgsl` traces from the light toward the entire loaded caster volume;
`mesh_query.comp`'s `SHADOW_CSM` variant uses ray queries for polygon map creation.
The portable backend uses software triangle intersection during map creation.
These are cached GPU map-building rays, not distant per-pixel shadow queries.

Hard shadows sample actual depth maps with 3×3 PCF; soft shadows use a bounded dense tent footprint. Their light-space texel grids are
snapped to world coordinates. Each tap compares stored depth with the receiver
plane evaluated at that texel center, plus a small numerical bias; this avoids
using a large slope bias that would detach shadows. Cascade-edge blending and
the eight-metre near/far blend reduce transitions. Outside loaded cascade
coverage, the mapped result is lit rather than starting a distant fallback ray.

Voxel depths rebuild with world/light/cascade changes. Polygon depths additionally
refresh at most once per 0.25 seconds during animation, and immediately when
bounds escape, objects change, or alpha/hardware modes change. This intentionally
allows slightly older distant animated shadows. Explicit scene replacement and
voxel edits invalidate the maps. Near geometry still queries the current coherent
AS snapshot. The HUD reports both conservative-bound and actual-depth build counts;
the cascade-update timing includes both types of construction.

Mapped distant shadows are approximate: finite-resolution cascades can soften or miss tiny
casters, and animated distant shadows can update in steps. The all-distance ray
reference remains available for comparisons. The Rust/WGSL/GLSL ABI is now 464
bytes, with twenty storage-buffer descriptors and a separate shadow-control
vector. Both cascade buffers together use 8.5 MiB; interactive use never maps them.

## AO edge-flicker correction

A flat-floor regression reproduced false AO even with depth pyramids disabled.
The original DDA forward epsilon could cross two almost-coincident grid planes
and select a voxel past the later crossing while retaining that crossing's
normal. On a continuous floor this sometimes returned a horizontal **internal**
voxel-face normal, rather than the exposed upward floor normal. View changes
then produced narrow, unstable bands of false ambient occlusion.

Opaque hits now resolve their unit-cell entry analytically. At near-tied crossings,
internal solid/solid faces are rejected in favor of the neighboring exposed face.
The hit depth, normal and UV all use that resolved boundary. AO also rejects
sampled surfaces outside the receiver's outward hemisphere, preventing ordinary
pixel-center depth quantization from self-occluding coplanar geometry.

The regression checks all visible floor normals and requires AO exactly one across
grazing pitches, subpixel camera movement, two odd resolutions and both depth
paths. It previously failed with 15% false darkening and, at wrongly oriented
pixels, up to the 0.15 AO clamp. After correction its maximum error is zero.
Floor/wall corner darkening and alpha-cutout occlusion remain covered separately.
