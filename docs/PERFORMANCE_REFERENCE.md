# VXGI rollback reference — 2026-10-22

Git tag: `vxgi-reference-2026-10-22`, on `main`.
The separate branch `vxgi-regressed-2026-10-22` preserves the experimental
implementation before rollback. That branch is diagnostic, not recommended.

## What was restored

The VXGI transport, screen-index/frame sample rotation, temporal reprojection
and half-resolution reconstruction were reconstructed from the implementation
before the cave light-leakage changes. The full-resolution fallback cone traces,
per-source sky-visibility rays, world-space 25 cm sampling grid and face-mask
production were removed. Animated mean-alpha GI transport was also rolled back.

World generation, caustics, water, resource packs, scene lighting, entities and
streaming remain. GPU GI timestamps and their HUD readout remain as useful
instrumentation; the window benchmark now reports mean GI separately. This is
a reconstructed rollback, not a recovered historical commit: the workspace had
no Git history before this checkpoint.

**Known cave light leakage is intentionally retained.** Neither this tag nor its
measurements establish that the user's earlier fullscreen 80 FPS has returned.
The 8 FPS case was not reproduced by these fixed-camera tests. The restored
sampling has no 25 cm hash grid, but moving-camera visual quality still needs
user confirmation in the reported scene.

## Measured conditions

NVIDIA RTX 2060 SUPER, native Vulkan swapchain, same respawn camera, default
8-chunk view distance, default resource pack and mesh backend. Meshes, foliage,
shadows, atmosphere, water and caustics enabled. Audio disabled and animation
frozen at 1.25 s to keep comparisons repeatable. No compiler was running during
performance measurements. The fullscreen monitor extent was **2560×1440**;
windowed extent was **1280×720**.

| Run | Presented FPS | Mean GPU frame (ms) | Mean shade incl. GI (ms) |
| --- | ---: | ---: | ---: |
| Regressed, windowed, 360 frames | 71.1 | 12.568 | 10.533 |
| Restored, windowed, 360 frames | 94.9 | 9.100 | 7.242 |
| Restored, windowed, 600 frames | 96.1 | 9.267 | 7.411 |
| Regressed, fullscreen, 240 frames | 28.3 | 33.667 | 26.496 |
| Restored, fullscreen, 240 frames | 29.3 | 32.690 | 25.381 |

The final 600-frame run measured GI at **5.434 ms** and post-warmup frame
latency p50/p95/p99 at **10.068/11.371/12.025 ms**. Presented FPS includes all
requested game frames after startup; GPU means and latency exclude 60 warmup
frames. Shading includes GI: do not add its separate time again. Fullscreen
results show only a small gain, not a demonstrated recovery to 80 FPS.

Raw logs are in [benchmarks/vxgi-reference-2026-10-22](benchmarks/vxgi-reference-2026-10-22).

```sh
cargo build --release
target/release/blockworld --vxgi --frames 600 --no-audio --freeze-animation --fixed-time 1.25
# Identical camera/effects at the monitor's native extent:
target/release/blockworld --vxgi --fullscreen --frames 240 --no-audio --freeze-animation --fixed-time 1.25
# Interactive scene, with animation and audio enabled:
target/release/blockworld --vxgi --fullscreen
```

## Validation

Release build, 57 CPU tests, Clippy with warnings denied, `--vxgi-test`, and the
full Vulkan smoke test with synchronization validation pass (zero validation
warnings/errors).
The GI test verifies geometry invariance, irradiance bounds, history accumulation
and rejection, mode toggles, streaming/eviction/re-enable and resize handling.
The removed cave/sky tests remain in the experimental branch, where their
assumptions apply. Headless readback is for assertions only; interactive display
never reads rendered images back to the CPU.
