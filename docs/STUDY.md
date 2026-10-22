# Study notes and exercises

## Why these Rust types?

`#[repr(u8)]` fixes the block discriminant size. `#[repr(C, align(64))]` fixes
subchunk field ordering and cache-line alignment. These are explicit layout
contracts, not assumptions about Rust's default struct layout. Const assertions
and tests verify size and coordinate boundaries.

`Option<Box<SubChunk>>` owns either no section or one heap allocation; dropping
the world recursively frees its allocations. An empty high section costs one
pointer slot rather than 4096 bytes. `std::array::from_fn` constructs non-Copy
array elements. The region slot array is built on the heap to avoid putting a
large temporary array on the stack when selecting 64×64 regions.

`HashMap` is used only to find regions. The interior grids use predictable fixed
array indexing. The GPU uses a dense active-window copy for fast shader address
calculation. One CPU block is one byte; one GPU block is four bytes to avoid
requiring Vulkan 8-bit storage extensions.

There are no unsafe world or physics operations. `renderer.rs`, `rt.rs` and
`presentation.rs` contain the unsafe boundary because Vulkan handles and mapped pointers require explicit
lifetime and synchronization reasoning. Device ownership is shared with buffers
through `Arc`; it is not used to share mutable rendering work across threads.

## Coordinate exercise

For world `(x,y,z)=(-1,-1,-1)`, the chunk coordinates are `(-1,-1)` and the
horizontal local coordinates are `(15,15)`. The vertical offset from MIN_Y is
63: subchunk index 3 and local Y 15. Truncating integer division would give the
wrong chunk for negative coordinates; `div_euclid` and `rem_euclid` are essential.

## Vulkan concepts to inspect

- `Entry`, `Instance`, physical device and logical device are distinct objects.
- Headless queue selection requires compute. Interactive mode requires a single
  family supporting compute, graphics and surface presentation.
- A buffer handle is separate from the `DeviceMemory` allocation bound to it.
- Descriptor bindings 0–4 are world, layered visibility, output, textures and
  parameters. Bindings 5–7 hold occupancy sections, camera section mask and SSAO.
  Bindings 8–11 hold polygon objects, triangles, primary hits and sun visibility;
  12 is the optional TLAS descriptor, and 13 is the material table.
- The pipeline layout describes those bindings; the pipelines contain SPIR-V.
- Command buffers record work; queue submission runs it.
- A pipeline barrier synchronizes GPU accesses between passes. A fence tells the
  host when work finishes. Neither replaces the other.
- HOST_COHERENT avoids explicit flush/invalidate; it does not remove the need to
  synchronize host/device access. CPU uploads happen only between completed frames.
- Renderer destruction waits for idle before releasing pipelines/descriptors.
  Buffers retain the device until their allocations are unmapped and freed.
- Interactive buffers are device-local, with a separate host-visible upload
  staging buffer. The host never maps visibility or output in interactive mode.
- Swapchain acquisition and presentation use binary semaphores; a CPU fence alone
  does not order presentation. Each swapchain image owns its render-finished
  semaphore. Reacquisition ensures its previous present wait has completed.
- Nine timestamp queries measure GPU work; reading these tiny counters for the
  overlay is distinct from reading back a rendered frame.

Most shaders are WGSL compiled through Naga. The optional ray-query passes are
GLSL compiled by glslc, providing explicit alpha-candidate confirmation. Vulkan
executes SPIR-V for both source languages. The buffer layouts use
16-byte vector rows so Rust and shader alignment agree without implicit padding.
The layered visibility record is 64 bytes, parameters 176 bytes.

See [rendering details](RENDERING.md) for hierarchical traversal, water, sunlight,
SSAO reconstruction; [hybrid notes](HYBRID.md) cover the implemented triangle
ray-query path and future voxel LOD.

## Follow-up projects

1. Add save/load of edited subchunks with a versioned region file format.
2. Track dirty sections and upload changed ranges rather than the entire window.
3. Generate exposed voxel faces, then greedy-mesh them; replace DDA with raster
   visibility and keep the existing deferred shading stage.
4. Replace the output buffer + direct GPU copy with a storage image. Add two or
   three per-frame resource sets to overlap CPU/GPU work. Preserve the existing
   swapchain image transitions and per-image presentation semaphore ownership.
5. Compare section / small-cell skipping with the exhaustive traversal switch;
   track dirty occupancy cells instead of rebuilding metadata after each edit.
6. Evict distant CPU chunks after saving edits; move generation to worker threads.
7. Add water refraction, reflection rays and temporal denoising; compare SSAO
   against voxel ray-traced ambient occlusion at equal sample budgets.

Before optimizing, compare rendered debug views and keep negative-coordinate,
collision and GPU editing smoke tests passing.

## Streaming ownership

Region chunk slots now hold `Arc<Chunk>`. Window snapshots share immutable chunk
allocations with the worker cache. Loading a resident chunk touches its LRU age
without copying its sections; editing uses `Arc::make_mut` for copy-on-write.
The fixed region and section layouts are unchanged. GPU buffers are `Send`, not
`Sync`: channels transfer exclusive ownership, never shared mapped pointers.
Each AS slot retains the exact shader geometry associated with its BLAS/TLAS.
See STREAMING.md for completion, retirement and budget rules.
