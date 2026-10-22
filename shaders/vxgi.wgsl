// Experimental one-bounce voxel cone sampling. No feedback from last frame's
// shaded image: history filters indirect irradiance, never recursively adds it.
@group(0) @binding(4) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read> hits:array<Hit>;
fn gi_environment(direction:vec3<f32>)->vec3<f32> {
    if direction.y<0.0 {return vec3<f32>(0.0);}
    if p.sky_state.z==0u {return sky_diffuse(direction);}
    return sky_radiance(direction)*(p.sky_light.w*0.8*3.141592654/22.0);
}
fn gi_source_ambient(n:vec3<f32>)->vec3<f32> {
    if p.sky_state.z==0u {return sky_diffuse(n);}
    var e=vec3<f32>(0.0);let base=p.sky_state.x*SKY_BANK+558464u;
    for(var i=0u;i<9u;i++) {
        let convolution=select(select(0.25,0.666666667,i<4u),1.0,i==0u);
        e+=sky_cache[base+i].xyz*sky_basis(n,i)*convolution;
    }
    // Exclude the old analytic ground bounce from this one-bounce source.
    return max(e,vec3<f32>(0.0))*(p.sky_light.w*0.8*3.141592654/22.0);
}
fn gi_node(cell:vec3<i32>,diameter:f32)->vec3<u32> {
    if cell.y< -64 || cell.y>=320 {return vec3<u32>(0u,16u,0u);}
    let chunk=vec2<i32>(floor(vec2<f32>(cell.xz)/16.0));
    let wrapped=bitcast<vec2<u32>>(chunk)&vec2<u32>(__LOD_PAGE_SIDE__u-1u);
    let index=wrapped.x+__LOD_PAGE_SIDE__u*wrapped.y;let slot=8u+index*8u;
    if lod_pages[slot+6u]==2u && bitcast<i32>(lod_pages[slot])==chunk.x && bitcast<i32>(lod_pages[slot+1u])==chunk.y {
        let merged=lod_pages[8u+__LOD_PAGE_COUNT__u*8u+index*24u+u32(cell.y+64)/16u];
        if p.lod.y>0.5 && (merged>>8u)>16u && f32(merged>>8u)<=diameter {return vec3<u32>(merged&255u,merged>>8u,1u);}
        return svo_cone_cell(lod_pages[slot+2u],cell,lod_pages[slot+3u],diameter);
    }
    if in_world(cell) {return vec3<u32>(block_at(cell),1u,0u);}
    return vec3<u32>(0u,16u,2u);
}
fn gi_cone(start:vec3<f32>,direction:vec3<f32>)->vec3<f32> {
    var t=0.0;var cell=vec3<i32>(floor(start));
    for(var step=0u;step<192u;step++) {
        if t>=p.gi.y {break;}
        let node=gi_node(cell,max(1.0,2.0*t*p.gi.z));
        if node.z==2u {return gi_source_ambient(direction);}
        let material=node.x;let size=max(1u,node.y);
        if material!=0u && material!=5u {
            let low=vec3<f32>(cell&vec3<i32>(~i32(size-1u)));
            let safe=select(vec3<f32>(1.0),direction,abs(direction)>vec3<f32>(1e-20));
            let entry=select(vec3<f32>(-1e20),min((low-start)/safe,(low+f32(size)-start)/safe),abs(direction)>vec3<f32>(1e-20));
            var n=vec3<f32>(0.0,0.0,-sign(direction.z));
            if entry.x>=entry.y && entry.x>=entry.z {n=vec3<f32>(-sign(direction.x),0.0,0.0);}
            else if entry.y>=entry.z {n=vec3<f32>(0.0,-sign(direction.y),0.0);}
            let point=start+direction*max(t,max(entry.x,max(entry.y,entry.z)));
            var h=miss();h.material=material;h.normal_depth=vec4<f32>(n,0.0);h.valid=9u;
            let albedo=hit_color(h,point);var visibility=1.0;
            if p.effects.w>0.5 && p.acceleration.y!=0u {visibility=far_sunlight(point+n*0.02,n,false);}
            return albedo*(gi_source_ambient(n)+direct_light()*max(dot(n,p.sun.xyz),0.0)*visibility);
        }
        let crossing=grid_step(start,direction,cell,vec3<i32>(0),size,t);
        t=crossing.face.w;cell=crossing.cell;
    }
    return gi_environment(direction);
}
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    let size=gi_size();if any(id.xy>=size) {return;}
    let i=id.x+size.x*id.y;let base=gi_base(p.history.y,i);
    let pixel=min(id.xy*2u,p.screen.xy-1u);let h=hits[pixel.x+p.screen.x*pixel.y];
    if (pixel_flags(pixel.x+p.screen.x*pixel.y)&1u)==0u {
        gi_history[base]=vec4<f32>(0.0);gi_history[base+1u]=vec4<f32>(0.0);return;
    }
    let point=point_at(pixel,h.normal_depth.w);let n=h.normal_depth.xyz;
    let tangent=normalize(cross(select(vec3<f32>(0.0,1.0,0.0),vec3<f32>(1.0,0.0,0.0),abs(n.y)>0.9),n));
    let bitangent=cross(n,tangent);
    let seed=shadow_hash(i^shadow_hash(p.history.z));let rotation=f32(seed&65535u)/65536.0*6.283185307;
    var value=vec3<f32>(0.0);
    // Four stratified cosine-weighted directions, rotated each frame.
    for(var cone=0u;cone<4u;cone++) {
        let radius=sqrt((f32(cone)+0.5)/4.0);let phi=rotation+f32(cone)*2.39996323;
        let direction=normalize(tangent*(radius*cos(phi))+bitangent*(radius*sin(phi))+n*sqrt(1.0-radius*radius));
        value+=gi_cone(point+n*0.03,direction)*0.25;
    }
    value=clamp(value,vec3<f32>(0.0),vec3<f32>(32.0));var count=1u;
    let clip=p.previous_vp*vec4<f32>(point,1.0);let uv=vec2<f32>(clip.x,-clip.y)/max(clip.w,0.00001)*0.5+0.5;
    if p.gi.w>0.5 && p.history.x==0u && clip.w>0.0 && all(uv>=vec2<f32>(0.0)) && all(uv<vec2<f32>(1.0)) {
        let q=min(vec2<u32>(uv*vec2<f32>(size)),size-1u);let old=gi_base(1u-p.history.y,q.x+size.x*q.y);
        let surface=gi_history[old];let previous=gi_history[old+1u];let key=bitcast<u32>(surface.w);let stamp=bitcast<u32>(previous.w);
        if (stamp>>8u)==(p.history.w&0x00ffffffu) && (stamp&255u)>0u && (key>>24u)==h.material && dot(unpack_history_normal(key),n)>0.98 && distance(surface.xyz,point)<max(0.12,h.normal_depth.w*0.004) {
            count=min((stamp&255u)+1u,16u);value=mix(previous.xyz,value,1.0/f32(count));
        }
    }
    gi_history[base]=vec4<f32>(point,bitcast<f32>(history_surface(h)));
    gi_history[base+1u]=vec4<f32>(value,bitcast<f32>(((p.history.z&0x00ffffffu)<<8u)|count));
}
