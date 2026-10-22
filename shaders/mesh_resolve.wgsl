@group(0) @binding(1) var<storage,read_write> hits:array<Hit>;
@group(0) @binding(4) var<uniform> p:Params;
@group(0) @binding(10) var<storage,read> mesh_hits:array<MeshHit>;
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=p.screen.x || id.y>=p.screen.y || p.meshes.x==0u {return;}
    let i=id.x+p.screen.x*id.y;
    // Unwritten misses retain old storage; only this frame's hits may resolve.
    if aux[i]!=(p.screen.w>>1u) {return;}
    let mesh=mesh_hits[i];
    if mesh.flags==0u || mesh.normal_depth.w>=hits[i].normal_depth.w {return;}
    // Recompute only accepted foreground pixels so water attenuation stops at the fish.
    var h=hits[i];
    if h.water.y>0.5 {h=trace(p.eye.xyz,ray_at(vec2<f32>(id.xy)),mesh.normal_depth.w,true);}
    h.normal_depth=mesh.normal_depth;h.uv=mesh.uv;h.material=mesh.material;h.valid=mesh.flags;
    hits[i]=h;store_pixel_flags(i,h);store_linear_depth(i,h,ray_at(vec2<f32>(id.xy)));
}
