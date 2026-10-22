# Streaming, chunk caching and acceleration-structure snapshots

## Visibility and residency are different

Primary visibility first tests full-height chunk AABBs against the camera
frustum, then occupied 16³ sections inside surviving chunks. GPU traversal skips
empty 16³ sections and 4³ groups, and stops at the nearest opaque block. Polygon
primary rays use that depth as their maximum range. The close-up fixture verifies
that one block filling the image takes at most two primary steps per pixel (air,
then block), with no hidden polygon hits. Frustum masks alone cannot eliminate
geometry behind an occluder. Off-screen chunks may still cast sun shadows and
must remain available to shadow queries.

Turning the camera does **not** generate/upload another terrain window or rebuild
the scene. Residency follows player chunk coordinates and view-distance radius,
with a guard chunk around that radius. Turning only changes the primary frustum
mask and rays. A `--stream-test` also changes position/range to exercise actual
resident-window transitions.

## CPU LRU cache

`World` retains its fixed region slot grids and optional 16³ sections. Resident
chunk slots hold `Arc<Chunk>`; immutable worker/render snapshots share allocations.
Edits use copy-on-write. Touching an existing chunk does not clone its sections.
Each load updates a monotonically increasing LRU age. At a window change the
worker evicts least-recently-used chunks outside the pinned working set until its
estimated chunk-payload budget is met. Empty region tables are removed too.

ImGui's **CPU chunk cache (MiB)** and `--cache-mb N` select an 8–1024 MiB budget,
default 128 MiB. This accounts for chunk/section payloads, not all process/driver
memory or GPU buffers. The active window stays pinned even when its payload is
larger than the budget; the overlay reports both pinned and cached usage. The
edit journal is retained separately so eviction and procedural regeneration do
not erase player edits. There is still no disk persistence. Transient active and
pending snapshots can temporarily exceed the cache estimate.

## Asynchronous world replacement

`Streamer` owns one preparation thread with a persistent CPU cache. A request
contains generation, center, radius, budget, edit revision data and animation time.
The worker:

1. Applies edits, generates missing chunks, updates the LRU and evicts excess entries.
2. Prepares the dense voxel window, occupancy hierarchy and deterministic decorations.
3. Allocates new GPU terrain/mask buffers and a complete mesh/AS slot.
4. Copies terrain/metadata and builds the slot's acceleration structures.
5. Signals a completion semaphore and publishes the result after its worker fence.

The render thread polls without waiting. It adopts matching completed generations
at a frame boundary, binds their descriptors, and waits on the producer semaphore
in its GPU submission. Obsolete generations are retired instead. Old CPU/GPU
windows and AS slots return to the worker for recycling/destruction. There is at
most one world request and one animation request outstanding; desired changes
coalesce while a request runs. Main-thread collision uses the active CPU world;
movement into an unavailable chunk is held until residency catches up.

Terrain staging and GPU storage use one byte per material ID. Shaders unpack
IDs from 32-bit words; no additional 8-bit Vulkan feature is required. The
exact window is capped at 19×19 chunks including the guard band, using
33.84 MiB per voxel window. Distant terrain uses separate sparse LOD pages. Tree ownership metadata is cached with chunks and
included in CPU cache estimates; tree deletion is journaled for every member
before a replacement generation is requested.

The near path replaces a **bounded dense window**. The separate terrain LOD
worker uploads distant sparse pages progressively; see [Terrain LOD](TERRAIN_LOD.md). It does not use camera visibility as an I/O
trigger, preserving off-screen shadows and avoiding camera-turn upload churn.
Terrain buffers retain one retired allocation for reuse, and staging buffers grow
to the largest requested window instead of being mapped/allocated every crossing.
The old window can remain visible during a pending request. Teleporting farther
than its guard band can temporarily reveal missing terrain. Normal walking gives
the worker the guard-band interval to catch up.

## Three-slot AS/geometry ring

Each mesh slot owns shader object/triangle buffers, local build positions,
instance transforms, BLAS/TLAS storage, scratch and its scene pose. The worker
updates a free slot; the renderer keeps the latest completed slot if the new one
is not ready. Older TLASes are never paired with newer vertex/instance buffers.
The whole fish pose can lag, including its primary image and shadow. A slot is
recycled only after its previous render use completes. Static foliage BLASes
build once per slot/generation; fish refit, or rebuild when the debug option is on.

BLAS storage and scratch are pooled into two aligned buffers per slot, with
non-overlapping slices for every structure. Independent BLAS builds/refits are
recorded in `vkCmdBuildAccelerationStructuresKHR` batches; a barrier orders
those before the TLAS update, followed by a barrier for shader queries. TLAS
storage/scratch are separate. This avoids hundreds of Vulkan memory allocations
and build commands for each new window. The worker submits at most 16 BLASes
per command buffer, so a large new window does not produce one long driver
submission. Layouts/counts cannot change during a
refit; a world generation creates new slots instead.

The worker prefers a second, lower-priority queue in the same compute/graphics
family. The render and worker queues have separate host locks. Single-queue
hardware shares a lock for submissions; GPU work can serialize there. Device-idle
operations such as swapchain recreation synchronize both queues. Resize/shutdown
can still wait; normal streaming does not call device-idle on the render thread.
The render frame itself still has one completion fence; graphics frames are not
triple-buffered by this AS ring.

## Measurements and limitations

ImGui reports chunk/section frustum counts, cache/pinned payload, pending world
state, AS snapshot age, worker preparation time and actual worker GPU BLAS/TLAS
timestamps. Render GPU total excludes concurrent worker build time. Logs report
frame-time percentiles, main-thread streaming cost and maximum CPU acquire,
prepare, submit/present and render-fence times. Timestamp reads are small profiler
queries; displayed images never use CPU readback.

Asynchrony removes explicit render-thread generation/upload/build waits. It
cannot guarantee zero stalls: driver allocations/submission, shared GPU bandwidth,
non-preemptible GPU work, presentation and OS scheduling can still cause long
frames. Pooled allocation/batched builds reduce those costs, but this remains a
study renderer rather than a production sparse streaming allocator.

Use `BLOCKWORLD_VALIDATION=1 BLOCKWORLD_SYNC_VALIDATION=1` for Khronos
synchronization validation. The smoke suite checks cache eviction and edit replay,
completed-generation adoption, old-snapshot reuse, ring wraparound, refit/rebuild,
and AS versus software triangle intersection at visible fish poses.

Relevant Vulkan rules: [acceleration structure builds, updates and synchronization](https://docs.vulkan.org/spec/latest/chapters/accelstructures.html).
