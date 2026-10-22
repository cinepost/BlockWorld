# Experimental one-bounce SVO GI

Enable **Experimental voxel GI (replaces ambient + AO)** in F11, or launch:

```sh
target/release/blockworld --vxgi --fly
```

The default remains sky SH / approximate ground-bounce ambient and SSAO.
Disabling GI restores that path. Direct sun/moon light, soft shadows, tone
mapping and exposure remain available. GI replaces receiver ambient rather
than adding the same energy twice. Interactive presentation stays on the GPU.

## Performance reference implementation

This restores the transport and reconstruction used before the cave-leakage
experiments. See [PERFORMANCE_REFERENCE.md](PERFORMANCE_REFERENCE.md) for the
checkpoint, measured conditions and known limitations. The mask/sky-visibility
experiments are preserved separately rather than enabled in this reference.

The GI dispatch runs at half width and half height. Each sample casts four
stratified cosine-weighted directions, rotated using the pixel index and frame.
A cone selects SVO nodes using diameter `2 * distance * tan(half_angle)`.
Empty children and homogeneous merged parents accelerate traversal. ImGui
controls cone angle (default 30 degrees) and distance (default 96 m).

The background worker prepares near-ring SVO pages while GI is enabled, then
evicts these GI-only pages when disabled. Resident SVO data takes priority;
the dense voxel window is a fallback when its page is unavailable. A hit uses
material-average albedo, sky SH irradiance and direct sun/moon light with the
existing far-shadow lookup. Unknown pages use sky SH; range/step termination
uses environment radiance. There are no additional per-hit sky visibility rays.

Two history banks store world position, packed normal/material, irradiance and
frame/count. Reprojection validates frame, position, normal and material and
accumulates up to 16 frames. Edits, settings, resource-pack changes and resizing
invalidate history. A surface-aware 3×3 reconstruction upsamples these samples.
Pixels without compatible support use the existing sky ambient approximation;
the full-resolution shading pass does not launch fallback cone traces. Sampling
has no world-space 25 cm hash grid. No camera jitter or TAA is used.

History costs approximately 16 bytes per full-resolution pixel, allocated
lazily. `--no-gi-temporal` disables temporal accumulation for comparisons.

## Accepted limitations

Cave light leakage is a known limitation of this checkpoint: termination and
unknown residency can admit environment light, and source SH lacks local sky
occlusion. Coarse/stale pages may approximate nearby walls. Mixed SVO nodes
use representative opaque materials, without alpha/transmittance mips. Water
has no secondary GI absorption/refraction, and polygonal foliage/NPCs do not
occlude these cones. Four directions plus temporal filtering can still shimmer
when moving. This is one bounce, not a pre-injected radiance volume.

These limitations are intentionally retained while restoring performance. A
future leakage fix should be measured against this checkpoint rather than
silently adding nested visibility rays or full-resolution GI work.

## Validation

`--vxgi-test` checks exact primary visibility, changed indirect lighting, finite
irradiance, temporal accumulation/rejection, settings/edit invalidation, ambient
restoration, near SVO streaming/eviction/re-enable and odd/1px resizing. Headless
readback is used for test assertions only. Cave correctness tests belonging to
the reverted experiment are preserved in its separate source snapshot.
