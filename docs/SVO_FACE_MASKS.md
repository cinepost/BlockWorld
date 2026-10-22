> Historical experiment: reverted from the performance reference. The commands
> and mask implementation below belong to the archived regressed branch.

# SVO face-mask experiment

An independent experiment from the VXGI cave leakage fixes. Every SVO node
keeps its existing 8-byte layout: first-child offset, then child-presence byte,
material byte, **face-mask byte**, and a reserved byte. No position data or new
GPU buffer is added. The shared `svo_cone_sample` reader returns the mask in its
fourth component; existing geometry consumers retain their three-component API.
This makes the metadata available to other rendering/injection passes without
changing their behavior in this iteration.

Within the face-mask byte:

| Bit | Meaning |
|---|---|
| 0 | Every face fully occluded |
| 1 | Every face fully unoccluded |
| 2 | −X face fully occluded |
| 3 | +X face fully occluded |
| 4 | −Y face fully occluded |
| 5 | +Y face fully occluded |
| 6 | −Z face fully occluded |
| 7 | +Z face fully occluded |

A side bit means its **entire neighboring face plane** is opaque, not merely
that its representative material is opaque. Partial coverage, cutout leaves and
unknown chunk-border neighbors cannot prove full occlusion. Fully unoccluded
requires all six planes to be known air/water, not merely the absence of closed
bits. Water is transparent under the current GI model. Original full-resolution
source voxels prove the masks for every level, even majority-reduced LODs.
Reduced fixtures without original source data leave their masks unknown.
Chunk-border faces stay unknown until neighbor-aware preparation is implemented.

The SVO preparation cache version is now 4. Masks are computed with the payload
and replaced with it; no separate stale-mask stream exists. Current exact near
voxels always override stale near SVO geometry for GI, and do not trust cached
masks. For SVO hits, the optional shortcut applies only on the corresponding
node boundary: fully occluded/closed-side flags reject source lighting; fully
unoccluded skips the exterior-neighbor query. Other faces still query current
geometry. Neither flag skips visibility testing to sky or sunlight.

## Separate tests and metrics

```
cargo test --release svo_face_mask_flags_are_conservative
cargo test --release svo_face_mask_metrics -- --nocapture --test-threads=1
target/release/blockworld --gi-mask-test --no-audio
```

The CPU test compares the same heterogeneous section with mask generation off
and on. It verifies unchanged child offsets, topology, materials and payload
size. The separate correctness test checks enclosed/isolated voxels, partial
faces, cutout leaves, and unknown borders.

The GPU test places the camera/SVO world outside the dense upload window, so
it exercises stored masks rather than the exact-near fallback. It waits for
289 pages, freezes cone directions, disables temporal accumulation, compares
identical rendered images, then collects 64 samples per mode in paired
OFF/ON/ON/OFF batches. A dedicated timestamp isolates the GI dispatch. The
fixture must contain nonzero indirect illumination.

RTX 2060 SUPER sample, 320×180 headless targets: **611,037 nodes / 4,889,452
payload bytes**, GI median **0.763 ms off / 0.766 ms on** (−0.42% apparent gain).
This is within normal timing variation: **no measurable speedup**. It is not a
gameplay FPS benchmark. The current on-demand GI hits exposed faces, so fully
closed-face rejection is uncommon; exact-near sources do not use cached masks.
Other consumers such as radiance injection or face generation need their own
measurements before claiming a benefit.

Persistent CPU-cache and GPU payload overhead is **0 bytes**, because the byte
was reserved inside the node. A separate byte array would cost 611,037 bytes
for this fixture, before alignment. Peak CPU preparation scratch grows by one
96 KiB full-resolution source copy per active chunk build; masks must be proven
against source data rather than reduced palette IDs. Preparation adds CPU work;
see the recorded isolated timing in [VALIDATION.md](VALIDATION.md).

The runtime shortcut is **off by default**. Enable **GI stored face masks** in
F11 or `--gi-face-masks`; `--no-gi-face-masks` bypasses it. Payload preparation
still writes the shared metadata in either mode, so this switch measures GPU
consumption, while the CPU test separately measures generation cost. The cave
correctness fixes remain active in both modes.
