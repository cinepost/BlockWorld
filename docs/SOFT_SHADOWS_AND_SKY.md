# Soft shadows and atmospheric sky

The F11 HUD selects `Ray Traced`, `Hybrid` (default), or `CSM`.
`--shadow-type ray|hybrid|csm` selects the same modes on startup.

| Mode | Surface shadow visibility |
| --- | --- |
| Ray Traced | Voxel traversal and polygon ray queries throughout the view |
| Hybrid | Rays nearby, blending into CSM over the last 8 m of the configured ray distance |
| CSM | Cascaded depth lookups, without per-pixel shadow rays |

Actual voxel/polygon depth maps default to four 512×512 cascades each, consuming
8 MiB. The HUD and `--csm-resolution` select 128, 256, 512, 1024 or 2048;
actual-map storage is `32 × resolution²` bytes (32 MiB at 1024, 128 MiB at 2048). The separate conservative rejection hierarchy remains 128×128 and
512 KiB. These are separate resources with different jobs. Depth construction
is cached; animated polygon depths refresh on a bounded cadence.

## Disk lights and filtering

The default angular radius is 0.266 degrees. The CSM estimate chooses one disk
sample in apparently uniform regions and 2–4 disk samples around detected edges
(default 3). A binary map value is not proof of geometric umbra: the one-sample
path also samples the disk, rather than falling back to a hard centre ray. Near
receivers retain temporal history even when the map classifier misses an edge.
Stable lit/umbra pixels skip the spatial filter and shading correction. A highest
surface with no polygon casters can still use a proven-lit fast path.

The mesh pass considers both voxel and polygon edge estimates, so joint near
visibility always intersects the same disk directions before averaging. Software
and Vulkan ray-query paths share this behavior. Distant map filtering combines
the caster classes approximately; CSM-only shading performs no per-pixel rays.

CSM uses a PCSS-style blocker search, including the surrounding physical light
cone instead of only the centre 3×3 neighborhood. Width follows
`tan(angular_radius) * blocker_receiver_separation`, with no four-texel cap.
Small filters use 16 disk taps, wide filters 32; every tap bilinearly interpolates
four receiver-plane-corrected depth comparisons. Sub-texel filters use one
bilinear comparison. Wide patterns rotate over frames and use the temporal
filter. Cascade selection reserves space for the cone, and samples outside a
map are lit instead of repeating a clamped edge. The cost is bounded by sample
count rather than blur width. The ray-sample slider controls near ray queries;
`--hard-shadows` disables the disk path.

Opaque penumbrae use reprojection, position/normal/material rejection, history
clamping, first/second moments, and a variance-aware 3×3 edge filter. Uniform
pixels skip filter arithmetic. This is a compact SVGF-inspired filter, rather
than a full multi-scale SVGF implementation. History resets on relevant scene,
manual light, mode and render-target changes. Routine animated map refreshes,
cascade-grid movement and gradual day-cycle lighting retain validated history. Water interfaces receive disk shadows
but do not yet have a separate temporal shadow history.

History and raw-shadow storage add 80 bytes/pixel (281.25 MiB at 2560×1440).
Windowed rendering keeps these buffers on the GPU. Only diagnostic counters
and timestamps are read back; rendered images are never copied through the CPU
for display.

## Atmospheric cache

A GPU single-scattering Rayleigh/Mie integration builds a 1024×512 HDR
latitude/longitude sky. Earth radii, density scale heights and scattering
coefficients follow the model illustrated by
[wwwtyro/glsl-atmosphere](https://github.com/wwwtyro/glsl-atmosphere), published
under the Unlicense. Integration uses 16 view steps and 8 light steps, with
kilometre units for numerical stability.

A 256×128 importance map stores luminance weighted by cell solid angle,
conditional/marginal CDFs, and a normalized solid-angle PDF. This distribution
is prepared for future sky-light sampling; current rendering does not use it
to launch GI rays.

Two banks hold sky, importance and row metadata (about 17 MiB total). Startup
builds a complete cache. Subsequent rebuilds process 64 sky rows per frame and
publish only after the complete sky and importance distribution are ready.
Readers continue using the previous bank during those eight frames.

The scheduler derives an interval from angular speed and an angular error
budget: 0.18 degrees normally, 0.04 degrees near the horizon, clamped to
0.05–30 seconds. Completion also depends on the eight-frame build budget.
Manual light-angle changes and observer altitude changes over 50 m invalidate
promptly. The HUD reports build progress and target cadence.

`--day-cycle` enables a 600-second cycle by default; the HUD adjusts its duration.
The moon is represented by an opposite disk with reduced intensity and cooler
lighting. This is a simple animation model, without lunar phases, clouds or
multiple atmospheric scattering. `--sun-elevation` sets the starting solar
angle; `--fixed-time` freezes animation for comparisons. `--no-atmosphere`
restores the inexpensive analytic sky.

## ABI and profiling

The shared Rust/WGSL/GLSL uniform is 464 bytes. The device must expose at least
20 storage buffers per compute stage. Eighteen GPU timestamps (144 bytes)
include separate sky-build and shadow-denoising timings.

Representative isolated RTX 2060 SUPER measurements at native 2560×1440,
view distance 8, fixed ordinary spawn, 500 frames / 60 warmup, all new effects:

| Mode | FPS | GPU frame (ms) |
| --- | ---: | ---: |
| Ray Traced | 65.8 | 12.545 |
| Hybrid | 69.7 | 11.694 |
| CSM | 72.2 | 11.295 |

The masked temporal/spatial filter cost about 0.5 ms. These measurements precede
the final shared-sample/classifier refinement and are representative, not an
assertion of a performance improvement over the earlier hard-shadow renderer.
The sky was cached during these fixed-light runs; dynamic rebuild cost must be
assessed separately.

A separate animated `--day-cycle --frames 300 --no-audio` run at 1280×720
completed at 171.4 FPS (startup excluded), with six completed sky builds.
After 60 warmup frames, frame p50/p95/p99 were 5.500/8.057/10.561 ms.
GPU frame mean was 3.876 ms; the final masked filter cost 0.139 ms. This run
also included moving fish/AS updates and changing shadow maps, so its latency
is an end-to-end result rather than an isolated atmospheric integration cost.

### CSM ringing correction

Empty center texels previously substituted view distance for blocker separation;
a sparse nine-point ring then produced detached copies of small shadows. The
subsequent four-texel tent fixed those rings but capped wide penumbrae. It has
now been replaced by bounded disk sampling over the physical footprint, with
bilinear comparisons and temporal rotation for wide filters. Empty map entries
never substitute camera view distance for caster separation. Nearby map
visibility retains a regression comparing view distances of 8 and 24 chunks.

`--shadow-quality-test` uses an elevated solid caster and a planar receiver. It
checks that CSM partial-shadow area grows at 0.45°, 1° and 2°, then compares hybrid
penumbra coverage to a 128-sample CPU disk-light/AABB reference. This reproduces
both the old width plateau and the map classifier's hard islands.

## Anti-aliasing status

TAA and camera jitter were removed. Soft-shadow temporal/variance filtering
remains, including the fixes that preserve receiver history across routine map
refreshes and streaming swaps. There is no temporal color resolve or TAA memory
reservation. Raw shadow storage is back to 16 bytes/pixel, with a separate
64-byte/pixel shadow-history buffer. Primary rays sample pixel centers.

### Sky-derived diffuse lighting and energy accounting

Each completed sky bank now includes nine RGB real spherical-harmonic radiance
coefficients (bands 0–2). The existing 256×128 importance-map preparation also
integrates upper-hemisphere RGB energy and horizontal irradiance, using exact
lat-long cell solid angles. A third dispatch reduces 128 independent row sums;
barriers precede publication of the complete bank. No CPU readback is required.
The SH constant coefficient encodes integrated RGB sky energy divided by
sqrt(4π). Visible sun/moon disks are composited separately and are absent from
this integration; atmospheric forward-scattering glow remains sky light.

Diffuse shading convolves SH with the Lambert kernel (band factors 1, 2/3, 1/4
including the BRDF's 1/π). This costs nine coefficient reads and arithmetic per
surface, with no stochastic sky rays or additional per-pixel history. SSAO
modulates diffuse sky only. Water secondary surfaces and the shadow denoiser's
direct-light correction use the same lighting functions. Importance CDFs remain
available for future visibility-aware sky sampling; SH does not resolve distant
sky occlusion or indirect terrain bounce.

Energy compensation uses a reference horizontal receiver: incoming irradiance
is 22·max(sun elevation sine,0), or 0.035·moon elevation sine. Cached sky SH is
scaled per RGB channel if needed, then direct irradiance receives the remainder.
The remainder is divided by the light elevation cosine so that surface Lambert
shading preserves that horizontal sum. A 0.02 horizon guard retains bounded
residual twilight and avoids division blow-up. This is an approximate energy
allocation model, not a multiple-scattering atmospheric transport solution; the
single active sun/moon switch retains the existing -0.02 threshold. The existing
intensity control scales both components, with a shared artistic exposure
0.8·π/22. Low-order SH can lose detail and is clamped to nonnegative irradiance.

The GPU smoke test checks finite coefficients, the horizontal diffuse+direct
budget per RGB channel, and agreement of SH convolution with sky quadrature
(within 15%). Atmosphere-disabled reference rendering retains its legacy light
model.

### SSAO metre radius and final filmic display

SSAO's old kernel had at most 0.55 times the UI radius of lateral reach. Lateral
coordinates now span the specified radius in metres, with heights bounded to
keep taps inside that sphere. Occluder-distance weights fade over 0.8R..R rather
than R..2R. The depth-pyramid early-out uses a conservative sphere bound for the
new kernel. Default radius is 1 m; the HUD strength range is now 0..8. SSAO still
attenuates only diffuse sky light: it does not darken unobstructed direct sun.

The final composition stage applies the Uncharted 2 filmic curve (2× exposure
bias, white point 11.2), then the sRGB output transfer. Exposure in the Lighting
controls is -8..+8 EV; +1 EV doubles input intensity. Reset exposure restores 0 EV.
The transform is fused with final water/sky output, avoiding another fullscreen
HDR allocation/dispatch. Opaque shading, shadow filtering and reflection sampling
stay linear HDR. Sky sampling now also retains HDR rather than applying an early
exponential curve. Debug views, ImGui and the loading image bypass the transform;
the filmic toggle restores raw-clamped output for comparison.

Previously HDR values were written directly into display bytes without an output
transfer, crushing apparent shadow brightness. The corrected transfer improves
visibility under trees, but the renderer still has no indirect terrain/foliage
bounce. Downward-facing surfaces can therefore remain very dark: upper-sky SH
plus direct light is not a complete global-illumination model. Exposure is an
inspection control rather than an artificial ambient-light floor.

### Average ground reflection and Hable saturation

The earlier no-ground-bounce limitation is superseded by an approximate one-bounce
lower-hemisphere light. Two GPU reduction dispatches sample 256 columns across a
64×64 m patch near the camera, within the resident full-resolution volume. Each
column selects the highest terrain material, skipping air, trunks and leaves;
water uses reflectance 0.02. Each selected terrain top texture is averaged across
its 16×16 texels, including resource-pack tint/animation. Reflectance is bounded
to [0,0.9]. Columns without terrain contribute no light. No CPU readback or
per-pixel bounce rays are involved.

For each channel, ground radiance is rho·(E_sky_up + E_direct·sun.y)/π.
This is reflected energy, so it does not reduce the already compensated primary
sky/direct budget a second time. Incoming lower-hemisphere radiance is integrated
analytically as L_ground·(1-normal.y)/2 in diffuse shading. Upward surfaces gain
zero ground light, side-facing surfaces receive half, and downward surfaces the
full contribution. SSAO modulates combined sky/ground ambient. The original sky
map, importance PDF and sky SH are preserved; the ground term has no SH horizon
ringing and is shared by opaque and water-secondary shading.

Ground reductions refresh every 16 rendered frames, on publication of a sky bank,
and every headless validation frame. This is a local average assuming unshadowed
terrain illumination, without per-column solar visibility or multi-bounce GI;
it can leak light into enclosed rooms. Sampling uses the same albedo convention
as the current material shaders. The Ground bounce checkbox enables comparisons.

The display curve is explicitly John Hable's Uncharted 2 operator:
A=.15, B=.50, C=.10, D=.20, E=.02, F=.30, white=11.2, exposure bias=2.
These match the previous curve's expanded coefficients; naming is now explicit.
Source: https://filmicworlds.com/blog/filmic-tonemapping-operators/
Saturation (0..2, default 1) mixes Rec.709 luminance with tone-mapped linear RGB
before gamut clamping and the sRGB transfer. Zero is grayscale, one preserves
normal color, two increases saturation. Diagnostic views and ImGui bypass it.
