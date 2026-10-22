# Experimental one-bounce SVO GI

Enable **Experimental voxel GI (replaces ambient + AO)** in F11, or launch:

```sh
target/release/blockworld --vxgi --fly
```

The default remains the existing sky SH / approximate ground-bounce ambient and
SSAO. Disabling the experiment restores that path. Direct sun/moon lighting,
soft shadows, tone mapping and exposure remain available in both modes. GI is
not added on top of the existing receiver ambient, avoiding double counting.

The test dispatches at half width and half height with four stratified,
cosine-weighted hemisphere directions per sample. Their azimuth rotates every
frame. A cone uses diameter `2 * distance * tan(half_angle)` to select the deepest
SVO node whose size fits the footprint. This approximates a solid-angle LOD test;
it does not use the primary camera's projected-pixel criterion. Empty child
cells and complete homogeneous 32/64m parents accelerate the DDA. Cone angle
(default 30 degrees) and maximum distance (default 96m) are adjustable in ImGui.

While enabled, the background worker still prepares SVOs for near chunks, but
exact near voxels take priority for GI queries, including when an old/coarse page
overlaps them. Empty near sections and 4 m occupancy bricks accelerate traversal.
Unknown holes within the residency bounds remain conservative. Rays leaving
the finite rendered domain, or rising above its actual occupied height bounds,
can sample sky after testing all preceding occluders. Near GI-only pages are evicted when disabling the experiment;
re-enabling reoffers them from the preparation cache.

A voxel hit evaluates its material-average albedo, sky SH irradiance and direct
sun/moon illumination with source visibility tests. The entered face must have
a known solid interior and a known air/water exterior. This is the relevant face
of an exposure mask queried from current data. An optional stored mask is also
packed into the existing 8-byte SVO node; it is an independent performance
experiment, off by default. See [SVO_FACE_MASKS.md](SVO_FACE_MASKS.md) for its
bit layout, CPU/GPU A/B tests and memory metrics.
Buried faces and coarse surfaces whose exact interior is empty cannot radiate.
Two stable upward-biased sky probes gate source SH irradiance, and an
explicit ray gates direct sun/moon when shadows are enabled, independently of
the primary shadow mode. The probes are a low-cost occlusion proxy, not full
hemispherical visibility integration. The previous analytic ground-bounce term
is excluded from this source. Environment light requires a visibility ray that
reaches the actual occupied ceiling or the rendered-domain boundary through
known space. It does not require an arbitrary ascent to world height 320.
Range/step exhaustion alone, the lower world boundary and unknown interior
residency do not emit sky. Escaping cones sample sky without a direct solar disk. The result is indirect diffuse lighting;
the original direct-light term is evaluated separately at the receiver.

Two half-resolution history banks store world position, packed normal/material,
linear RGB and a frame/count stamp. Reprojection checks the previous frame,
position, normal and material before accumulating up to 16 samples. Edits,
resource-pack changes, GI settings and target resizing reset history. A
surface-aware 3x3 reconstruction avoids mixing incompatible normals/materials
across full-resolution edges. Each 2×2 tile selects a visible receiver rather
than blindly using its top-left pixel. Thin surfaces without compatible support
receive the same four-cone traced estimate at full resolution, avoiding both
black holes and unoccluded-ambient leakage. History gathers four previous
neighbors with material, normal, tangent-plane, world-distance and frame checks;
its tolerance scales with the projected footprint. Near-exact world-position
matches receive higher weight. Sampling is world-anchored rather than screen-
anchored; moving the camera does not change a fixed-seed surface estimate.
**GI temporal reprojection** / `--no-gi-temporal`
allows comparison without accumulation. No camera jitter or TAA is added.

History takes approximately 16 bytes per full-resolution pixel (31.6 MiB at
1920x1080). It is allocated lazily when enabling the experiment; a disabled
initial renderer uses a dummy binding. Normal interactive rendering remains
entirely GPU-to-swapchain with no image readback.

## Current limits

This is an on-demand one-bounce cone experiment, not a pre-injected radiance
volume or recursive multi-bounce solution. Mixed nodes retain representative
material IDs and do not carry opacity/coverage mips: treating a selected mixed
node as an opaque sample can over-occlude sparse geometry and lose thin detail.
Four directions plus temporal averaging can still shimmer during motion or
light changes. Unknown interior residency can under-light surfaces. Two sky
probes cannot resolve every hemispherical opening. Beyond the finite rendered
domain, unknown geometry cannot cast GI occlusion. Coarse data outside the near volume still approximate
geometry, although source face checks reject known buried/empty faces. Polygonal NPCs/foliage do
not occlude these secondary cones, and water has no GI absorption/refraction.
Alpha-tested voxel foliage now uses animated per-face mean texture alpha for
light throughput and weighted source contribution instead of opaque cubes.
This is a density approximation, not an opacity SVO mip hierarchy or per-leaf
triangle intersection. Water secondary-hit shading still
uses the original ambient approximation.
These are explicit limitations to assess before adding radiance/opacity caches.

## Validation

`--gi-sky-test` checks exposed receiver sky, a sky-lit secondary bounce,
transmitting canopy leaves, fixed-point lighting at 8/5/4/3/2/1 m and a moving
8-to-1 m temporal sequence with both fixed and rotating cone samples. It is
also included in `--vxgi-test`.

`--vxgi-test` checks changed indirect lighting with exact primary visibility,
finite nonnegative irradiance, stationary temporal accumulation, rejection after
settings changes/edits/disocclusion, temporal bypass, original ambient restoration,
near SVO streaming and re-enable, eviction, and odd/1px resize. Sealed one-voxel
cave walls spanning chunk/section boundaries must have zero GI with cone ranges
1/24/96 m; source sunlight is checked with CSM on/off, stale near SVO pages must
not replace exact data, and opening the roof must admit real skylight. Headless QA reads
buffers only to make assertions; gameplay does not read rendered images back.


On the RTX 2060 SUPER, a 600-frame fixed-camera comparison at 1280×720,
with meshes disabled and animation frozen, measured 284.5 FPS / 2.753 ms mean
GPU frame for ambient/SSAO and 161.1 FPS / 5.205 ms for GI. Both use the same
sun, camera, world and presentation path. GI loaded all 289 near pages (2.14 MiB
payload); the mean excludes the first 60 frames. These are one camera's timings,
not an FPS guarantee for moving cameras or the full animated scene.
