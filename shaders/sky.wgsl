// CPU conservative frustum rejection proves there are no primary surfaces.
// Keep headless visibility/AO/mesh outputs initialized for validation and parity.
@group(0) @binding(4) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read_write> hits:array<Hit>;
@group(0) @binding(2) var<storage,read_write> pixels:array<u32>;
@group(0) @binding(7) var<storage,read_write> ao:array<f32>;
@group(0) @binding(10) var<storage,read_write> mesh_hits:array<MeshHit>;
@group(0) @binding(11) var<storage,read_write> mesh_shadows:array<vec2<f32>>;
@group(0) @binding(14) var<storage,read_write> scene_color:array<vec4<f32>>;
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=p.screen.x || id.y>=p.screen.y {return;}
    let i=id.x+p.screen.x*id.y;
    var color=sky_color(ray_at(vec2<f32>(id.xy)));
    if p.screen.z!=0u {
        color=vec3<f32>(0.0);
        if p.screen.z==2u || p.screen.z==5u || p.screen.z==7u {color=vec3<f32>(1.0);}
        if p.screen.z==8u {color=vec3<f32>(0.0,0.1,0.3);}
        if p.screen.z==9u {color=vec3<f32>(0.2);}
    }
    if (p.screen.w&1u)!=0u {
        store_pixel_flags(i,miss());
        hits[i]=miss();ao[i]=1.0;mesh_hits[i]=mesh_miss();mesh_shadows[i]=vec2<f32>(1.0);
        scene_color[i]=vec4<f32>(color,1.0);
    }
    pixels[i]=pack_display(id.xy,color);
}
