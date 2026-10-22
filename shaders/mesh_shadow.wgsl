@group(0) @binding(4) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read> hits:array<Hit>;
@group(0) @binding(11) var<storage,read_write> mesh_shadows:array<vec2<f32>>;
fn shadow(point:vec3<f32>,normal:vec3<f32>)->vec2<f32> {
    if p.effects.w<0.5 || dot(normal,p.sun.xyz)<=0.0 {return vec2<f32>(1.0,15.0);}
    let blend=shadow_blend(point);
    if blend>=1.0 {return vec2<f32>(far_sunlight(point,normal,true),15.0);}
    var exact=0.0;var mask=0u;
    if penumbra(point,normal,true) || penumbra(point,normal,false) {
        let samples=u32(clamp(p.soft.z,2.0,4.0));
        for(var sample=0u;sample<samples;sample++) {let hit=mesh_trace(point+normal*0.003,disk_direction(point,sample),p.up.w);let lit=hit.flags==0u;exact+=select(0.0,1.0,lit);if lit {mask|=1u<<sample;}}
        exact/=f32(samples);mask|=16u;
    } else {
        if p.soft.y<0.5 && cascade_clear(point+normal*0.003,true) {return vec2<f32>(1.0,15.0);}
        var direction=p.sun.xyz;if p.soft.y>0.5 {direction=disk_direction(point,0u);}
        let hit=mesh_trace(point+normal*0.003,direction,p.up.w);exact=select(1.0,0.0,hit.flags!=0u);mask=select(0u,15u,hit.flags==0u);
    }
    if blend<=0.0 {return vec2<f32>(exact,f32(mask));}
    return vec2<f32>(mix(exact,far_sunlight(point,normal,true),blend),f32(mask));
}
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=p.screen.x || id.y>=p.screen.y {return;}
    let i=id.x+p.screen.x*id.y;
    if pixel_flags(i)==0u {mesh_shadows[i]=vec2<f32>(1.0);return;}
    let h=hits[i];var result=vec2<f32>(1.0);
    if h.valid!=0u {let sampled=shadow(point_at(id.xy,h.normal_depth.w),h.normal_depth.xyz);result.x=sampled.x;shadow_raw[i].x=sampled.y;}
    if h.water.y>0.5 {let sampled=shadow(point_at(id.xy,h.water_normal_depth.w),h.water_normal_depth.xyz);result.y=sampled.x;shadow_raw[i].w=sampled.y;}
    mesh_shadows[i]=result;
}
