@group(0) @binding(4) var<uniform> p:Params;
@group(0) @binding(10) var<storage,read_write> mesh_hits:array<MeshHit>;
@group(0) @binding(1) var<storage,read> hits:array<Hit>;
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=p.screen.x || id.y>=p.screen.y {return;}
    let i=id.x+p.screen.x*id.y;
    let limit=hits[i].normal_depth.w;
    var hit=mesh_miss();hit.normal_depth.w=limit;
    if limit>=p.scene_min.w {hit=mesh_trace(p.eye.xyz,ray_at(vec2<f32>(id.xy)),limit);}
    if hit.flags!=0u {aux[i]=p.screen.w>>1u;}
    if hit.flags!=0u || (p.screen.w&1u)!=0u {mesh_hits[i]=hit;}
}
