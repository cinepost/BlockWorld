# Implemented biomes

The generator currently implements these seven biomes. Each generated chunk
retains one byte of biome identity per X/Z column. `World::biome_at` returns
that stable identity after block edits; unloaded queries evaluate the same
stateless climate fields without loading chunks. F11 shows biome, temperature,
humidity, erosion and roughness at the player's position.

| Biome | Selection / terrain | Surface and vegetation |
|---|---|---|
| Ocean | Seabed below sea level, up to 24 m deep | Sand near shores; gravel deeper down, water up to sea level; no trees |
| Deep ocean | Seabed more than 24 m below sea level | Gravel over stone, deep water; no trees |
| Meadow | Moderately warm, moderately humid elevated land | Meadow grass over dirt; sparse oak/pine trees and existing grass/fern cards |
| Jagged peaks | Cold climate and height above 85 m | Granite near the surface, snow caps above Y=110; no generated trees |
| Forest | High humidity inland, excluding desert/peaks | Podzol over dirt; denser oak/pine stands, with pine favored in colder/higher areas |
| Plains | Temperate/dry transition land and remaining lowlands | Grass over dirt; sparse trees, grass/fern cards, sandy shore margins |
| Desert | Temperature above 0.66, humidity below 0.40, height below 90 m | Sand over sandstone; no trees or grass cards |

Temperature and humidity are independent, smoothly interpolated multiscale
noise maps. Temperature combines 1024/256 m fields; humidity combines 896/256 m
fields. Cold peaks require temperature below 0.58, leaving a temperate climate
range between snowy peaks and hot deserts. Ocean/deep-ocean selection also uses
actual seabed height, and meadow/peaks use elevation. Terrain height does not
switch abruptly when a categorical biome ID changes. Surface materials and
vegetation still change discretely at block boundaries; full biome color-map
blending and transitional plant species are not implemented.

## New blocks and resource packs

Existing material IDs remain unchanged, including polygon slots 7–11.

| ID | Block | Primary use | Minecraft resource-pack texture |
|---|---|---|---|
| 15 | Gravel | Ocean/deep-ocean bed | `gravel.png` |
| 16 | Sandstone | Desert subsurface | `sandstone.png`, `sandstone_top.png` |
| 17 | Granite | Jagged peaks | `granite.png` |
| 18 | Meadow grass | Meadow surface | `grass_block_top.png`, `grass_block_side.png`, `dirt.png` |
| 19 | Podzol | Forest surface | `podzol_top.png`, `podzol_side.png`, `dirt.png` |

All five are solid voxel blocks, selectable in the ImGui material selector.
Generated 16×16 fallback textures work without a resource pack. Material tables,
texture metadata, GPU rendering and SVO palettes/mip aggregation include the new
IDs. Pack-based grass uses the existing fixed vegetation tint; arbitrary pack
biome colormaps are not parsed yet.

## Terrain fields

Continuous continentalness controls the broad base height and deep ocean basins.
An erosion field reduces hill amplitude, cuts smooth valleys and suppresses
mountain uplift; a roughness field controls finer relief. Cold, continental,
low-erosion areas receive ridged mountain uplift. All fields use global world
coordinates, including negative coordinates, and share the same seed in the
normal terrain and LOD workers.

`src/terrain.rs::SETTINGS` exposes compile-time base-height offset, erosion
strength and roughness strength adjustments. The erosion is a procedural
terrain-shaping model, not a hydraulic or thermal erosion simulation. World
height remains -64 through 319 m, with terrain clamped away from the top/bottom.
Default spawning searches a broader land radius and avoids cave entrances.

## Caves and cave networks

`src/caves.rs` evaluates shared 3D density fields on a world-aligned 4 m lattice
and interpolates them for block carving. Intersections of two smooth noise bands
form branching tunnels; a third field adds chambers. Neighbor chunks sample the
same border lattice, so they do not need to load each other to connect caves.
The generation remains deterministic under asynchronous/progressive loading.

The lowest four block layers stay solid. Normally at least four blocks of roof
are preserved; sparse dry-land entrance regions allow caves to open at the
surface. Ocean roofs remain sealed, and selected deep regions receive water
below Y=-24 as a simple aquifer approximation. There is no water-flow simulation,
guarantee that every chamber connects to an entrance, or explicit dungeon graph.
Caves are carved into normal block data, so collisions, edits, occupancy
hierarchies, shadows and SVOs use the same geometry.

Trees use biome-dependent density and climate-dependent oak/pine selection.
Existing stable tree identities and whole-tree removal behavior are retained.
The SVO generator cache version is now **3**, separating these worlds from the
previous generator. Disk persistence remains a future feature.

## Inspection

```sh
./target/release/blockworld --biome desert --fly
./target/release/blockworld --biome deep-ocean --fly
./target/release/blockworld --biome jagged-peaks --fly
./target/release/blockworld --biome meadow --fly
```

`--biome` accepts the names in the table (spaces may be quoted, or replaced by
hyphens). It finds the nearest sampled matching column within an 8192 m search
radius and starts above it. `--noisy-world` remains the separate, intentionally
incompressible stress generator and does not use biome terrain or caves.
