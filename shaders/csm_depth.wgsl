@group(0) @binding(4) var<uniform> p:Params;
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=p.sky_state.w || id.y>=p.sky_state.w || id.z>=4u {return;}
    let origin=cascade_ray_origin(id.xy,id.z);var key=0u;
    if dispatch.x==0u {
        let hit=trace(origin,-p.sun.xyz,p.shadow_controls.w,false);
        if hit.valid!=0u {key=ordered_depth(p.shadow_controls.z-hit.normal_depth.w);}
    } else if p.meshes.x!=0u {
        let hit=mesh_trace(origin,-p.sun.xyz,p.shadow_controls.w);
        if hit.flags!=0u {key=ordered_depth(p.shadow_controls.z-hit.normal_depth.w);}
    }
    far_depth[id.x+p.sky_state.w*id.y+(p.sky_state.w*p.sky_state.w)*(id.z+dispatch.x*4u)]=key;
}
