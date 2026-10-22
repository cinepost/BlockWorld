# Native-resolution shading and culling

The reported low frame rate was reproducible at full-screen **2560×1440** on the
RTX 2060 SUPER. Resolution scaling and effect reductions are not used by this fix.
The baseline was saved before the rendering changes, with only repeatable camera
and fullscreen CLI options added.

## Causes and changes

- An empty camera view still dispatched full-screen mesh primary/resolve, AO,
  mesh-shadow, opaque-lighting and water-composition kernels. Conservative
  occupied-section and triangle-frustum tests now select a dedicated sky pipeline
  when they can prove that no primary geometry exists. A safe false positive
  keeps the ordinary pipeline.
- A section's entire 16³ cube was used as its frustum bound, even when its upper
  portion was air. CPU-only bounds now enclose its actual voxel occupancy. Mesh
  candidate tests refine object bounds with triangle bounds. These metadata stay
  with the corresponding asynchronously uploaded generation.
- Even rejected mesh rays wrote 48-byte miss records, and resolve read those
  records again. Interactive misses now write nothing. A contiguous GPU epoch
  array identifies current-frame hits, so stale records cannot be resolved. Mesh
  bounds and a conservative nearest-mesh distance reject impossible primary rays
  before a hardware query; no primary triangles means no primary/resolve dispatch.
- AO, shadow, lighting and water passes loaded large visibility records for
  irrelevant pixels. A compact opaque/water flag region rejects those reads.
  Ordinary dry pixels skip water data and optical evaluation. Screen-space mesh
  reflection queries also skip when no mesh appears in primary visibility.
- The AO bilateral filter computed nonlinear weights even when every neighboring
  AO sample was exactly one. In that case it now returns one directly, with the
  same numerical result.
- Voxel rays walked the world's full 384-block height even above its occupied
  extent. Exact occupied Y bounds now limit traversal, retaining an air-cell
  margin for water exits. Upward sunlight above the highest voxel skips voxel
  traversal. Off-screen shadow casters and optical geometry remain available.

The auxiliary buffer uses eight bytes per pixel, at descriptor binding 17: one
region for mesh epochs and one for opaque/water flags. Epochs clear on allocation,
resize and counter wrap. WGSL and hardware-query GLSL use a shared 464-byte uniform
ABI. With the depth pyramid and shadow cascades, storage-buffer descriptor limits
must support twenty. See [lighting acceleration](LIGHTING_ACCELERATION.md).

## Controlled before/after measurements

Release builds, default 8-chunk radius, fixed seed 42, all rendering effects and
animations enabled, HUD visible, BGM disabled for isolation. Each native full-screen
run rendered 700 frames. Runs were sequential, without compilation or competing
benchmarks. The world/renderer setup is excluded; presentation is included.
GPU means and frame percentiles exclude 60 warmup frames. Average FPS can still
include initial fullscreen/swapchain resize stalls; steady percentiles are given
alongside it. These are local scene measurements, not a guarantee for every view.

| Controlled view | Before FPS | After FPS | GPU before / after | Frame p50 before / after |
|---|---:|---:|---|---|
| Guaranteed-empty sky | 101.8 | 531.3 | 7.425 / 0.386 ms | 9.098 / 1.470 ms |
| Single grass face filling screen | 86.8 | 130.2 | 9.462 / 5.407 ms | 11.220 / 7.060 ms |

The sky fixture places the camera above MAX_Y and points upward. The close-up
fixture places one grass block at Y=100 and the eye 0.05 m above its top. Full
scene meshes remain resident and animated in both tests. The latter specifically
checks that hidden meshes and irrelevant shading do not dominate a near hit.

For the grass fixture, mesh primary/resolve fell from **2.741 to 0.944 ms**, and
combined opaque/water shading from **3.488 to 1.417 ms**. The dry-pixel water pass
fell from **0.839 to 0.222 ms**. SSAO and legitimate shadow work still have costs;
frustum culling must not remove off-screen shadow casters.

Additional final-build native views, without a saved before run:

| Natural view | FPS | Mean GPU | Frame p50 / p95 / p99 |
|---|---:|---:|---|
| Looking up from the normal ground spawn | 199.6 | 2.550 ms | 4.341 / 4.998 / 6.547 ms |
| Existing grass surface close-up | 144.6 | 4.925 ms | 6.533 / 7.267 / 7.612 ms |

## Verification

- Release build, **29 Rust tests**, and Clippy `--all-targets -- -D warnings` pass.
- The controlled grass close-up headless PPM is **byte-for-byte identical** between
  baseline and optimized renderers: zero changed bytes. Headless QA readback does
  not change the GPU-only window presentation path.
- Full RTX smoke passes with Khronos and synchronization validation enabled,
  including sky/debug parity against ordinary passes, initialized headless outputs,
  sparse/full mesh image parity and stale-hit rejection on hardware/software paths.
- Water tests retain the same 3,028 reflected-geometry pixels and verify distant
  snow/trees, off-screen culling independence, underwater Fresnel/TIR/refraction,
  fog, wave replay, and target resizing. Off-screen sun/alpha shadow checks pass.
- Cache/streaming/AS-ring tests cover generation changes and edit replay. Bounds
  and flags are never inferred through CPU image readback.

## Repeat

```sh
target/release/blockworld --fullscreen --sky-view --no-audio --frames 700
target/release/blockworld --fullscreen --ground-sky-view --no-audio --frames 700
target/release/blockworld --fullscreen --block-view --no-audio --frames 700
target/release/blockworld --fullscreen --ground-view --no-audio --frames 700
BLOCKWORLD_VALIDATION=1 BLOCKWORLD_SYNC_VALIDATION=1 target/release/blockworld --smoke-test /tmp/blockworld.ppm
```

`--sky-view` is guaranteed empty; `--ground-sky-view` retains the ordinary spawn
height and looks up. `--ground-view` chooses an existing grass face with air above
it; `--block-view` is the isolated block fixture. The renderer logs the actual
render extent. The HUD reports when the CPU-proven sky-only path is active.

## AO pyramid and conservative cascades

The next change retains full-resolution twelve-tap AO and exact shadow fallbacks.
See [LIGHTING_ACCELERATION.md](LIGHTING_ACCELERATION.md) for algorithms, cache
invalidation, memory and controls.

Final deterministic comparison on the same RTX 2060 SUPER, native 2560×1440,
default 8-chunk view, seed 42, ordinary spawn camera, all visual effects enabled,
HUD visible and audio disabled. Use `--fixed-time 0.25` to hold fish and wave
poses identical; the initial AS snapshot is reused. Each release run renders
700 frames sequentially; GPU means and percentiles omit 60 warmup frames.

| Lighting path | Average FPS | GPU total | AO including mip build | Mesh shadows | Opaque + water shading | Frame p50 |
|---|---:|---:|---:|---:|---:|---:|
| Reference (`--no-depth-pyramid --no-csm`) | 51.1 | 17.481 ms | 8.580 ms | 0.752 ms | 3.475 ms | 19.049 ms |
| Depth pyramid only (`--no-csm`) | 82.1 | 9.863 ms | 1.207 ms | 0.825 ms | 3.605 ms | 11.566 ms |
| Pyramid + conservative cascades | 82.4 | 9.850 ms | 1.212 ms | 0.783 ms | 3.566 ms | 11.435 ms |

The tiled mip build alone was 0.113 ms on the last frame. Its first level is
written in visibility/resolve, avoiding a separate full-screen visibility read.
Voxel/polygon cascades rebuilt 2/1 times over the final fixed-pose run; the last
frame needed no rebuild. These are measured scene results, not universal gains.
The cascades' incremental benefit here is within normal variation; coarse caster
bounds leave uncertain pixels on exact rays. Most improvement comes from AO.

With animation running, both accelerations measured 76.5 FPS / 10.885 ms GPU,
versus 48.4 FPS / 18.103 ms with both disabled. Those runs sample different wave
and fish times, so the fixed-pose comparison above is the stronger evidence.
The single-block close-up gained little: 128.5 FPS / 5.549 ms with both enabled
versus 135.1 FPS / 5.405 ms on the reference. Its original AO already rejects most
taps behind the eye, leaving little work to remove; hierarchy construction still
costs time. Keep the independent controls when profiling other scenes.

Example comparison commands (run sequentially):

```sh
target/release/blockworld --fullscreen --frames 700 --no-audio --fixed-time 0.25 --no-depth-pyramid --no-csm
target/release/blockworld --fullscreen --frames 700 --no-audio --fixed-time 0.25 --no-csm
target/release/blockworld --fullscreen --frames 700 --no-audio --fixed-time 0.25
```


## Distance-limited contact shadows

After the voxel-boundary/AO correction, a fixed-pose maximum-distance mountain
comparison used `--fullscreen --mountain-view --view-chunks 24 --frames 500
--no-audio --fixed-time 0.25` on the RTX 2060 SUPER at 2560×1440. All effects,
including the AO pyramid and conservative nearby-ray rejection, stayed enabled.
GPU means and percentiles exclude 60 warmup frames; runs were sequential.

| Directional shadow path | Average FPS | Total GPU | Mesh shadows | Opaque + water shading | Frame p50 |
|---|---:|---:|---:|---:|---:|
| Exact at all distances (`--no-far-shadows`) | 84.9 | 8.945 ms | 0.734 ms | 1.974 ms | 11.039 ms |
| Exact nearby / mapped beyond 48 m | 87.7 | 8.711 ms | 0.724 ms | 1.749 ms | 10.775 ms |

This is a modest scene-specific gain, with approximate distant shadows rather
than exact image parity. It does not imply a universal speedup. The depth maps
were cached in this fixed-pose run; animated or moving views pay rebuild costs.
Cold-start/presentation outliers are reflected in average FPS; initial AS build
stats retained by the fixed snapshot are not ongoing per-frame AS costs.
Earlier AO/cascade measurements above predate this distance cutoff and the
voxel-normal correction; use `--no-far-shadows` to study that earlier ray policy.
