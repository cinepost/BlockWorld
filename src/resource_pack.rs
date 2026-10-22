//! Minecraft ZIP textures -> RGBA8 frames -> padded guillotiere atlas.
//! Models/blockstates are deliberately not interpreted by this initial loader.
use crate::{config::TEXTURE_PIXELS, meshes::MATERIALS};
use anyhow::{ensure, Context, Result};
use guillotiere::{size2, AtlasAllocator};
use image::ImageDecoder;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Cursor, Read},
    path::Path,
};
pub const EDGE: u32 = 16;
pub const PAD: u32 = 1;
pub const RECORD_WORDS: usize = 12;
pub const METADATA_WORDS: usize = MATERIALS * 6 * RECORD_WORDS;
pub const DESCRIPTORS: u32 = 8;
const PREFIX: &str = "assets/minecraft/textures/block/";
const MAX_ENTRY: u64 = 8 * 1024 * 1024;
const MAX_FRAMES: usize = 16384;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
}
#[derive(Clone, Debug)]
pub struct Frame {
    pub index: usize,
    pub ticks: u32,
}
#[derive(Clone, Debug)]
pub struct Sprite {
    pub pixels: Vec<Vec<u8>>,
    pub rects: Vec<Rect>,
    pub averages: Vec<[f32;3]>,
    pub timeline: Vec<Frame>,
    pub interpolate: bool,
}
impl Sprite {
    pub fn sample(&self, seconds: f64) -> (usize, usize, f32) {
        let total: u64 = self.timeline.iter().map(|f| f.ticks as u64).sum();
        let tick = if seconds.is_finite() {
            (seconds.max(0.0) * 20.0).rem_euclid(total as f64)
        } else {
            0.0
        };
        let mut start = 0.0;
        for (i, frame) in self.timeline.iter().enumerate() {
            if tick < start + frame.ticks as f64 {
                return (
                    frame.index,
                    self.timeline[(i + 1) % self.timeline.len()].index,
                    if self.interpolate {
                        ((tick - start) / frame.ticks as f64) as f32
                    } else {
                        0.0
                    },
                );
            }
            start += frame.ticks as f64;
        }
        (self.timeline[0].index, self.timeline[0].index, 0.0)
    }
}
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub name: String,
    pub imported: usize,
    pub skipped: usize,
    pub animated: usize,
    pub frames: usize,
    pub blockstates: usize,
    pub warnings: Vec<String>,
}
#[derive(Clone)]
struct Face {
    sprite: String,
    tint: [f32; 3],
    flags: u32,
}
pub struct Pack {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub sprites: BTreeMap<String, Sprite>,
    pub stats: Stats,
    faces: Vec<Face>,
}
impl Pack {
    pub fn generated(fallback: &[u32]) -> Result<Self> {
        Self::build(None, fallback)
    }
    pub fn open(path: &Path, fallback: &[u32]) -> Result<Self> {
        Self::build(Some(path), fallback)
    }
    fn build(path: Option<&Path>, fallback: &[u32]) -> Result<Self> {
        ensure!(
            fallback.len() == MATERIALS * TEXTURE_PIXELS,
            "Invalid fallback texture table"
        );
        let mut sprites = BTreeMap::new();
        for material in 0..MATERIALS {
            let pixels = fallback[material * TEXTURE_PIXELS..(material + 1) * TEXTURE_PIXELS]
                .iter()
                .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8])
                .collect();
            sprites.insert(
                format!("__fallback_{material}"),
                Sprite {
                    pixels: vec![pixels],
                    rects: vec![],
                    averages: vec![],
                    timeline: vec![Frame { index: 0, ticks: 1 }],
                    interpolate: false,
                },
            );
        }
        let mut stats = Stats {
            name: "Generated textures".into(),
            ..Stats::default()
        };
        if let Some(path) = path {
            stats.name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let mut zip = zip::ZipArchive::new(
                File::open(path)
                    .with_context(|| format!("Open resource pack {}", path.display()))?,
            )
            .context("Read resource-pack ZIP directory")?;
            ensure!(zip.len() <= 50000, "Resource pack has too many ZIP entries");
            let mut names: Vec<String> = zip
                .file_names()
                .filter(|n| {
                    n.starts_with(PREFIX) && n.ends_with(".png") && !n[PREFIX.len()..].contains('/')
                })
                .map(str::to_owned)
                .collect();
            names.sort();
            names.dedup();
            stats.blockstates = zip
                .file_names()
                .filter(|n| n.starts_with("assets/minecraft/blockstates/") && n.ends_with(".json"))
                .count();
            for name in names {
                let short = name[PREFIX.len()..name.len() - 4].to_string();
                let result = (|| {
                    let bytes = read_entry(&mut zip, &name, MAX_ENTRY)?;
                    let meta_name = format!("{name}.mcmeta");
                    let meta = if zip.index_for_name(&meta_name).is_some() {
                        Some(read_entry(&mut zip, &meta_name, 256 * 1024)?)
                    } else {
                        None
                    };
                    decode(&short, &bytes, meta.as_deref())
                })();
                match result {
                    Ok(sprite) => {
                        stats.imported += 1;
                        stats.animated += usize::from(sprite.timeline.len() > 1);
                        sprites.insert(short, sprite);
                    }
                    Err(e) => {
                        stats.skipped += 1;
                        if stats.warnings.len() < 32 {
                            stats.warnings.push(format!("{short}: {e:#}"));
                        }
                    }
                }
                ensure!(
                    sprites.values().map(|s| s.pixels.len()).sum::<usize>() <= MAX_FRAMES,
                    "Resource pack exceeds {MAX_FRAMES} decoded frames"
                );
            }
            ensure!(
                stats.imported > 0,
                "No supported 16x16 block PNG textures in {} ({} skipped)",
                path.display(),
                stats.skipped
            );
            apply_biome_tints(&mut sprites);
        }
        let mut faces = Vec::new();
        let candidates: [&[&str]; MATERIALS] = [
            &["stone"],
            &["grass_block_side"],
            &["dirt"],
            &["sand"],
            &["water_still", "water_flow"],
            &["oak_log"],
            &[],
            &[],
            &["short_grass", "grass"],
            &["fern"],
            &["oak_sapling"],
            &["snow"],
            &["oak_leaves"],
            &["spruce_leaves"],
            &["gravel"],
            &["sandstone"],
            &["granite"],
            &["grass_block_side"],
            &["podzol_side"],
        ];
        for (m, names) in candidates.iter().enumerate() {
            for face in 0..6 {
                let special = if (m == 1 || m == 17) && face == 2 {
                    Some("grass_block_top")
                } else if (m == 1 || m == 17 || m == 18) && face == 3 {
                    Some("dirt")
                } else if m == 5 && (face == 2 || face == 3) {
                    Some("oak_log_top")
                } else if m==18 && face==2 {
                    Some("podzol_top")
                } else if m==15 && (face==2 || face==3) {
                    Some("sandstone_top")
                } else {
                    None
                };
                let chosen = if let Some(special)=special {
                    sprites.contains_key(special).then_some(special)
                } else {names.iter().copied().find(|s| sprites.contains_key(*s))};
                let mut tint = [1.0; 3];
                if chosen.is_none() && (m == 1 || m==17) && face != 2 {
                    tint = [0.85, 0.78, 0.55];
                }
                let flags =
                    u32::from(matches!(m, 8..=10 | 12 | 13)) | if chosen.is_some() { 2 } else { 0 };
                faces.push(Face {
                    sprite: chosen
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("__fallback_{m}")),
                    tint,
                    flags,
                });
            }
        }
        let frames = sprites.values().map(|s| s.pixels.len()).sum::<usize>();
        stats.frames = frames;
        let mut edge = 64;
        let positions = loop {
            let mut allocator = AtlasAllocator::new(size2(edge, edge));
            let mut positions = Vec::with_capacity(frames);
            for _ in 0..frames {
                let Some(a) =
                    allocator.allocate(size2((EDGE + 2 * PAD) as i32, (EDGE + 2 * PAD) as i32))
                else {
                    break;
                };
                positions.push(Rect {
                    x: a.rectangle.min.x as u32 + PAD,
                    y: a.rectangle.min.y as u32 + PAD,
                });
            }
            if positions.len() == frames {
                break positions;
            }
            edge *= 2;
            ensure!(edge <= 4096, "Atlas exceeds 4096x4096 at 16px resolution");
        };
        let mut rgba = vec![0u8; edge as usize * edge as usize * 4];
        let mut slots = positions.into_iter();
        for sprite in sprites.values_mut() {
            for frame in &sprite.pixels {
                let mut sum=[0.0f32;3];let mut weight=0.0f32;
                for texel in frame.as_chunks::<4>().0 {
                    let alpha=texel[3] as f32/255.0;weight+=alpha;
                    for (i,value) in sum.iter_mut().enumerate() {*value+=texel[i] as f32/255.0*alpha;}
                }
                sprite.averages.push(sum.map(|v|v/weight.max(1.0)));
                let rect = slots.next().unwrap();
                for y in -1i32..=16 {
                    for x in -1i32..=16 {
                        let source = (x.clamp(0, 15) + y.clamp(0, 15) * 16) as usize * 4;
                        let target =
                            ((rect.x as i32 + x) + (rect.y as i32 + y) * edge) as usize * 4;
                        rgba[target..target + 4].copy_from_slice(&frame[source..source + 4]);
                    }
                }
                sprite.rects.push(rect);
            }
            // Keep only the master atlas after packing, not a second CPU copy of every frame.
            sprite.pixels.clear();
        }
        Ok(Self {
            width: edge as u32,
            height: edge as u32,
            rgba,
            sprites,
            stats,
            faces,
        })
    }
    pub fn metadata(&self, seconds: f64, linear: bool) -> Vec<u32> {
        let mut data = Vec::with_capacity(METADATA_WORDS);
        for face in &self.faces {
            let sprite = &self.sprites[&face.sprite];
            let (a, b, blend) = sprite.sample(seconds);
            let average: [f32;3]=std::array::from_fn(|i| (sprite.averages[a][i]*(1.0-blend)+sprite.averages[b][i]*blend)*face.tint[i]);
            let rgb=average.map(|v| (v.clamp(0.0,1.0)*255.0).round() as u32);
            let packed=(rgb[0]<<16)|(rgb[1]<<8)|rgb[2];
            let a = sprite.rects[a];
            let b = sprite.rects[b];
            data.extend([
                a.x,
                a.y,
                0,
                face.flags | if linear { 4 } else { 0 },
                b.x,
                b.y,
                0,
                blend.to_bits(),
                face.tint[0].to_bits(),
                face.tint[1].to_bits(),
                face.tint[2].to_bits(),
                packed,
            ]);
        }
        data
    }
}
fn read_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>> {
    let mut entry = zip.by_name(name)?;
    ensure!(entry.size() <= limit, "ZIP entry exceeds byte limit");
    let mut bytes = Vec::new();
    entry.by_ref().take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Expanded ZIP entry exceeds byte limit"
    );
    Ok(bytes)
}
fn decode(name: &str, bytes: &[u8], meta: Option<&[u8]>) -> Result<Sprite> {
    let decoder =
        image::codecs::png::PngDecoder::new(Cursor::new(bytes)).context("Decode PNG header")?;
    let (width, height) = decoder.dimensions();
    ensure!((width==16 || (name.ends_with("_flow") && width==32)) && height>=width && height%width==0 && height/width<=1024,"Expected 16px square/vertical strip (32px flow convention supported), got {width}x{height}");
    ensure!(
        decoder.total_bytes() <= 8 * 1024 * 1024,
        "Decoded PNG exceeds memory limit"
    );
    let rgba = image::DynamicImage::from_decoder(decoder)?.into_rgba8();
    let frame_count = (height / width) as usize;
    let mut timeline = Vec::new();
    let mut interpolate = false;
    if let Some(meta) = meta {
        let value: Value = serde_json::from_slice(meta).context("Parse .png.mcmeta JSON")?;
        if let Some(animation) = value.get("animation") {
            for key in ["width", "height"] {
                if let Some(v) = animation.get(key) {
                    ensure!(
                        v.as_u64() == Some(width as u64),
                        "Unsupported animation frame {key}"
                    );
                }
            }
            let duration = animation
                .get("frametime")
                .map(ticks)
                .transpose()?
                .unwrap_or(1);
            interpolate = animation
                .get("interpolate")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if let Some(frames) = animation.get("frames") {
                let frames = frames
                    .as_array()
                    .context("Animation frames must be an array")?;
                ensure!(
                    !frames.is_empty() && frames.len() <= 4096,
                    "Invalid animation sequence length"
                );
                for frame in frames {
                    let index = frame
                        .as_u64()
                        .or_else(|| frame.get("index").and_then(Value::as_u64))
                        .context("Invalid animation frame index")?;
                    ensure!(
                        index < frame_count as u64,
                        "Animation frame index {index} outside strip"
                    );
                    timeline.push(Frame {
                        index: index as usize,
                        ticks: frame
                            .get("time")
                            .map(ticks)
                            .transpose()?
                            .unwrap_or(duration),
                    });
                }
            } else {
                timeline = (0..frame_count)
                    .map(|index| Frame {
                        index,
                        ticks: duration,
                    })
                    .collect();
            }
        }
    }
    if timeline.is_empty() {
        timeline = (0..frame_count)
            .map(|index| Frame { index, ticks: 1 })
            .collect();
    }
    let pixels = (0..frame_count)
        .map(|i| {
            let frame =
                image::imageops::crop_imm(&rgba, 0, i as u32 * width, width, width).to_image();
            if width == EDGE {
                frame.into_raw()
            } else {
                image::imageops::resize(&frame, EDGE, EDGE, image::imageops::FilterType::Nearest)
                    .into_raw()
            }
        })
        .collect();
    Ok(Sprite {
        pixels,
        rects: vec![],
        averages: vec![],
        timeline,
        interpolate,
    })
}
fn ticks(value: &Value) -> Result<u32> {
    let n = value
        .as_u64()
        .context("Frame time must be a positive integer")?;
    ensure!(
        (1..=1_000_000).contains(&n),
        "Frame time outside 1..1000000 ticks"
    );
    Ok(n as u32)
}
fn apply_biome_tints(sprites: &mut BTreeMap<String, Sprite>) {
    for (name, sprite) in sprites.iter_mut() {
        let tint = match name.as_str() {
            "grass_block_top"
            | "grass_block_side_overlay"
            | "short_grass"
            | "grass"
            | "fern"
            | "oak_leaves" => [0.55, 0.8, 0.32],
            "spruce_leaves" => [0.38, 0.6, 0.42],
            "water_still" | "water_flow" => [0.23, 0.46, 0.9],
            _ => continue,
        };
        let colored = sprite
            .pixels
            .iter()
            .flat_map(|f| f.as_chunks::<4>().0.iter())
            .filter(|p| p[3] > 0)
            .any(|p| p[0].abs_diff(p[1]) > 16 || p[1].abs_diff(p[2]) > 16);
        if !colored {
            for frame in &mut sprite.pixels {
                for pixel in frame.as_chunks_mut::<4>().0.iter_mut() {
                    for c in 0..3 {
                        pixel[c] = (pixel[c] as f32 * tint[c]) as u8;
                    }
                }
            }
        }
    }
    if let Some(overlay) = sprites
        .get("grass_block_side_overlay")
        .and_then(|s| s.pixels.first())
        .cloned()
    {
        if let Some(side) = sprites.get_mut("grass_block_side") {
            for frame in &mut side.pixels {
                for (base, over) in frame
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(overlay.as_chunks::<4>().0.iter())
                {
                    let a = over[3] as u32;
                    for c in 0..3 {
                        base[c] =
                            ((over[c] as u32 * a + base[c] as u32 * (255 - a) + 127) / 255) as u8;
                    }
                    base[3] = base[3].max(over[3]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, RgbaImage};
    use std::io::Write;
    fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let image = RgbaImage::from_pixel(width, height, image::Rgba(color));
        let mut bytes = vec![];
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                image.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        bytes
    }
    fn zip(entries: &[(&str, Vec<u8>)]) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "blockworld-pack-unit-{}-{}.zip",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        for (name, bytes) in entries {
            writer
                .start_file(
                    name,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        path
    }
    #[test]
    fn rgba_padding_copies_all_edges_and_corners_without_neighbor_bleed() {
        let mut im = RgbaImage::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                im.put_pixel(
                    x,
                    y,
                    image::Rgba([
                        (x * 15) as u8,
                        (y * 15) as u8,
                        31,
                        if x < 8 { 0 } else { 255 },
                    ]),
                );
            }
        }
        let mut bytes = vec![];
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(im.as_raw(), 16, 16, image::ExtendedColorType::Rgba8)
            .unwrap();
        let path = zip(&[
            ("assets/minecraft/textures/block/oak_leaves.png", bytes),
            (
                "assets/minecraft/textures/block/stone.png",
                png(16, 16, [0, 0, 255, 255]),
            ),
        ]);
        let p = Pack::open(&path, &crate::renderer::textures()).unwrap();
        std::fs::remove_file(path).unwrap();
        let r = p.sprites["oak_leaves"].rects[0];
        for y in -1i32..=16 {
            for x in -1i32..=16 {
                let i = ((r.x as i32 + x) + (r.y as i32 + y) * p.width as i32) as usize * 4;
                assert_eq!(
                    &p.rgba[i..i + 4],
                    &im.get_pixel(x.clamp(0, 15) as u32, y.clamp(0, 15) as u32).0
                );
            }
        }
    }
    #[test]
    fn animation_order_overrides_tick_durations_and_interpolates_across_wrap() {
        let bytes = png(16, 48, [255, 0, 0, 255]);
        let sprite=decode("stone",&bytes,Some(br#"{"animation":{"frametime":2,"frames":[2,{"index":0,"time":3},1],"interpolate":true}}"#)).unwrap();
        assert_eq!(sprite.sample(0.0), (2, 0, 0.0));
        assert_eq!(sprite.sample(0.05), (2, 0, 0.5));
        assert_eq!(sprite.sample(0.1), (0, 1, 0.0));
        assert_eq!(sprite.sample(0.25), (1, 2, 0.0));
        let (a, b, mix) = sprite.sample(0.325);
        assert_eq!((a, b), (1, 2));
        assert!((mix - 0.75).abs() < 0.00001);
        assert_eq!(sprite.sample(0.35), (2, 0, 0.0));
        for meta in [
            br#"{"animation":{"frametime":0}}"#.as_slice(),
            br#"{"animation":{"frames":[3]}}"#,
            br#"{"animation":{"frames":[]}}"#,
        ] {
            assert!(decode("stone", &bytes, Some(meta)).is_err());
        }
    }
    #[test]
    fn zip_filter_partial_fallback_faces_and_unsupported_sizes_are_explicit() {
        let path = zip(&[
            (
                "assets/minecraft/textures/block/stone.png",
                png(16, 16, [255, 0, 0, 255]),
            ),
            (
                "assets/minecraft/textures/block/dirt.png",
                png(16, 16, [93, 55, 12, 255]),
            ),
            (
                "assets/minecraft/textures/block/grass_block_top.png",
                png(16, 16, [0, 200, 0, 255]),
            ),
            (
                "assets/minecraft/textures/block/hd.png",
                png(64, 64, [0, 0, 255, 255]),
            ),
            (
                "assets/minecraft/textures/item/ignored.png",
                png(16, 16, [0, 0, 0, 0]),
            ),
            ("assets/minecraft/blockstates/stone.json", b"{}".to_vec()),
        ]);
        let p = Pack::open(&path, &crate::renderer::textures()).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            (p.stats.imported, p.stats.skipped, p.stats.blockstates),
            (3, 1, 1)
        );
        assert_eq!(p.faces[6 + 2].sprite, "grass_block_top");
        assert_eq!(p.faces[6 + 3].sprite, "dirt");
        assert_eq!(p.faces[3 * 6].sprite, "__fallback_3");
        assert_eq!(p.metadata(0.0, false).len(), METADATA_WORDS);
        let path = zip(&[(
            "assets/minecraft/textures/block/stone.png",
            png(128, 128, [0, 0, 0, 255]),
        )]);
        assert!(Pack::open(&path, &crate::renderer::textures())
            .err()
            .unwrap()
            .to_string()
            .contains("No supported"));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn double_width_flow_frames_downsample_and_missing_metadata_uses_ticks() {
        let sprite = decode("water_flow", &png(32, 96, [20, 40, 60, 255]), None).unwrap();
        assert_eq!(sprite.pixels.len(), 3);
        assert_eq!(sprite.pixels[0].len(), 16 * 16 * 4);
        assert_eq!(sprite.sample(0.05).0, 1);
        assert!(decode("stone", &png(32, 32, [0, 0, 0, 255]), None).is_err());
    }
}
