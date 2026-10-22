# Verification — 2026-10-20

Built and exercised on Linux x86-64 with Rust 1.99.1.

- `cargo build --release --locked`: passed; executable `target/release/blockworld`.
- `cargo test --locked`: **twenty-two tests passed**: block layout, negative coordinates,
  generation, ray selection/range, collision/placement overlap, sparse upload
  layout, conservative frustum bounds and water/solid occupancy updates, surface swimming, exiting onto a one-block bank,
  swimming under a solid ceiling, weighted mesh skinning/topology and material flags. New regression tests cover
  exact decoration geometry/poses across positive/negative chunk crossings and
  view-window changes, both Ctrl run keys, and auto-jump disabled/enabled,
  two-block walls and overhead ceilings.
- `cargo clippy --all-targets -- -D warnings`: passed.
- Headless Vulkan smoke test with Khronos validation on Mesa llvmpipe: passed.
  Actual compute results verify:
  - Hierarchical + culled traversal matches exhaustive traversal for every initial
    pixel's opaque material/depth and water path; **56.6% fewer primary steps**.
  - **75 of 422** occupied sections intersect the initial camera frustum.
  - Water retains the opaque background, one-metre thickness, correct interface
    depth and underwater exit; the transparency switch changes the rendered pixel.
  - Off-screen solid blocks cast sun shadows with camera culling enabled. Water
    replacements do not. Disabling shadows restores unoccluded sun visibility.
  - SSAO darkens a floor/wall corner, stays finite/in range and becomes exactly
    one when disabled. Minimum raw AO in that fixture is 0.150 (the clamp).
  - Rays above world maximum Y enter the loaded volume and find a block.
  - Block deletion/placement reaches the GPU; all ten debug views dispatch.
  - Visibility, output and AO buffers resize through 321×239, 800×450 and 1×1,
    then restore to 960×540; center-hit material and buffer lengths stay correct.
    Zero-size requests retain targets; impossible extents are rejected.
- Forced unsupported-feature path with `BLOCKWORLD_DISABLE_RAY_QUERY=1` also
  passed the full smoke suite on llvmpipe: no AS allocation/pipelines, software
  triangle visibility/shadows including alpha holes. Ray-query/refit comparisons
  are explicitly reported unavailable in that path.
- Actual swapchain/ImGui output captured and visually inspected: `preview.png`.

## Airborne momentum, water SSR and filtered waves

The final release build, 22 tests and Clippy with warnings denied pass. New CPU
checks cover full/partial WASD and Ctrl release while airborne, preserved diagonal
velocity, bounded airborne input speed, stopping after landing, deterministic precomputed waves and all seven
mip levels. Existing auto-jump, wall/ceiling collision and shore-exit tests pass.

The full Vulkan suite passes synchronization validation on RTX 2060 SUPER and
llvmpipe, including the forced no-ray-query path. The 161×97 pool/wall fixture
produces **2396 reflected-wall pixels** above its confidence threshold. It verifies
SSR color changes, missing-geometry sky fallback, underwater rejection, scrolling
and exact wave replay, unmodified visibility/water path lengths, flat normals with
waves disabled and HDR/depth resizing. Both tested devices use **8× anisotropy**.
Twelve debug views dispatch successfully. The 480×270 fixture exports were visually
inspected; `water-preview.png` is the actual shaded GPU result.

See WATER.md for the isolated lake-view benchmark: **125 FPS** with SSR/waves,
**171 FPS** with waves only, and **173 FPS** for flat water at native 1280×720 on
RTX 2060 SUPER. The new water composition pass averages **2.513 ms** with SSR
versus about **0.38 ms** without SSR. These are water-heavy stationary views.

## Asynchronous streaming and primary occlusion

Final release build and Clippy with warnings denied pass. The 19 unit tests add
chunk→section frustum parity against brute-force section tests, actual LRU order,
pinned working-set retention, empty-region eviction, edit replay and immutable
chunk sharing without section copies.

The final smoke suite passes with both normal and **synchronization validation**
on RTX 2060 SUPER, llvmpipe, and llvmpipe with ray-query features disabled. It
adds a one-block close-up fixture (at most two DDA steps per primary ray, no
hidden polygon hits), and real worker streaming through multiple windows with
an intentionally undersized cache. It verifies eviction/reload edit state,
three-slot wraparound, old-snapshot reuse, BLAS refit/rebuild, and visible fish
geometry/depth parity between completed AS slots and software intersections.
Independent BLAS storage/scratch arena slices and batched builds also pass
synchronization validation. The final native `--stream-test --resize-test --frames
600` run also passed synchronization validation through all four resize transitions. A swapchain acquire/layout barrier issue found by
the stricter native check was corrected.

An isolated **1280×720 RTX** run, validation disabled, `--stream-test --frames
1000`, turns continuously, moves every 120 frames and alternates view radius
between 8 and 2 chunks under an 8 MiB CPU cache budget. It completed 9/9 world
requests and 769 mesh snapshot swaps:

| Metric | Result |
|---|---:|
| Average FPS, including initial asynchronous preparation | 181.4 |
| After-warmup frame p50 / p95 / p99 | 5.279 / 7.179 / 9.420 ms |
| After-warmup maximum frame | 12.455 ms |
| Maximum main-thread streaming/adoption call | 0.251 ms |
| Final cached / pinned CPU chunk payload | 7.98 / 7.53 MiB |
| Final cached chunks | 384 |

Warmup excludes the first 60 frames. A driver submit/present call reached
53.607 ms during initial preparation, so these results do **not** establish
zero-stall behavior at startup or when allocating a new larger range. No other
benchmark or software Vulkan workload ran concurrently with this measurement.
Pooling BLAS memory, bounded build batches and reusing retired terrain/staging
allocations removed recurring large submission hitches in this run. Graphics
render frames still have one completion fence; the AS ring is independent.
See STREAMING.md for exact ownership and budget limits.

## Stable decorations, controls and variable view distance

Release build, 16 unit tests and Clippy with warnings denied pass. Regression
geometry comparisons include fish and foliage, negative/positive chunk crossings,
the edge of an upload window, and changes in window size. Auto-jump is tested
against one-block steps, disabled mode, two-block walls and a low ceiling. Both
Ctrl keys produce 7 m/s running versus 4.3 m/s walking.

The Vulkan smoke suite passes on NVIDIA RTX 2060 SUPER and llvmpipe with Khronos
validation enabled. Underwater cameras retain fish material/depth and water-path
information with software triangles and hardware ray queries. Repeated window
changes **1 → maximum → 2 → maximum → 1** verify voxel, occupancy and visibility
buffer resizing; both triangle backends and voxel rays reject geometry beyond
the selected range. Maximum is 8 chunks on RTX, 7 on llvmpipe due to its storage
buffer limit. The forced no-ray-query path also passed. Native window resizing
through 800×600, 1440×810, 480×640 and 1280×720 passed with validation enabled.

At **8 chunks / 128 m**, a native **1280×720** RTX run (`--view-chunks 8 --frames
600`, validation disabled) averaged **181.2 FPS**, with 30 fish, 1004 plants,
4976 triangles and 336 objects. Mean GPU time after 60 warmup frames was
**3.393 ms**: BLAS 0.051, TLAS 0.021, polygon primary 0.240, polygon shadows 0.232,
voxel visibility 0.668, SSAO 1.585 and shading 0.505 ms. Requested AS buffers and
scratch totaled 2,197,632 bytes. This is a stationary-camera measurement; dense
world uploads and AS reconstruction can still cause a pause at chunk crossings
or when changing the slider. These timings do not replace a streaming benchmark.

## Previous hybrid triangle benchmark (before stable placement / range slider)

On NVIDIA GeForce RTX 2060 SUPER, native **1280×720**, IMMEDIATE presentation,
Khronos validation enabled, 1000 frames per run. The former globally capped scene had **12 skeletal fish,
160 grass/fern plants, 1024 triangles and 28 BLAS/TLAS instances** (12 fish and
16 static foliage section groups). Stage means exclude the first 60 frames:

| Mode | Average FPS | Mean GPU BLAS | TLAS | Mesh primary rays | Mesh shadow rays | Mean GPU total |
|---|---:|---:|---:|---:|---:|---:|
| Hybrid, fish BLAS refits | **189.3** | 0.041 ms | 0.018 ms | 0.229 ms | 0.382 ms | 3.079 ms |
| `--no-meshes` voxel baseline | 229.1 | 0 | 0 | 0 | 0 | 2.552 ms |
| `--no-rt` software triangle traversal | 154.9 | 0 | 0 | 1.216 ms | 0.918 ms | 4.568 ms |
| `--rebuild-blas` every-frame fish builds | 181.3 | 0.116 ms | 0.018 ms | 0.230 ms | 0.320 ms | 3.134 ms |

Hybrid mean CPU skinning was 0.054 ms and upload 0.048 ms. Initial scene AS setup
on CPU was 24.870 ms; first GPU BLAS+TLAS build was 0.223 ms. Requested AS input,
storage and scratch buffers totaled **249472 bytes (243.6 KiB)**, excluding the
renderer attribute buffers and driver-private overhead. Runtime-disabled modes
retain allocations, but skip steady AS updates; `--no-meshes` skips mesh ray passes.

The hybrid run had approximately 17.4% lower FPS than its voxel-only comparison.
These short live-animation runs do not sample identical poses at identical frames;
clock speeds and desktop activity also vary. Use `--freeze-animation` for an
identical-pose comparison. Small fish do not establish large NPC update costs.

Both RTX ray queries and software triangles passed headless integration checks:
fixed replay poses agree in material/depth (up to eight permitted silhouette-edge
pixels); animation changes actual hits; rebuild/refit agree; water clips at fish;
foliage holes reveal the background and transmit sunlight. Accepted foliage carries
mesh/alpha visibility flags. The ordinary SSAO pass uses that combined depth.

The hybrid desktop resize test also passed 800×600 → 1440×810 → 480×640 →
1280×720, covering the new mesh-hit/shadow buffers. No Vulkan validation errors
were reported. [Raw final runs](benchmark.txt) include means and initial-build data.

## Previous voxel-only native-resolution rendering

Interactive targets now match the swapchain pixel extent. A release desktop run
with Khronos validation successfully resized through **800×600 → 1440×810 →
480×640 → 1280×720**. Each transition recreated render buffers and updated their
descriptors; no Vulkan validation errors were reported. The output copies directly
to the acquired swapchain image, with no intermediate image or scaling blit.

On NVIDIA GeForce RTX 2060 SUPER, all effects enabled, native **1280×720**,
IMMEDIATE presentation: **1000 frames in 4.266 s, 234.4 FPS average**. Last-frame
GPU timestamps were visibility 0.520 ms, SSAO 1.691 ms, shading 0.421 ms and total
3.183 ms. These are individual last-frame measurements, not stage averages.
Native 1280×720 processes 77.8% more scene pixels than the earlier 960×540 mode.

## Earlier fixed-resolution measurements

NVIDIA GeForce RTX 2060 SUPER, internal scene **960×540**, window **1280×720**,
IMMEDIATE presentation, Khronos validation enabled, 1000 frames per run:

| Configuration | Run duration | Average FPS | Last GPU visibility | SSAO | Shading | GPU total |
|---|---:|---:|---:|---:|---:|---:|
| All effects, hierarchy enabled | 3.029 s | **330.1** | 0.303 ms | 0.514 ms | 0.525 ms | 1.418 ms |
| `--no-hierarchy` | 4.204 s | 237.9 | 0.443 ms | 0.514 ms | 1.108 ms | 2.141 ms |
| `--no-ssao` | 2.133 s | 468.9 | 0.303 ms | 0.093 ms | 0.187 ms | 0.658 ms |

GPU timings are individual last-frame timestamp measurements, not run averages.
The disabled SSAO pass still writes an all-one buffer, so it has a small nonzero
cost. Disabling SSAO also skips its bilateral filtering in shading. Disabling
hierarchy affects both primary visibility and sunlight shadow rays.

No Vulkan validation errors were reported during these runs. The older initial
renderer (opaque water, simple Lambert, no SSAO/shadow rays) measured 545.5 FPS;
that is a different feature set, not the current renderer's performance.

These runs use a stationary initial-spawn camera, exclude initialization and do
not measure exploration/upload hitches or performance on other hardware. IMMEDIATE
mode can tear. Interactive frame data stays on the GPU; headless verification
intentionally reads output/visibility/AO and saves a PPM.

Reproduce locally:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
BLOCKWORLD_VALIDATION=1 target/release/blockworld --smoke-test /tmp/blockworld.ppm
BLOCKWORLD_VALIDATION=1 target/release/blockworld --frames 360 --resize-test
BLOCKWORLD_VALIDATION=1 target/release/blockworld --frames 1000
BLOCKWORLD_VALIDATION=1 target/release/blockworld --frames 1000 --no-meshes
BLOCKWORLD_VALIDATION=1 target/release/blockworld --frames 1000 --no-rt
BLOCKWORLD_VALIDATION=1 target/release/blockworld --frames 1000 --rebuild-blas
BLOCKWORLD_VALIDATION=1 target/release/blockworld --frames 1000 --no-hierarchy
BLOCKWORLD_VALIDATION=1 target/release/blockworld --frames 1000 --no-ssao
```


## 2026-10-20: extended range, terrain and tree collections

Release build, 25 unit tests, and Clippy with `--all-targets -- -D warnings` pass.
Tree tests include oak/pine removal across negative chunk **and region** boundaries,
replaced-member independence, cache eviction/regeneration, worker edit replay,
non-overlapping collection membership, deterministic generation and snowy terrain.
Physics and decoration tests now use controlled fixtures independent of terrain.

The RTX 2060 SUPER Vulkan smoke suite passes with both Khronos and synchronization
validation enabled. It covers byte-packed voxel IDs across word boundaries,
16×16 atlas access, snow/oak/pine materials, range transitions
1→24→2→24→1, mesh hardware/software parity, alpha holes, sunlight/SSAO, water
SSR/bump, target resizing, close-up traversal and asynchronous edit/eviction/AS
rings. The complete suite also passes with `BLOCKWORLD_DISABLE_RAY_QUERY=1`.
No validation errors were emitted. The mountain inspection camera also renders
successfully with synchronization validation enabled.

Isolated native 1280×720 runs, RTX 2060 SUPER, all effects enabled, HUD visible,
1,000 frames each. Presentation is included; initial world/renderer creation is
excluded. Frame percentiles exclude 60 warmup frames. These are local scene
measurements, not guarantees for every position or other hardware.

| View radius | Average FPS | p50 / p95 / p99 frame ms | Max after warmup | GPU mean | Resident chunks | Fish / foliage | Mesh triangles |
|---|---:|---|---:|---:|---:|---|---:|
| 8 chunks / 128 m | 145.6 | 6.700 / 8.085 / 8.923 | 13.849 ms | 5.045 ms | 361 | 42 / 850 | 4,744 |
| 24 chunks / 384 m | 123.6 | 8.053 / 9.171 / 11.182 | 24.511 ms | 6.158 ms | 2,601 | 490 / 3,533 | 29,812 |

At 24 chunks the active AS slot uses 10,353,408 bytes; voxel storage alone is
243.84 MiB per window. Cached CPU chunk/tree payload is approximately 55.67 MiB.
The expanded maximum remains subject to device storage-buffer limits; devices
with smaller per-buffer limits expose a smaller slider maximum.

`BLOCKWORLD_VALIDATION=1 BLOCKWORLD_SYNC_VALIDATION=1 target/release/blockworld
--stream-test --frames 1000` also completed without validation errors. It alternated
24/2 chunks, turned continuously, moved every 120 frames and used an 8 MiB cache
budget. Nine requests were issued: four adopted, four became obsolete and one
remained pending at exit. Maximum main-thread world synchronization was 0.190 ms.
The final cache held 370 chunks / 7.99 MiB, including 0.97 MiB pinned.
Post-warmup frame p50/p95/p99/max was 7.534/10.432/11.965/89.878 ms.
The largest submit/present stage was 83.734 ms: asynchronous preparation avoids
waiting on worker fences, but large driver allocations/submissions can still
produce frame spikes. Rapid movement can obsolete a large replacement before it
is adopted; old complete generations remain usable while requests coalesce.

Use `--mountain-view` for snow inspection, key 7 for snow placement and F11 to
hide/show the developer overlay. See WORLD.md for collection semantics.


## 2026-10-20: optical water, BGM and fly camera

The release executable builds; 27 unit tests and Clippy `--all-targets -- -D
warnings` pass. New unit coverage checks PCM wraparound without inserted samples
and fly movement through solids, zero-input hovering, vertical boost and momentum
reset when switching modes.

The full RTX smoke suite passes with Khronos and synchronization validation:
voxel/mesh visibility and shadows, alpha tests, 24-chunk buffer transitions,
resizing, water/wave effects, and asynchronous world/AS snapshots. Final targeted
water tests and captures also pass with those validation settings enabled.
The water fixture verifies snowy geometry and tree blocks beyond the previous
32 m SSR range, reflection independence from primary frustum culling, submerged
bed reflections, Fresnel critical-angle TIR, near-normal reflectance, equal-IOR
zero reflection, stronger submerged fog and a bed that is not treated as a water
surface. The controlled fixture reports 3,028 pixels with reflected geometry.
See WATER.md and the above-/below-water and Fresnel captures there.

BGM is now a bundled, predecoded Rodio/CPAL source independent of engine threads.
A four-second output test reported 3.97 s PCM supplied, 42.56 ms maximum feed gap,
zero gaps over 116.1 ms and zero backend errors. A native all-effects stress run
`--water-view --stream-test --fly --frames 1500` reported 13.06 s PCM, maximum gap
43.31 ms, no late events and no backend errors while generation, uploads and
24/2-chunk transitions were active. Its video-frame maximum after warmup was
91.016 ms, showing that this render stall did not become a corresponding source
feed gap. Metrics observe scheduling/backend reports, not physical speaker output.

An isolated native RTX 2060 SUPER / 1280×720 final run
`--underwater-view --fly --frames 1000` averaged **122.5 FPS** with BGM, optics,
wave bump, mesh fish/foliage and other default effects enabled. Post-warmup frame
p50/p95/p99/max: 8.037/9.162/11.208/16.299 ms. Mean water composition 3.912 ms;
mean render-queue GPU total 6.358 ms. Audio supplied 8.92 s PCM, maximum feed gap
42.59 ms, zero late events/backend errors. Startup is excluded; presentation is
included, and stage means/percentiles exclude 60 warmup frames. Scene and device
conditions affect results.

Reproduce:

```sh
cargo build --release --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
BLOCKWORLD_VALIDATION=1 BLOCKWORLD_SYNC_VALIDATION=1 target/release/blockworld --water-test
target/release/blockworld --audio-test
target/release/blockworld --water-view --stream-test --fly --frames 1500
target/release/blockworld --underwater-view --fly --frames 1000
```


## 2026-10-20: full-screen shading/culling performance

See PERFORMANCE.md for the diagnosis, exact native-resolution fixtures, before/after
GPU and frame timings, and reproduction commands. At 2560×1440 on RTX2060SUPER,
the controlled sky fixture improved 101.8→531.3 FPS and the isolated grass close-up
86.8→130.2 FPS, retaining all effects. Final natural ground/sky cameras measured
144.6/199.6 FPS respectively. A baseline/final headless grass image comparison
found zero changed bytes. Release build, 29 Rust tests, Clippy and full Khronos +
synchronization-validation smoke pass, including sky/debug parity and sparse-hit
stale-data rejection. Water/underwater and off-screen-shadow regressions pass.

## Depth hierarchy and conservative shadow cascades

Release build, all 29 Rust tests and Clippy with warnings denied pass. Native
RTX 2060 SUPER smoke tests with Khronos and synchronization validation cover:

- Accelerated/reference **exact image parity** on real terrain, at fixed fish and
  wave poses, across two polygon backends and four light directions including
  grazing, overhead and downward light. AO differs by less than 0.00001.
- Cache reuse, camera movement inside/across four-metre cache cells, animated
  fish containment and existing off-screen block/alpha-cutout shadow tests.
- Every min/max mip compared with CPU reduction after odd-sized resizing,
  including one-pixel axes and 1×1; base validity matches resolved visibility.
- Existing edits, water interfaces/reflections, frustum independence, range
  changes, sparse mesh epochs and asynchronous world/AS streaming regressions.

The tiled shader initially exposed a Naga 24 workgroup-array SPIR-V decoration
rejected by newer validation. The portable pyramid kernel now uses GLSL/glslc;
subsequent native validation runs are clean. Interactive display and all depth
hierarchy/cascade consumers remain GPU-only; buffer readback is headless QA.
Fixed-pose performance measurements are recorded in PERFORMANCE.md.

Final feature-disabled run (`BLOCKWORLD_DISABLE_RAY_QUERY=1`) also passes the
complete suite with Khronos and synchronization validation, including the
portable pyramid shader. Final RTX-enabled run passes the same suite cleanly.
A 360-frame windowed streaming stress run at 1280×720 finished with 222.2 FPS,
0.236 ms maximum main-thread world synchronization and four/one voxel/polygon
cascade builds. It adopted one world request and discarded one stale request;
it is not a completed 24-chunk terrain-streaming benchmark. Frame max was 86.008
ms, dominated by an 83.186 ms submit/present outlier, so this run does not establish
an absence of presentation stalls.


## AO edge stability and distant CSM lookup

The screenshot's narrow flickering edge motivated a new flat-floor regression.
Before correction, the unaccelerated path produced 15% false AO; some near-tied
voxel crossings returned horizontal internal-face normals and darkened to the
0.15 AO clamp. The corrected unit-cell/exposed-face resolution and receiver
hemisphere rejection now produce **zero false AO** in all 36 grazing/subpixel
cases at 321×239 and 967×523, and every visible floor normal is upward. Existing
floor/wall AO darkening remains at the 0.15 minimum, rather than being disabled.

New map-only shadow checks force the contact radius to zero, verifying isolated
receivers, off-screen voxel blockers, static cache reuse, edit invalidation,
alpha-tested polygon depths on hardware/software backends and grazing-angle
receiver-plane PCF. A 64 m contact radius matches the exact reference at a nearby
receiver. Empty-scene cache metadata now clears correctly instead of rebuilding
blank polygon maps every frame. Alpha-mode and explicit mesh-scene changes also
invalidate actual depth maps.

Final release build, all 29 unit tests and Clippy with warnings denied pass.
Complete native RTX-enabled and actual feature-disabled Vulkan smoke tests pass
with Khronos and synchronization validation. Both include the new AO and shadow
regressions, all odd-sized mip checks, block edits, water optics and async world/AS
adoption. No validation messages were emitted. The mapped buffer is GPU-only in
interactive mode; only headless QA reads rendered results.

A fixed-pose 24-chunk mountain benchmark at native 2560×1440 on RTX 2060 SUPER
measured 84.9 FPS / 8.945 ms GPU with all-distance rays, versus 87.7 FPS / 8.711 ms
with a 48 m contact cutoff. The mapped distant result is approximate; this is not
an exact-image comparison. See PERFORMANCE.md for conditions and stage timings.

## Soft disk shadows and cached atmosphere (2026-10-20)

Release build, all 31 Rust unit tests, and Clippy with warnings denied pass.
The complete smoke suite passes on RTX 2060 SUPER with Vulkan core and
synchronization validation, both with hardware ray queries enabled and with
`BLOCKWORLD_DISABLE_RAY_QUERY=1`.

The new 161×121 GPU fixture finds 121 classified penumbra pixels and 119 partial
shadows. It verifies finite visibility, temporal accumulation, mode-change
resets, camera-motion rejection, overlapping voxel/polygon casters, and all three
shadow modes. Sky tests verify finite positive radiance, a normalized
solid-angle importance PDF, normalized conditional/marginal CDFs and day/night
rebuilds. Existing flat-plane AO cases retain zero false occlusion, and resize,
water optics, alpha foliage and async streaming regressions pass.

Algorithm details, memory use and representative native-resolution measurements
are in [SOFT_SHADOWS_AND_SKY.md](SOFT_SHADOWS_AND_SKY.md).

### CSM ringing regression

The corrected soft CSM filter passes the full native RTX smoke suite with core
and synchronization validation, exercising WGSL and hardware-query GLSL shadow
paths. Nearby raw shadow visibility stays identical when view distance changes
from 8 to 24 chunks; the fixture contains nontrivial shadows. The soft-shadow
fixture now finds 127 classified edge pixels and 125 partial shadows. Release
build and Clippy with warnings denied pass. The filter uses real local blocker
separation and contiguous tent taps instead of view-distance fallback and sparse
ring taps. Its radius is bounded to four texels, limiting very broad CSM softness.

## Configurable CSM, history continuity and TAA

Release build, 33 unit tests (including Halton bounds/period and jittered
projection round trips), and Clippy with warnings denied pass.
Native RTX core/synchronization validation and the feature-disabled software
backend smoke suite cover CSM sizes 128, 256, 512, 1024, 2048 and back to 512.
They also verify that receiver shadow history survives repeated animated depth
refreshes, TAA history actually contributes to color, explicit scene replacement
rejects old color, and odd-size/1×1 target changes initialize valid history.
Existing AO, water, alpha geometry and async-streaming regressions pass.

The periodic noisy-frame cause was a global history reset on cached mesh-depth
refresh. Camera-grid movement and gradual day-cycle changes likewise no longer
flush receiver history. Streaming cache revisions are separated from explicit
scene replacement; per-surface validation handles ordinary snapshot changes.

Isolated RTX 2060 SUPER, 2560×1440, default view 8 / hybrid / CSM 512,
fixed time 0.25, no audio, 500 frames (60 warmup):

| TAA | FPS | GPU frame mean | Frame p50 / p95 / p99 |
| --- | ---: | ---: | --- |
| On | 65.7 | 12.638 ms | 14.239 / 15.264 / 17.026 ms |
| Off | 70.7 | 11.482 ms | 13.131 / 14.089 / 17.846 ms |

The last TAA dispatch measured 1.247 ms. These are end-to-end measurements;
jitter also changes primary sample locations and conservative culling slightly.

## TAA removal and native loading screen

TAA, primary-ray jitter, its shader/pipeline/UI controls, and the extra
80-byte/pixel color-history allocation are removed. Soft-shadow accumulation and
its history-continuity fixes remain. The shared ABI is back to 464 bytes and
profiling reads eighteen timestamps.

The complete native RTX smoke suite passes with core and synchronization
validation, including the animated shadow-refresh regression, configurable CSM,
flat-plane AO, water optics, foliage and asynchronous streaming. Release build,
31 unit tests and Clippy with warnings denied pass.

The startup path presents the supplied artwork before world preparation begins.
A native window capture verified aspect-preserving image placement and the thin
bar beneath the logo (see [loading-preview.png](loading-preview.png)). A
300-frame windowed validation run passed the loading/swapchain handoff and all
800×600 → 1440×810 → 480×640 → 1280×720 resize transitions, with the initial
streamed polygon/AS snapshot ready before gameplay. The window is hidden during
minimal Vulkan setup so the artwork is its first presented content. Observed
startup presentation was roughly 0.5 seconds from entering minimal setup; no
world generation or game pipeline setup occurs before that frame.

## Resource-pack pipeline (2026-10-21)

Release build, 35 unit tests and Clippy with warnings denied pass. The complete
native RTX smoke suite passes with core/synchronization validation after replacing
fixed texture-buffer reads with atlas-image sampling. Existing AO, CSM, water,
foliage, resize and async-streaming regressions remain passing.

Synthetic Deflate ZIP tests exercise PNG import, exact frame advancement,
interpolated red/blue frames, duplicated RGBA edge/corner padding, linear sampling
at sprite boundaries, imported voxel-side V orientation, leaf alpha holes,
software/ray-query card alpha agreement and generated-texture restoration.
The focused GPU test also passes with both descriptor indexing and hardware ray
queries disabled. Cross-backend alpha agreement is checked away from exact
coplanar/texel-boundary rays, where float rounding can select either texel.

A native startup/resize run loaded a synthetic 11-texture ZIP, including animated
water, through the loading screen. It completed gameplay and the 800×600 resize
with Vulkan synchronization validation and no validation errors. No third-party
pack assets are redistributed by these fixtures.

See [RESOURCE_PACKS.md](RESOURCE_PACKS.md) for supported mappings, animation rules,
16px constraints, bindless/fallback behavior and the deferred blockstate scope.

An isolated 500-frame fullscreen run at 2560×1440 on RTX 2060 SUPER, default
hybrid shadows and nearest texture filtering, loaded the synthetic 11-texture
pack at fixed time 0.25. It measured 74.4 FPS; mean GPU frame 11.678 ms and
frame p50/p95/p99 13.303/14.142/15.912 ms after 60 warmup frames. This is one
end-to-end scene measurement, not a general guarantee for resource packs.

## Terrain LOD and long-ray stall — 2026-10-21

- Release build, Clippy with `-D warnings`, and 40 CPU tests pass.
- Sparse Vulkan fixtures cover all five voxel scales (1/2/4/8/16 m), negative
  page coordinates, 300/700/1,021 m hits, exact depth, steep occupied-slab entry,
  range eviction, and a 5.3 km ray with a bounded traversal-step count.
- A fixed `0.0001` traversal epsilon can round to zero at kilometre-scale CSM
  distances. The initial interactive stress run stalled and was killed. The fix
  uses an ULP-scaled epsilon and clips far rays to resident page bounds.
- Repeating the same 600-frame turning/movement/range/resize stress completed
  in 8.553 s on RTX 2060 SUPER with core and synchronization validation enabled.
  All four requested buffer resizes completed; no validation errors were logged.
- A subsequent 2,200-frame 1280×720 mountain-view run completed in 21.468 s,
  with shadows enabled and all 16,352 distant pages resident. Page distribution
  was `[0, 588, 2493, 9854, 3417]` for 1/2/4/8/16 m cells, using 3.53 MiB of the
  64 MiB payload/table allocation at an effective 8 px target. No validation
  errors occurred. This was a responsiveness/validation run, not an isolated FPS
  benchmark; CPU verification jobs ran concurrently.
- The headless 161×91 progressive-load test completed all 16,352 pages in 8.35 s,
  first page 0.008 s, then verified distant visibility and range eviction. Its
  low framebuffer resolution intentionally selects mostly 16 m cells.

See [TERRAIN_LOD.md](TERRAIN_LOD.md) for design, limits and commands.
- After the precision fix, the complete Vulkan smoke suite passed again,
  including planar AO, CSM, water, AS streaming, rotated foliage/resource-pack
  alpha coverage and sparse LOD tests. The LOD/5.3 km fixtures also passed with
  both bindless textures and hardware ray queries disabled.

## Chunk-local LOD edits — 2026-10-21

The edit journal now maintains independent content versions for affected chunks.
Unrelated edits no longer invalidate the entire worker cache, reject unaffected
queued results, or purge GPU residency. Dirty resident chunks are prioritized;
old GPU pages remain visible until replacement storage and payload are ready.
Allocation pressure postpones replacement instead of deleting the old page.

Release build, Clippy (`-D warnings`) and 42 CPU tests pass. New CPU regressions
cover negative chunk boundaries, journal removal/restoration, multi-chunk changes,
and worker publication after unrelated edits. The native Vulkan `--lod-edit-test`
kept 152 pages resident: a near edit uploaded zero pages; placement and deletion
in a distant chunk each replaced exactly one page and changed the rendered hit.
Core and synchronization validation reported no errors.

## Linear filtering and voxel-edge coverage — 2026-10-21

Linear atlas sampling is now enabled by default, including initial texture
metadata; `--texture-filter nearest` and the HUD retain the alternative. The
Vulkan sampler already had linear min/mag filters, but the previous default
shader path bypassed it with `textureLoad`.

The new `--voxel-edge-test` reproduced 18 missed hits in 96 positive-coverage
rays with the previous epsilon traversal. Forward sampling could cross a short
visible interval at a cube edge before checking that voxel. Traversal now keeps
integer cell ownership, advances a crossed axis explicitly, computes absolute
plane distances and handles tied axes without jumping over cells. Near and sparse
LOD paths share the stepping helper; fused point evaluation reduces rounding.
This also preserves progress for kilometre-long CSM rays without an epsilon.

The expanded edge test checks 128 hits/misses on isolated and adjacent cubes,
hierarchy enabled/disabled, and translations 0, 512, -1024 and 10000. The full
Vulkan suite passed after the traversal change, including zero-error planar AO,
water, CSM, 5.3 km rays, LOD edit retention, mesh backends and resource-pack alpha.
Release build, 42 CPU tests and Clippy with `-D warnings` passed.
The 128-ray edge test also passed with bindless textures and hardware ray queries
disabled. A native 600-frame view64 turning/movement/resize run completed in
8.200 s with core and synchronization validation enabled and no reported errors.

## Correct minification versus magnification — 2026-10-21

The earlier all-linear default was incorrect for magnified block textures.
The atlas now uses `magFilter = NEAREST`, `minFilter = LINEAR`, nearest mip
selection and `maxLod = 0.25` with its existing single mip level. Compute block
shaders derive an unwrapped planar camera footprint and supply explicit LOD;
LOD zero/maxLod zero would otherwise always choose magnification filtering.
The HUD toggle now describes linear minification with nearest magnification.

`--texture-filter-test` creates a red/blue striped PNG in a synthetic pack and
renders the same UV boundary at 129×129 and 17×17. Magnification preserves a pure
nearest red texel; minification produces the expected bilinear purple. Resizing
back restores nearest red, and the nearest-minification override also passes.
The full native Vulkan smoke suite, 42 CPU tests, release build and Clippy pass.

## Hybrid hard islands and wide CSM softness — 2026-10-21

The elevated-caster `--shadow-quality-test` reproduced a fixed CSM partial-shadow
area of `[233, 233, 233]` at 0.45°, 1° and 2°. Of 1,423 analytically identified
hybrid penumbra pixels, 1,190 were unclassified and retained hard shadow values.

The four-texel filter cap is removed. PCSS-style search covers the light cone;
16/32 disk taps use bilinearly reconstructed receiver-plane depth comparisons,
wide patterns rotate temporally, and cascade selection reserves filter coverage.
Binary CSM estimates no longer select hard central rays: near voxel/polygon
paths share stochastic disk directions and retain temporal history, with a fast
spatial-filter skip for stable uniform regions.

The corrected native test reports CSM partial areas `[501, 1235, 2384]` and zero
unclassified pixels / hard islands among the 1,423 reference penumbra pixels.
The existing four-texel limitation described in earlier validation entries is
superseded by this change.

The full native GPU smoke suite passes with core and synchronization validation;
42 CPU tests, the release build and Clippy also pass. A normal 600-frame native
view64 streaming/turning/resize run completed in 8.379 s (71.6 FPS average), with
10.117 ms median and 21.039 ms p95 frame latency after warmup. This is a changing
view-distance stress sequence, not a fully populated fixed-camera benchmark.
An earlier interactive run with synchronization validation exceeded its 40 s
timeout; the ordinary run completed, but validation-mode stress performance
remains uncharacterized.

## Sky SH lighting and energy split — 2026-10-21

The native full Vulkan smoke suite passes with core/synchronization validation,
including RGB horizontal sky+direct conservation, finite SH coefficients and
SH-versus-quadrature irradiance agreement within 15%. All 42 CPU tests and Clippy
with warnings denied pass. The release build compiles all WGSL/GLSL stages.
A 180-frame fixed-time default view8 window run at 1280×720 on RTX 2060 SUPER
averaged 192.8 FPS; after 60 warmup frames p50 was 4.921 ms and p95 5.439 ms.
Mean GPU shade time was 2.081 ms and total frame GPU time 4.143 ms. This is a
single static-camera measurement, not an isolated before/after overhead result.

## SSAO metre footprint and filmic display — 2026-10-21

The floor/wall GPU fixture checks occlusion 0.65..0.9 m from a wall at radius
1 m, no occlusion beyond 1.05 m, monotonic strength through 8, and exact
pyramid/reference parity. Planar grazing/motion/odd-resolution tests still
produce zero self-occlusion. The final display test compares GPU output against
the Uncharted curve plus sRGB transfer at -4/0/+4 EV and checks diagnostic/raw
bypasses. Full native Vulkan smoke passes with core/synchronization validation;
42 CPU tests, release build and warnings-as-errors Clippy also pass.

Tone mapping is fused into final composition rather than reading HDR back to
the CPU or allocating another fullscreen buffer. Both opaque and sky-only paths
are covered; lighting and SSR input remain HDR. Uniform ABI is now 480 bytes in
Rust, WGSL and GLSL.

## Ground reflection and Hable saturation — 2026-10-21

Native GPU regressions now check a downward-facing wood receiver over terrain:
bounce on brightens it relative to bounce off, snow returns more energy than
stone, reflectance stays bounded, and removing the terrain clears bounced
radiance. The existing upper-sky/direct conservation and sky quadrature tests
remain unchanged and pass. Saturation 0/1/2 is compared per channel against the
Hable curve plus luminance mixing and sRGB transfer. Full native smoke passes
with core/synchronization validation; 42 CPU tests, release build and Clippy with
warnings denied also pass. The tiny reflectance-bound comparison allows 1e-5
for parallel float reduction rounding.

## Deferred NPC activation — 2026-10-21

46 CPU tests pass, including four new entity-unit tests for stationary and
one-second moving-player queries, frozen inactive rigs, player unions/removal,
negative coordinates and 3D activation bounds. The native `--entity-test` passes
with core/synchronization validation: inactive fish disappear on software and
hardware backends; removing a remote player rejects a far fish even when the
published scene still contains its old active geometry. This also exercises
forced nonopaque hardware candidates for live entity-volume rejection.

The ordinary 600-frame view64 streaming/turning/resize stress run completes,
retaining previous immutable snapshots while the worker updates. Dormant rigs
keep storage/BLAS indices but do not animate or refit; static foliage retains its
terrain rendering policy. No terrain LOD representation changes are included.
The final full native smoke suite passes with core/synchronization validation,
including the remote-player late-snapshot case. Clippy passes with warnings
 denied and the release executable builds. The 600-frame stress run averaged
82.8 FPS (7.250 s), with 8.579 ms median / 19.825 ms p95 after warmup and
0.755 ms maximum main-thread streaming synchronization. Those figures describe
the changing-view stress sequence, not a fixed-camera performance comparison.

## Sparse voxel octree distant terrain — 2026-10-21

All runtime distant pages now use format 2 SVO forests; dense pages remain only
as a QA reference. Per chunk, a four-byte 24-section root mask precedes eight-byte
nodes with an eight-bit child-presence mask, relative child base and aggregate
material ID. A single uniform 16³ section is 12 bytes rather than 4,096 block
bytes. Positions are implicit. Forest roots remain 16 m; 64 m parents spanning
multiple chunks are not implemented by this format.

49 CPU tests pass, including uniform/empty collapse, exhaustive sparse child-rank
lookup and finest-tree/generated-chunk parity. Native `--svo-test` passes with
core/synchronization validation: exact signed-coordinate hits, shader-only
pixel-threshold parent termination and averaged texture-palette colors. The full
native smoke suite also passes, including dense-versus-SVO hit depth at all five
storage scales, water/shadows, local edit replacement and all previous tests.
Clippy passes with warnings denied; all WGSL/GLSL stages and the release build
compile with the 496-byte shared uniform ABI.

The full view64 streaming test populated 16,352 distant pages in 9.49 s; first
publication was 0.007 s, payload 3.79 MiB, and 7,574 distant pixels were visible.
It verified that shrinking range evicts the pages. The ordinary 600-frame
view64 turning/movement/resize run completed in 6.708 s (89.4 FPS average), with
8.218 ms median and 18.269 ms p95 latency after warmup. Its effective detail target
was budget-adaptive (7.1 pixels at the final 1280×720 extent); this is not an
isolated performance comparison or a guarantee of one-pixel finest residency.
The GPU still applies the requested projected-size stop to the available tree.

## One-pixel LOD fidelity and cache defaults — 2026-10-21

The previous full-height dense estimate silently raised requested 1px residency
to about 7px, truncating uploaded trees before the shader could refine them.
Normal residency now uploads full-detail SVOs. Only actual encoded-byte/allocator
pressure can request coarser storage; queued re-offers are seeded from real
resident versions/scales so retained pages are not retransferred unnecessarily.
GPU stopping bounds projected width/height including off-axis position and depth
extent. Subpixel base leaves also use averaged appearance.

The 64 MiB and 128 MiB pool tests genuinely forced quality relaxation for a full
1080p-residency window. The pool is now 256 MiB, retaining replacement headroom.
`BLOCKWORLD_LOD_TEST_HEIGHT=1080 target/release/blockworld --lod-stream-test` populated all 16,352 pages at
1.00px in 10.08 s (first page 0.006 s): all pages retained finest-detail trees,
127.08 MiB payload, with no hidden target relaxation. The pool reservation is
256 MiB, not 127.08 MiB total application VRAM. The test also checks visible
terrain, memory bounds and eviction. The CPU chunk-cache default is 512 MiB;
the separate CPU SVO preparation cache remains 64 MiB.

The full native smoke suite passes core/synchronization validation, including the
new empty-fullscreen-residency regression, conservative SVO stops, subpixel
appearance, water/shadows and per-chunk edit retention. CPU worker tests now
assert that an unpressured coarse display target still receives a full-detail
tree for shader selection.

49 CPU tests and warnings-as-errors Clippy pass; the release executable builds.
The final 600-frame native view64 streaming/turning/resize run completed in
7.040 s (85.2 FPS average), p50 8.305 ms / p95 19.236 ms after warmup. All final
resident pages were finest detail and the target remained 1.0px. This is the
changing-view stress sequence, not an isolated before/after speed comparison.

## 2026-10-21: 128-chunk range, uniform cross-chunk parents, noisy fixture

Release build, 51 CPU tests and Clippy with warnings denied pass. Full native
Vulkan smoke suite passed core and synchronization validation, including local
edit replacement, sparse traversal, water, shadows and resizing.

Complete 64-chunk streaming: 16,352 finest-detail pages, 127.08 MiB payload,
9.77 s preparation/residency, first page 0.012 s. Merged/unmerged GPU primary
visibility matched. Complete 128-chunk streaming at a 161×91 QA framebuffer:
65,760 pages in 62.53 s, first page 0.009 s, 12.17 MiB final payload after actual
memory pressure selected a 1.32 px storage target. Level counts were
[243, 1462, 5796, 22938, 35321]. The dense near window remained bounded and
all distant pages were evicted on reducing range. Primary traversal steps in
the final 128-chunk camera were 636,711 merged versus 639,823 unmerged (0.49%
fewer); materials and depths matched. This is not a GPU-time speedup claim.
The enlarged toroidal page table and merge metadata reserve 32 MiB separately
from the payload inside the existing 256 MiB pool.

`--noisy-world --fly --view-chunks 16 --no-meshes --no-audio --frames 180`
completed under native core/synchronization validation. Independent per-voxel
noise deliberately resists homogeneous compression; the CPU test checks exact
round-trip samples and verifies encoded storage exceeds dense block IDs. The
interactive run reached 473 finest pages / 90.79 MiB while loading, averaging
104.9 FPS over 180 frames at the initial window extent. It is a startup-loading
sample, not steady-state or a comparison with near SVO rendering. Near SVO
rendering is not enabled by this change; the existing near occupancy path stays.

## 2026-10-21: separate experimental one-bounce SVO GI

Release build, 51 CPU tests and Clippy with warnings denied pass. The dedicated
`--vxgi-test` passes native Vulkan core/synchronization validation: unchanged
primary geometry, changed indirect lighting, finite nonnegative RGB, stationary
history accumulation, setting/edit/disocclusion rejection, temporal bypass,
identical original ambient after toggling off, asynchronous near-page residency,
eviction/re-enable, odd and 1px resizing. The full original smoke suite also
passes with GI disabled, including AO, sky/ground energy, sparse LODs, local
edits, water, soft shadows and entities. Shared Params are now 512 bytes; GI
history uses binding 28 and the descriptor layout requires 23 storage buffers.

The GI prototype samples four rotating cosine-weighted directions per half-res
sample, using a cone footprint to select SVO detail and uniform 32/64m parents.
It shades representative material-average voxel hits on demand; this is not a
pre-injected radiance volume, opacity-mip solution or recursive multi-bounce GI.
Current limits and reproduction commands are in VOXEL_GI.md.

Serial 600-frame windowed measurements on RTX 2060 SUPER, 1280×720, same fixed
camera, `--no-audio --no-meshes --freeze-animation --fixed-time 0`:

- Original ambient/SSAO: 284.5 FPS, warm p50/p95 3.409/3.947 ms,
  mean GPU total 2.753 ms, SSAO 0.271 ms, shading 1.653 ms.
- With `--vxgi`: 161.1 FPS, warm p50/p95 5.885/6.953 ms,
  mean GPU total 5.205 ms, AO bypass 0.022 ms, shading including GI 4.308 ms.
  All 289 near SVO pages loaded, 2.14 MiB payload, 1px storage target.

Startup is excluded from average FPS; GPU means exclude 60 warmup frames. These
are fixed-camera measurements with polygon rendering disabled, not a full-game
or moving-camera performance guarantee. Interactive display still has no image
readback; headless QA reads buffers only for assertions.

## 2026-10-22: optical water defaults and texture independence

Water density defaults to 1.0, underwater fog to 0.35 (saturation remains 1.15).
Removed resource-pack water albedo from both optical surface blending and the
scattering tint; wave slope maps continue to control normals. Regular terrain
texture sampling remains unchanged. Release build and warnings-as-errors Clippy
pass. Native water optics regression passes Vulkan core/synchronization
validation. The resource-pack GPU regression also verifies that advancing an
imported red/blue animated water texture leaves the optical water image identical
with waves held fixed, alongside the existing pack/alpha checks.

## 2026-10-22: stochastic small/medium water-wave bands

Enabled world/UV-anchored triangular stochastic tiling for the small and medium
wave bands; the large band remains coherent. Three stable hashed transforms per
band, smooth barycentric blending and approximate variance compensation replace
regular repetition. Slopes and explicit anisotropic footprints use matching
rotation transforms. The shader has seven wave texture reads instead of three,
with unchanged texture storage. ImGui and `--no-stochastic-waves` preserve A/B
comparison. Water density/fog/saturation defaults remain 1.0/0.35/1.15.

Release build, 51 CPU tests and warnings-as-errors Clippy pass. The Vulkan water
regression passes core/synchronization validation, including new checks for a
changed stochastic pattern, exact replay after toggling, tiny-time-step
continuity, normalized output normals and unchanged primary geometry/path
lengths. Captured shaded/normal fixtures were visually inspected for obvious
triangle seams; no obvious boundaries were observed in these views.

Serial fixed-camera lake tests on RTX 2060 SUPER at 1280×720, 600 frames each,
`--water-view --no-audio --no-meshes --freeze-animation --fixed-time 1.25`:

- With `--no-stochastic-waves`: 129.7 FPS, water composition 3.596 ms,
  mean GPU frame 6.947 ms, warm p50/p95 frame latency 7.606/8.439 ms.
- Default stochastic waves: 127.2 FPS, water composition 3.679 ms,
  mean GPU frame 6.998 ms, warm p50/p95 frame latency 7.646/8.647 ms.

GPU means exclude the first 60 frames; FPS excludes startup. The approximately
0.08 ms water-pass difference is one scene's measurement, not a general cost
bound. Interactive display remains GPU-only; exported images are headless QA.

## 2026-10-22: six wave bands, procedural domain warp and distance blend

Default water now combines six non-dyadic frequency bands (approximately 0.89 m
through 73 m tile wavelengths), with independent non-orthogonal orientations,
travel directions and speeds. Two slowly advected gradient-noise fields produce
coordinate warping; analytic derivatives consistently transform texture
footprints and sampled slopes. Three finer bands use stochastic tiling and fade
from 32 to 128 m by default; the broad bands persist. A footprint-based cutoff
also suppresses unresolved frequencies at grazing angles. Controls and limitations
are documented in WATER.md. The previous three-band path remains available via
`--no-multiscale-waves`. Shared Params are 528 bytes with a separate wave-control
vector; texture storage and descriptor counts are unchanged.

Release build, 51 CPU tests and warnings-as-errors Clippy pass. Native Vulkan
water tests pass core/synchronization validation. Added checks cover the warp
and multiscale toggles, exact deterministic replay, active far-field macro
swells, camera-distance detail blending and far-normal continuity across a
1/60-second time step. Existing checks cover normalized normals, short-time
continuity, geometric visibility/path-length invariance and water optics.
Shaded and normal captures were inspected without obvious triangle seams.

Serial 600-frame fixed lake runs on RTX 2060 SUPER, 1280×720,
`--water-view --no-audio --no-meshes --freeze-animation --fixed-time 1.25`:

- Previous model (`--no-multiscale-waves`): 122.0 FPS, mean water composition
  3.934 ms, mean GPU total 7.339 ms, warm p50/p95 8.109/8.889 ms.
- Default six warped bands: 119.7 FPS, mean water composition 4.141 ms,
  mean GPU total 7.579 ms, warm p50/p95 8.309/9.008 ms.

GPU means exclude 60 warmup frames; FPS excludes startup. Water composition
includes the optical rays whose directions change with wave normals, so the
approximately 0.21 ms difference is a scene-level measurement, not an isolated
texture-sampling cost or a guarantee for other cameras. Gameplay presentation
remains GPU-only; image readbacks are confined to headless QA.

## 2026-10-22: climate biomes, biome materials and cave networks

Implemented ocean, deep ocean, meadow, jagged peaks, forest, plains and desert;
BIOMES.md is the separate implemented-biome reference. Smooth independent
temperature/humidity fields select inland climate classes, with a temperate gap
between hot desert and cold peaks. Height remains continuous across biome IDs;
continental base height, erosion-shaped valleys, roughness and ridged uplift
control terrain. Chunk metadata stores one stable biome byte per X/Z column.

New voxel IDs 15–19 (gravel, sandstone, granite, meadow grass and podzol) preserve
old IDs and polygon slots. Expanded host material/atlas metadata and SVO
aggregation support these IDs. Existing texture packs map standard block-face
names; generated textures provide fallbacks. Trees and foliage use biome-aware
surfaces/density, retaining collection identity and whole-tree removal.

World-aligned 4m 3D density lattices carve interpolated tunnel/chamber fields,
with protected base/roof layers, sparse surface entrances and simple deep
aquifers. Normal world data and async LODs share this generator. SVO cache
version increments to 3; no disk persistence is introduced.

Release build, 57 CPU tests and warnings-as-errors Clippy pass. Tests include
all seven biome classes, climate/height continuity, distinct surface layers,
signed-coordinate cave border equality and traversable connections, actual
runtime carving, protected floor/roof, biome identity across edits/eviction,
new material IDs through finest/coarse SVOs, and existing tree/physics/cache
checks. Full native Vulkan smoke passes core/synchronization validation,
including GPU rendering of all five new materials, texture packs, local LOD
replacement, water, shadows, AO, tone mapping and entities. The tone-mapping
fixture now explicitly uses saturation 1.0 when isolating the curve and also
checks 1.15, fixing an old assumption about the changed gameplay default.

`--biome desert --fly --headless` and the existing `--mountain-view` inspection
mode render successfully under core/synchronization validation. Forest, desert
and snowy-peak captures were inspected. `--biome NAME` searches deterministic
sampled columns within 8192m; its CLI accepts hyphenated or quoted biome names.

New-terrain `BLOCKWORLD_LOD_TEST_VIEW=16 ... --lod-stream-test` completed all
800 distant pages in 0.99 s (first page 0.006 s), 11.70 MiB payload, all finest
level at a 1px target. Merged/unmerged GPU primary visibility matched, and range
reduction evicted distant pages. Primary traversal steps were 11,866/12,111 in
that QA camera. These are loading/QA measurements, not a gameplay FPS claim.

## Wave-driven underwater caustics — 2026-10-22

Added a local 256×256 compute photon-density map with four underwater depth
planes (1/3/7/15 m), shared live wave sampling, fixed-point bilinear splats and
filtered depth interpolation. Direct lighting includes refracted direction,
Fresnel transmission, horizontal-irradiance normalization and incident-path RGB
attenuation. Optical-ray/water/wave gates, depth and map-edge guards, and a
camera-distance fade restore ordinary direct lighting outside local coverage.
See [CAUSTICS.md](CAUSTICS.md) for approximation limits and controls.

Final release build, all **57 CPU tests**, and warnings-as-errors Clippy pass.
The full native Vulkan smoke suite now includes caustics and passes with core
and synchronization validation enabled, without validation errors. Dedicated
voxel-GI validation also passes after the compute-pipeline layout change.
Caustic QA verifies 38,025 water-source texels in a controlled pool, four distinct
approximately energy-conserving depth planes, uniform photon density when IOR
is 1, deterministic replay, wave animation, optical-ray/water/wave/zero-strength
gates, ambient isolation, exact far-fade bypass, and covered-pool rejection.
On/off underwater captures and the four map slices were inspected. Headless
mapped buffers are QA facilities; windowed maps remain device-local.

Final native **1280×720 RTX 2060 SUPER** comparison, 600 frames per run, 60-frame
warmup excluded from GPU averages:

```
target/release/blockworld --biome ocean --fly --frames 600 --no-audio \
  --no-meshes --freeze-animation --fixed-time 1.25 [--no-caustics]
```

| Measurement | Caustics off | Caustics on |
|---|---:|---:|
| Mean total GPU time | 4.999 ms | 5.513 ms |
| Mean caustic generation | 0.000 ms | 0.041 ms |
| Mean water composition | 2.439 ms | 2.698 ms |
| Presented FPS, including warmup | 172.1 | 158.2 |

These are scene-specific samples with normal run-to-run variation. The ocean
inspection camera is near a coastline; they are not worst-case dense-pool or
full-screen submerged costs. The delta includes receiver lookup/shading and
other frame timing variation, not only generation. Core/sync validation was
used separately, and no Rust compilation ran during these benchmarks.

## Fine shallow-water caustics — 2026-10-22

Added a 32 m / 256² near map (12.5 cm spacing) alongside the 96 m coarse map.
Near photons use normalized elliptical footprints derived from neighboring
refracted-ray Jacobians, with bounded support and smooth cutoff. A workgroup
shares 81 source rays for 64 photons. Near reconstruction removes the coarse
map's extra tent blur. Five shallow slices at 0.25/0.5/1/2/4 m avoid blending
unrelated patterns from the old 1/3/7/15 m depths. Edge and depth blending retain
the coarse/far fallback. Storage increases from 1.25 to **2.75 MiB**. F11 and CLI
can disable near mapping, adaptive footprints and shallow slices independently.

The release build, **57 CPU tests**, warnings-as-errors Clippy and dedicated
native Vulkan core/synchronization validation pass. The controlled 2 m pool
compares legacy, fine map only, adaptive footprints, and shallow slices at the
same pose/time/wave strength. Every stage changes the receivers. Checks cover
near-source coverage and energy, equal-IOR uniform density, small-time animation
continuity, and overlapping map values across an origin snap (allowing at most
16/8192 density rounding difference). Existing optical gates, replay, depth,
ambient isolation, far fade and covered-pool checks remain. Captures were
inspected. A Naga 24 workgroup-array ArrayStride decoration caused a validation
error during development; the build now removes only the workgroup-pointee
stride annotation, preserving storage-buffer layouts, and validation is clean.

Matched 16×16 m receiver patch at 2 m depth, reconstructed from the diagnostic
8-bit density captures (approximate, including their quantization/clipping):

| Stage | Mean density | Relative RMS contrast | 99th percentile density |
|---|---:|---:|---:|
| Legacy, including its tent filter | 0.993 | 0.030 | 1.08 |
| Fine near map, bilinear photons, old depths | 0.993 | 0.104 | 1.32 |
| Adaptive footprints, old depths | 0.993 | 0.122 | 1.39 |
| Adaptive footprints and shallow depths | 0.993 | 0.157 | 1.50 |

The increase is focused contrast rather than an average-energy boost. Wave
strength stays at 0.22; no caustic curvature multiplier or temporal history is
introduced. Screenshots show more defined fine bright structures, while the
actual strength of shallow focusing remains limited by the shared wave field.

Native RTX 2060 SUPER, 1280×720, same ocean coastline camera, 600 frames per
configuration, GPU means after 60 warmup frames. Base command:

```
target/release/blockworld --biome ocean --fly --frames 600 --no-audio \
  --no-meshes --freeze-animation --fixed-time 1.25
```

| Configuration / extra flags | Map generation | Total GPU |
|---|---:|---:|
| Legacy: `--no-caustic-near` | 0.045 ms | 5.351 ms |
| Fine only: `--no-caustic-adaptive --no-caustic-shallow` | 0.065 ms | 5.389 ms |
| Adaptive: `--no-caustic-shallow` | 0.132 ms | 5.504 ms |
| Final default | 0.140 ms | 5.469 ms |

Timings include normal run variation; small total-time differences are not a
precise standalone shader-cost estimate. This is a coastline view, not a dense
pool or full-screen submerged worst case. No Rust compilation ran during the
retained benchmarks. Window presentation and both maps remain entirely on GPU;
headless map readbacks/captures are test-only.

Final full native Vulkan smoke suite also passes with core and synchronization
validation, including the refined caustic checks; no validation errors or
warnings were emitted. Persisted controlled-pool comparison images are in
`docs/images/caustics-legacy.png` and `docs/images/caustics-refined.png`.

## VXGI cave leakage and independent face masks — 2026-10-22

Corrected unoccluded SH at bounce sources, unknown-residency light, and the
unoccluded ambient fallback for unmatched half-resolution reconstruction.
Sources now require known solid interior/air-side exposure, a sky-visibility
probe, and direct-light visibility when shadows are enabled. Environment light
requires proven escape through known air; range/step exhaustion is not an
exposed sky. Visibility restarts use the original ray origin, not a rounded
exit point. Exact near geometry overrides stale/coarse near SVO pages, while
empty near sections/bricks accelerate rays using the upload origin as their
alignment anchor. SVO space remains aligned to the world grid.

`--vxgi-test` includes a sealed one-voxel cave spanning chunk/section boundaries,
three camera angles and 1/24/96 m cone budgets. Active-bank GI irradiance must
remain below 1e-5. Source sunlight is tested with CSM on and off; contradictory
stale near SVO geometry cannot replace exact cave walls. Removing the roof must
admit nonzero skylight. Existing history, toggles, streaming and resizing checks
remain. This is a synthetic reproduction of verified leak paths, not the exact
camera position in the user's screenshot.

Stored face-mask generation/consumption has **separate tests**. User byte layout:
bit 0 fully occluded, bit 1 fully unoccluded, bits 2–7 closed −X/+X/−Y/+Y/−Z/+Z
planes. Original-resolution voxels prove full-plane coverage for all LOD levels;
partial/cutout/unknown faces do not become full coverage. The byte uses reserved
bits in the second SVO word: nodes remain **8 bytes**, encoded payload and
resident cache/VRAM bytes are unchanged. Cache version advances to 4. Shared
traversal exposes masks for other passes without changing primary visibility.

Isolated release CPU A/B (`cargo test --release svo_face_mask_metrics --
--nocapture --test-threads=1`): 4,489-node heterogeneous section, **35,916 bytes
in both modes**, median encode **0.261 ms off / 0.527 ms on** over nine samples
per mode. Timings vary with scheduling/clocks; this test isolates encoding, not
whole chunk generation. Peak pyramid-build scratch adds one 96 KiB exact-source
copy; persistent masks add zero bytes. Separate correctness checks cover fully
closed/open nodes, partial faces, cutout leaves and unknown chunk boundaries.

Independent native GPU A/B (`--gi-mask-test --no-audio`), RTX 2060 SUPER,
320×180 headless targets, 289 resident SVO pages outside the dense near window:
**611,037 nodes / 4,889,452 payload bytes**, **0 additional resident bytes**.
Fixed cone directions, temporal bypass, paired OFF/ON/ON/OFF batches, eight
warmup frames per batch, 64 retained samples per mode. Rendered images match
exactly and GI contains nonzero illumination. Dedicated GI-dispatch timestamp
median: **0.763 ms off / 0.766 ms on**, an apparent −0.42% gain within ordinary
run variation. No measurable GPU benefit is claimed. No compilation ran during
the retained benchmark. This is a headless shader A/B, not gameplay FPS.

The stored-mask runtime shortcut is **off by default**; F11 or `--gi-face-masks`
enables it, `--no-gi-face-masks` bypasses it. Metadata preparation remains active
for shared use and future experiments; CPU generation cost is measured separately.
Leakage correctness fixes stay active in either mode. See
[SVO_FACE_MASKS.md](SVO_FACE_MASKS.md) for limitations and reproduction commands.

Final release executable builds; **59 CPU tests** and warnings-as-errors Clippy
pass. Final VXGI cave tests, the independent mask fixture, and the full native
Vulkan smoke suite pass core and synchronization validation without errors or
warnings. Existing primary SVO rendering, block edits, water/caustics, shadows,
materials, entities and resizing remain covered. Validation-enabled timings
are diagnostic only; the mask performance sample above was taken separately
without validation overhead.

## VXGI sky restoration and approach reprojection — 2026-10-22

The previous cave change overcorrected: sky escape required reaching y=320
before leaving residency, a single binary probe gated source SH, foliage cubes
were opaque to GI, and unsupported half-resolution receivers returned black.
These conditions removed outdoor sky energy and made small/thin surfaces depend
on screen coverage. The earlier closed-room tests did not cover those outdoor
and moving-camera cases.

`gi_trace.wgsl` now shares transport between half-resolution GI and traced
full-resolution reconstruction fallback. Sky visibility tests the actual dense/
resident occupied ceiling and finite rendering-domain exit. Unknown interior
holes and exhausted traversal remain conservative. Occluders are still tested
before escape, preserving sealed caves. Two stable probes replace the single
source-SH gate. Alpha-tested voxel foliage uses animated mean texture alpha
for throughput and weighted source contribution. Alpha is packed into the
unused high byte of the existing material-average word; metadata stays 12 words
per face. A CPU test checks opaque fallback and animated half/quarter leaf
coverage/interpolation. This is mean-alpha density, not an opacity SVO pyramid.

Each 2×2 GI tile selects an actual visible receiver. Missing full-resolution
support evaluates the same four-cone estimate rather than choosing black or
unoccluded SH. Samples are world-anchored, with a face-normal/hash-grid offset preventing
normal-coordinate roundoff from changing the sample sequence on voxel faces. Temporal reprojection gathers four
previous neighbors, validates frame/material/normal and tangent-plane distance,
uses projected-footprint distance tolerance, and favors near-exact world-point
matches. History still caps at 16 samples; no TAA or camera jitter is added.

New `--gi-sky-test` (also included in `--vxgi-test`) verifies:

- Exposed primary receiver skylight despite a distant y=319 pillar, reproducing
  the old world-height/residency escape failure: RGB (0.1894, 0.2255, 0.2706).
- A downward-facing receiver gets a sky-lit floor bounce, with direct sun zero:
  RGB (0.1096, 0.1195, 0.0971).
- Canopy/trunk GI remains nonzero; fixed-seed irradiance at the same world point
  is stable within 0.003 vector length at 8/5/4/3/2/1 m camera distance.
- A 64-frame fixed-seed approach has neither a step above 0.025 nor accumulated
  drift above 0.025 in vector length.
- A 128-frame approach with the normal rotating sampler, after warmup, has blue
  irradiance **0.1662 early / 0.1639 late** (32-frame averages), rather than
  progressively darkening. These values belong to the controlled test tree,
  not the user's exact location.

Release build, **60 CPU tests**, warnings-as-errors Clippy, extended VXGI tests
(including sealed caves, source sun, stale SVO, real roof opening, history and
streaming), and full Vulkan smoke pass. Native core/synchronization validation
reports no errors or warnings. Interactive rendering remains GPU-only; test
readbacks inspect irradiance and geometry without altering presentation.


## 2026-10-22: VXGI performance rollback

The cave-leakage, stored-mask and sky/reprojection experiments described above
were reverted from the active implementation. Their passing correctness tests
did not establish acceptable gameplay performance. The experiments and their
tests are preserved on `vxgi-regressed-2026-10-22`; their historical results are
not claims about the restored shader. See [PERFORMANCE_REFERENCE.md](PERFORMANCE_REFERENCE.md)
for matched windowed/fullscreen timings, accepted cave leakage and verification.
