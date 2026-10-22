@group(0) @binding(4) var<uniform> p:Params;
fn splat(low:vec3<f32>,high:vec3<f32>,polygon:bool) {
    let center=(low+high)*0.5-p.cascade_center.xyz;
    let extent=(high-low)*0.5+0.001;
    let xy=vec2<f32>(dot(center,p.light_right.xyz),dot(center,p.light_up.xyz));
    let span=vec2<f32>(dot(extent,abs(p.light_right.xyz)),dot(extent,abs(p.light_up.xyz)));
    let upper=ordered_depth(dot(center,p.sun.xyz)+dot(extent,abs(p.sun.xyz))+0.001);
    for(var c=0u;c<4u;c++) {
        let r=cascade_radius(c);
        // Rasterize every texel touched by the projected closed AABB, including borders.
        let a=vec2<i32>(floor(((xy-span)/(2.0*r)+0.5)*128.0));
        let b=vec2<i32>(floor(((xy+span)/(2.0*r)+0.5)*128.0));
        if any(b<vec2<i32>(0)) || any(a>vec2<i32>(127)) {continue;}
        let lo=max(a,vec2<i32>(0));let hi=min(b,vec2<i32>(127));
        for(var y=lo.y;y<=hi.y;y++) {for(var x=lo.x;x<=hi.x;x++) {
            let index=u32(x+128*y)+16384u*(c+select(0u,4u,polygon));
            atomicMax(&shadow_cascades[index],upper);
        }}
    }
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if dispatch.x==1u {
        if id.x>=p.meshes.x {return;}
        let object=objects[id.x];
        splat(object.low.xyz-1.0,object.high.xyz+1.0,true);return;
    }
    if dispatch.x==2u {
        if id.x>=__LOD_PAGE_COUNT__u {return;}
        let slot=8u+id.x*8u;if lod_pages[slot+6u]==0u {return;}
        let low=vec3<f32>(f32(i32(lod_pages[slot])*16),-64.0+f32(lod_pages[slot+4u]*lod_pages[slot+3u]),f32(i32(lod_pages[slot+1u])*16));
        let high=vec3<f32>(low.x+16.0,-64.0+f32(lod_pages[slot+5u]*lod_pages[slot+3u]),low.z+16.0);
        if high.y>low.y {splat(low,high,false);}return;
    }
    let side=p.dims.x/16u;let count=side*side*(p.dims.y/16u);
    if id.x>=count {return;}
    let masks=sections[id.x].opaque;
    if (masks.x|masks.y)==0u {return;}
    let section=vec3<u32>(id.x%side,id.x/(side*side),(id.x/side)%side);
    let base=vec3<f32>(p.origin.xyz)+vec3<f32>(section*16u);
    for(var brick=0u;brick<64u;brick++) {
        if (masks[brick/32u]&(1u<<(brick%32u)))==0u {continue;}
        let low=base+vec3<f32>(vec3<u32>(brick%4u,brick/16u,(brick/4u)%4u)*4u);
        splat(low,low+4.0,false);
    }
}
