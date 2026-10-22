# Deferred entity activation

`src/entities.rs` owns activation, swimming movement and skeletal animation.
`src/meshes.rs` owns geometry, bind-pose assets and mesh construction. The existing
fish are the first NPC implementation; birds, animals and villagers can add
behavior/rig dispatch here later. No new species or pathfinding are introduced.
Foliage remains a static world decoration, not an NPC.

Active players are `ActivePlayer { id, position }` entries in the renderer's
player list. ID 0 is the local player and its position is updated from player
feet; additional IDs are preserved. Activation uses the union of all
player-centered 64×64×64 metre boxes (±32 m on each axis), with duplicate
candidates removed. Selection considers an entity's position, not camera frustum
or terrain view distance. Remote terrain streaming/network replication is not
implemented by this list.

An entity spatial hash uses signed 16 m cells. Its catalog is built when a mesh
world generation is prepared. Queries cache candidate indices and run initially,
on player-list membership changes, or at most once per second when positions
have changed since the last query. Stationary players do not trigger periodic
queries. Fish are indexed at their anchors with a conservative swimming-orbit
margin. Future freely roaming NPCs must migrate spatial buckets as they move.
Every completed animation snapshot performs the exact current-box test on its
candidates; inactive fish do not swim, skin, recalculate bounds or refit BLAS.

Runtime updates run on `world-upload-as`, separate from the render thread, using
the existing bounded outstanding-animation request and three-slot immutable
geometry/AS ring. The entity unit has no Vulkan ownership. World preparation and
entity updates currently share that worker, so heavy world preparation can delay
an entity snapshot; rendering continues using the previous complete snapshot.
Outside entities retain dormant rig/geometry storage to keep stable object/BLAS
indices. Their TLAS instance masks are zero and software tracing rejects them.
The current GPU player list is also checked by both polygon ray paths, rejecting
out-of-volume entities even while an older snapshot is in use. Activation changes
invalidate the cached polygon shadow classification. The GPU player buffer grows
as needed and adds one storage-buffer descriptor (22 total).

The one-second schedule is broad-phase candidate maintenance, not a one-Hz
animation rate. Active animation continues at the deferred snapshot cadence.
Newly entered volumes can wait up to one query interval plus worker completion;
leaving a volume is rejected immediately in rendering. Dormant fish resume their
deterministic time-based patrol when reactivated. Persistent AI state and save
serialization are future work.

CPU tests cover stationary/throttled queries, inactive rig preservation, signed
coordinates, vertical/corner bounds, multi-player union and membership changes.
The Vulkan smoke suite checks active/inactive geometry on both polygon backends,
plus asynchronous world uploads and the immutable AS ring. The streaming fixture
uses an additional active player near its fish to exercise the multiplayer union.
