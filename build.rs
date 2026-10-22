#[allow(dead_code)]
#[path="src/config.rs"] mod config;
use std::{env, fs, path::PathBuf};
fn main() {
    for name in [
        "loading",
        "visibility",
        "ao",
        "shade",
        "mesh_visibility",
        "mesh_shadow",
        "mesh_resolve",
        "water",
        "sky",
        "csm",
        "csm_depth",
        "shadow_temporal",
        "shadow_filter",
        "atmosphere",
        "sky_importance",
        "vxgi",
        "caustics",
    ] {
        let path = format!("shaders/{name}.wgsl");
        println!("cargo:rerun-if-changed={path}");
        let mut source = if name == "loading" {
            String::new()
        } else {
            fs::read_to_string("shaders/common.wgsl").unwrap()
        };
        println!("cargo:rerun-if-changed=shaders/common.wgsl");
        if name != "loading" {
            source.push_str(&fs::read_to_string("shaders/pack_textures.wgsl").unwrap());
        }
        println!("cargo:rerun-if-changed=shaders/pack_textures.wgsl");
        if name == "atmosphere" || name == "sky_importance" {
            source = source.replace(
                "var<storage,read> sky_cache:array<vec4<f32>>;",
                "var<storage,read_write> sky_cache:array<vec4<f32>>;",
            );
        }
        if name == "csm_depth" {
            source = source.replace(
                "var<storage,read> far_depth:array<u32>;",
                "var<storage,read_write> far_depth:array<u32>;",
            );
        }
        if name == "csm" {
            // Atomic writes during construction; ordinary cached reads in all consumers.
            source = source
                .replace(
                    "var<storage,read> shadow_cascades:array<u32>;",
                    "var<storage,read_write> shadow_cascades:array<atomic<u32>>;",
                )
                .replace(
                    "return shadow_cascades[i];",
                    "return atomicLoad(&shadow_cascades[i]);",
                );
        }

        if name == "caustics" || name == "shadow_filter" || name == "vxgi"
            || name == "visibility"
            || name == "shade"
            || name == "mesh_resolve"
            || name == "water"
            || name == "csm"
            || name == "csm_depth"
        {
            source.push_str(&fs::read_to_string("shaders/traversal.wgsl").unwrap());
        }
        println!("cargo:rerun-if-changed=shaders/traversal.wgsl");
        if name != "ao" && name != "loading" {
            source.push_str(&fs::read_to_string("shaders/meshes.wgsl").unwrap());
        }
        println!("cargo:rerun-if-changed=shaders/meshes.wgsl");
        if name=="vxgi" || name=="shade" {source.push_str(&fs::read_to_string("shaders/gi_trace.wgsl").unwrap());}
        println!("cargo:rerun-if-changed=shaders/gi_trace.wgsl");
        if name=="water" || name=="caustics" {
            source.push_str(&fs::read_to_string("shaders/wave_field.wgsl").unwrap());
        }
        println!("cargo:rerun-if-changed=shaders/wave_field.wgsl");
        if name=="water" || name=="shade" || name=="shadow_filter" {
            source.push_str(&fs::read_to_string("shaders/caustic_lookup.wgsl").unwrap());
        }
        println!("cargo:rerun-if-changed=shaders/caustic_lookup.wgsl");
        source.push_str(&fs::read_to_string(&path).unwrap());
        if name=="caustics" {
            source=source.replace("var<storage,read> caustic_data:array<u32>;","var<storage,read_write> caustic_data:array<atomic<u32>>;")
                .replace("return caustic_data[i];","return atomicLoad(&caustic_data[i]);");
        }
        let page_side=((config::MAX_VIEW_CHUNKS as u32*2+3).next_power_of_two()).to_string();
        source=source.replace("__LOD_PAGE_SIDE__",&page_side).replace("__LOD_PAGE_COUNT__",&((config::MAX_VIEW_CHUNKS as u32*2+3).next_power_of_two().pow(2)).to_string());
        println!("cargo:rerun-if-changed=src/config.rs");
        for single in [false, true] {
            let source = if single {
                source
                    .replace("binding_array<texture_2d<f32>,8>", "texture_2d<f32>")
                    .replace("pack_images[page]", "pack_images")
            } else {
                source.clone()
            };
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap();
            let options = naga::back::spv::Options::default();
            let pipeline = naga::back::spv::PipelineOptions {
                shader_stage: naga::ShaderStage::Compute,
                entry_point: "main".into(),
            };
            let mut words =
                naga::back::spv::write_vec(&module, &info, &options, Some(&pipeline)).unwrap();
            if name=="caustics" {strip_workgroup_array_strides(&mut words);}

            let bytes: Vec<u8> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
            fs::write(
                PathBuf::from(env::var("OUT_DIR").unwrap())
                    .join(format!("{name}{}.spv", if single { "_single" } else { "" })),
                bytes,
            )
            .unwrap();
        }
    }
    println!("cargo:rerun-if-changed=shaders/mesh_query.comp");
    println!("cargo:rerun-if-changed=shaders/depth_pyramid.comp");
    for (name, source, shadow) in [
        ("mesh_query", "shaders/mesh_query.comp", false),
        ("mesh_query_shadow", "shaders/mesh_query.comp", true),
        ("mesh_query_csm", "shaders/mesh_query.comp", false),
        ("depth_pyramid", "shaders/depth_pyramid.comp", false),
    ] {
        for single in [false, true] {
            let out = PathBuf::from(env::var("OUT_DIR").unwrap())
                .join(format!("{name}{}.spv", if single { "_single" } else { "" }));
            let mut command = std::process::Command::new("glslc");
            command
                .args(["--target-env=vulkan1.2", "-O", source, "-o"])
                .arg(&out);
            if single {
                command.arg("-DSINGLE_ATLAS=1");
            }
            if shadow {
                command.arg("-DSHADOW=1");
            }
            if name == "mesh_query_csm" {
                command.arg("-DSHADOW_CSM=1");
            }
            let output = command
                .output()
                .expect("Install glslc (shaderc tools) to compile hardware ray-query shaders");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

// Naga 24 decorates all WGSL arrays with ArrayStride, including workgroup-only
// arrays. Vulkan forbids that explicit layout without the corresponding workgroup
// layout extension. The caustic tile has its own array type, never a storage-buffer
// type: remove only its Workgroup-pointee ArrayStride decoration. Storage-buffer
// layouts and strides must remain unchanged. No driver extension is needed.
fn strip_workgroup_array_strides(words:&mut Vec<u32>) {
    let mut pointees=Vec::new();let mut i=5;
    while i<words.len() {
        let count=(words[i]>>16) as usize;let op=words[i]&0xffff;
        if op==32 && words[i+2]==4 {pointees.push(words[i+3]);}
        i+=count;
    }
    let mut result=words[..5].to_vec();i=5;
    while i<words.len() {
        let count=(words[i]>>16) as usize;let op=words[i]&0xffff;
        if !(op==71 && words[i+2]==6 && pointees.contains(&words[i+1])) {
            result.extend_from_slice(&words[i..i+count]);
        }
        i+=count;
    }
    *words=result;
}
