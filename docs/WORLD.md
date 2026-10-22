# Terrain and generated collections

Geometry constants remain in `src/config.rs`: 16×16 columns per chunk, 24 sections
of 16³ blocks, four sections below Y=0, and one metre per block. Region slot arrays
remain compile-time-sized. Subchunks still occupy exactly 4096 bytes aligned to 64
bytes. Material IDs are byte-sized on both CPU and GPU; GPU shaders extract a byte
from a 32-bit storage-buffer word, without requiring 8-bit storage extensions.

The default view radius is 8 chunks / 128 m. The slider and `--view-chunks` accept
up to 64 / 1,024 m. The exact near window remains at most 19×19 chunks including
the guard band, using 33.84 MiB of voxel storage. Separate sparse LOD pages cover
the distant world in a 64 MiB GPU budget, with progressive uploads and their own
worker/cache. See [Terrain LOD](TERRAIN_LOD.md) for selection and limitations.

Terrain combines smooth 192 m hills, 384 m valleys, a 640 m mountain envelope and
180 m ridges. Snow covers terrain at Y≥110. The dry-land spawn search extends to
256 m, so broader valleys do not silently spawn the player in water. Use:

```sh
cargo run --release -- --mountain-view
cargo run --release -- --view-chunks 64 --mountain-view
```

The mountain fixture finds a snowy ridge and positions the player above a nearby
slope, looking toward it. The water profiling camera remains `--water-view`.

## Trees are collections

`TreeId` is the tree's global candidate-grid coordinate. `Tree` stores kind, root
and its complete list of `(world position, block)` members. Oak trees have a broad
rounded crown and shorter trunk; pines have taller trunks and tapered crowns.
Higher elevations prefer pine. Leaves and needles currently use solid voxel
surfaces, while the existing polygon foliage keeps alpha-tested textures.

Each chunk stores tree descriptors and a voxel-index-to-tree membership map.
Generation enumerates nearby candidate cells too, so a canopy crossing a chunk
or region edge has the same identity and members regardless of loading order.
Candidate spacing prevents different trees sharing blocks. Chunk snapshots share
immutable descriptors through `Arc`; metadata follows the chunk cache.

`World::destroy` checks explicit membership, not connectivity or material. Breaking
any generated member writes Air edits for the entire collection before requesting
a new asynchronous GPU generation. Member coordinates outside current residency
are included. The existing edit journal replays those removals when evicted chunks
regenerate, including on the streaming worker. There is no disk persistence yet.

`World::set` detaches edited cells from collection ownership. A player-placed block
beside, or replacing part of, a generated tree is independent and survives later
tree removal. Ordinary wood blocks therefore do not acquire accidental tree
membership. Tree descriptors remain immutable; membership maps track live cells.
The HUD reports the targeted tree kind, ID, root and original member count.

F11 toggles the developer HUD. Key 7 selects snow; the HUD material selector also
exposes oak leaves and pine needles. All atlas entries are 16×16 RGBA texels; the
64×64 mipmapped, anisotropically filtered water wave maps are separate resources.

Biome identities, current climate/terrain fields, caves and biome-specific blocks are documented in [BIOMES.md](BIOMES.md). These replace the earlier generator description above; the block/chunk layout and tree ownership rules remain in use.
