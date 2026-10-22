# Minecraft resource-pack textures

```sh
cargo run --release -- --resource-pack /path/to/pack.zip
# Magnification is always nearest; disable default linear minification:
target/release/blockworld --resource-pack /path/to/pack.zip --texture-filter nearest
```

Loading runs in the existing startup worker while the loading screen remains
visible. F11 shows the pack filename, atlas dimensions, imported/skipped counts
and animation count. Filtering can be changed in the HUD. Select a different ZIP
on the next launch; there is currently no file picker or live pack reloader.
Missing textures retain the generated fallback; fish keep their original textures.

## Import and atlas

The loader uses `zip`, `image` (PNG only), `guillotiere`, and `serde_json`.
It reads canonical ZIP-root entries matching
`assets/minecraft/textures/block/*.png` without extracting files. Other namespaces,
item textures and nested texture subdirectories are not imported. PNGs decode to
RGBA8, including their alpha channel.

This iteration accepts 16×16 sprites and 16px vertical animation strips. The
32px-wide `_flow` convention used in some 16px packs is sliced as 32×32 frames and
nearest-downsampled to 16×16. Other 32/64/128/512px sprites are skipped with
diagnostics; a ZIP with no supported sprites fails explicitly. Higher resolutions
are not silently accepted or stretched into the current 16px material system.

Each physical frame receives an 18×18 allocation: 16×16 interior plus a one-pixel
gutter. Every edge and corner duplicates the nearest interior RGBA pixel. The
atlas grows in powers of two from 64 to a maximum of 4096. All matching supported
PNGs are packed, including assets not currently mapped to a game block.

The master atlas uploads once through a staging buffer to an optimal-tiled,
device-local `R8G8B8A8_UNORM` Vulkan image. Temporary decoded frames and the CPU
master pixels are released after upload. Sampling has one mip level, explicit
one atlas level. Magnification uses `VK_FILTER_NEAREST`, minification uses
`VK_FILTER_LINEAR`, and the optional nearest mode disables linear minification.
The compute shaders derive a planar block-face footprint from the camera before
UV wrapping, supplying explicit LOD to choose the filter. The sampler uses
`VK_SAMPLER_MIPMAP_MODE_NEAREST` and `maxLod = 0.25`, retaining mip zero while
allowing minification; clamping maximum LOD to zero would force magnification.
See [Khronos sampler guidance](https://registry.khronos.org/vulkan/specs/latest/man/html/VkSamplerCreateInfo.html).
The one-pixel
padding is appropriate for this level; future texture mipmaps need their own
per-sprite gutters. Existing shading color-space behavior is retained.

## Current material mappings

| Game material | Pack sprite(s) |
| --- | --- |
| Stone | `stone` |
| Grass block | `grass_block_top`, `grass_block_side`, bottom `dirt` |
| Dirt / sand | `dirt` / `sand` |
| Water | `water_still`, falling back to `water_flow` |
| Wood | `oak_log`, end faces `oak_log_top` |
| Grass / fern cards | `short_grass` (legacy `grass`) / `fern` |
| Broadleaf card | `oak_sapling` |
| Snow | `snow` |
| Oak / pine leaves | `oak_leaves` / `spruce_leaves` |

Grayscale grass/leaves/water receive fixed prototype biome tints; already colored
sprites retain their color. A supplied `grass_block_side_overlay` is composited
over the side texture. Full biome colormap/model behavior is not implemented.
Water textures modulate the existing optical water color; Fresnel, refraction,
reflection and wave normals still come from the water renderer. Lava frames can
be imported, but this game does not yet have a lava block/material.

## Animation

A sibling `name.png.mcmeta` supports:

- `animation.frametime` in Minecraft ticks (20 ticks/second).
- Ordered integer frame indices and `{ "index": N, "time": T }` overrides.
- `interpolate: true`, including interpolation from the last frame to the first.
- Frame width/height declarations consistent with the vertical strip.

Without an explicit sequence, frames run in strip order; the default duration is
one tick. A strip without metadata also uses that default. Invalid indices,
nonpositive durations or malformed metadata skip the sprite with a diagnostic.
Each frame has independent atlas padding. Runtime advances coordinates and blend
weights in a small material-face metadata buffer, never reuploads image pixels.
`--fixed-time` freezes texture animation along with the other deterministic clocks.
Animated cutout changes invalidate cached shadow depths when needed.

## GPU interface

Binding 3 now holds 12-word face records: current and next frame coordinates,
atlas descriptor indices, cutout/filter flags, interpolation weight and RGB tint.
Fourteen materials × six faces occupy 4032 bytes. All shading, secondary optical
rays, voxel alpha tests, software polygon tests and Vulkan ray-query candidate
alpha tests use the same records and animation clock. Leaf alpha holes therefore
also affect depth, AO inputs, CSM construction and ray shadows. Collision still
uses solid block bounds.

On devices with descriptor indexing, binding 24 is a bounded bindless table of
eight sampled-image descriptors; binding 25 is its sampler. The current master
atlas lives in slot zero. Remaining slots are initialized with valid aliases for
future atlas pages. Shaders fetch the descriptor index from face metadata. This
is a bindless atlas handle, not one Vulkan image per sprite. The storage-buffer
count remains twenty. Devices without the required indexing features compile/use
a single-atlas shader variant. `BLOCKWORLD_DISABLE_BINDLESS=1` forces that path;
hardware ray queries can independently be disabled.

## Scope and validation

Blockstate JSON files are counted for diagnostics but not interpreted. Weighted
variants, per-position rotations, model JSON, arbitrary face UVs, CTM/OptiFine,
PBR auxiliary maps and full Minecraft pack parity remain future work. The current
face-record interface separates game material IDs from atlas allocation positions
so those features can be added without changing stored voxel IDs.

Import bounds include 50,000 ZIP entries, 8 MiB per PNG entry, 256 KiB metadata,
1024 frames per strip, 16,384 total frames including fallbacks, and a 4096² atlas.

`--resource-pack-test` builds a synthetic Deflate ZIP and exercises PNG decoding,
GPU frame advancement/interpolation, linear gutter sampling, voxel alpha holes,
polygon-backend alpha agreement and fallback restoration. Unit tests additionally
verify all padded RGBA edges/corners, explicit animation timing/order, ZIP filters,
partial mappings, unsupported sizes and conventional flow strips.

The aggregate appearance word now also carries animated per-face mean alpha in
its high byte (RGB remains in the low 24 bits). VXGI uses this for cutout-voxel
light transmission. CPU metadata tests cover opaque fallback, half/quarter
coverage leaf frames, and interpolation; the record remains 12 words.
