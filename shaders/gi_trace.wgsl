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
fn gi_node(cell:vec3<i32>,diameter:f32)->vec4<u32> {
    if cell.y< -64 || cell.y>=320 {return vec4<u32>(0u,16u,0u,0u);}
    // Exact near data owns cave walls even when a coarse/asynchronously stale
    // SVO page overlaps it. Empty occupancy bricks/sections still accelerate air.
    let q=cell-p.origin.xyz;
    if q.x>=0 && q.z>=0 && q.x<i32(p.dims.x) && q.z<i32(p.dims.z) {
        let material=block_at(cell);if material!=0u {return vec4<u32>(material,1u,0u,0u);}
        let local=vec3<u32>(q);let side=p.dims.x/16u;
        let section=local.x/16u+side*(local.z/16u+side*(local.y/16u));
        let mask=sections[section].occupied;
        if (mask.x|mask.y)==0u {return vec4<u32>(0u,16u,0u,0u);}
        let brick=local.x%16u/4u+4u*(local.z%16u/4u+4u*(local.y%16u/4u));
        if (mask[brick/32u]&(1u<<(brick%32u)))==0u {return vec4<u32>(0u,4u,0u,0u);}
        return vec4<u32>(0u,1u,0u,0u);
    }
    let chunk=vec2<i32>(floor(vec2<f32>(cell.xz)/16.0));
    let wrapped=bitcast<vec2<u32>>(chunk)&vec2<u32>(__LOD_PAGE_SIDE__u-1u);
    let index=wrapped.x+__LOD_PAGE_SIDE__u*wrapped.y;let slot=8u+index*8u;
    if lod_pages[slot+6u]==2u && bitcast<i32>(lod_pages[slot])==chunk.x && bitcast<i32>(lod_pages[slot+1u])==chunk.y {
        let merged=lod_pages[8u+__LOD_PAGE_COUNT__u*8u+index*24u+u32(cell.y+64)/16u];
        if p.lod.y>0.5 && (merged>>8u)>16u && f32(merged>>8u)<=diameter {return vec4<u32>(merged&255u,merged>>8u,1u,0u);}
        return svo_cone_sample(lod_pages[slot+2u],cell,lod_pages[slot+3u],diameter);
    }
    // Unknown residency is explicitly distinct from known empty space.
    return vec4<u32>(0u,16u,2u,0u);
}
fn gi_crossing(start:vec3<f32>,direction:vec3<f32>,cell:vec3<i32>,size:u32,t:f32)->GridCrossing {
    let q=cell-p.origin.xyz;
    let near=q.x>=0 && q.z>=0 && q.x<i32(p.dims.x) && q.z<i32(p.dims.z);
    // Dense occupancy is anchored at the upload origin, which need not be
    // world-section aligned (headless/odd-sized near windows in particular).
    return grid_step(start,direction,cell,select(vec3<i32>(0),p.origin.xyz,near),size,t);
}
// Opacity belongs to the same animated texture metadata as aggregate RGB.
// Only alpha-tested voxel materials transmit; opaque solids remain opaque.
fn gi_coverage(material:u32,n:vec3<f32>)->f32 {
    if material==0u || material==5u {return 0.0;}
    let record=texture_record(material,n);
    if p.meshes.w==0u || (texture_records[record+3u]&1u)==0u {return 1.0;}
    return f32(texture_records[record+11u]>>24u)/255.0;
}
fn gi_outside_domain(cell:vec3<i32>)->bool {
    let point=vec2<f32>(cell.xz);
    var low=vec2<f32>(p.origin.xz);var high=low+vec2<f32>(p.dims.xz);
    if lod_pages[0]!=0u {
        low=min(low,vec2<f32>(f32(bitcast<i32>(lod_pages[1])),f32(bitcast<i32>(lod_pages[2]))));
        high=max(high,vec2<f32>(f32(bitcast<i32>(lod_pages[3])),f32(bitcast<i32>(lod_pages[4]))));
    }
    return any(point<low) || any(point>=high);
}
fn gi_sky_visible(start:vec3<f32>,direction:vec3<f32>)->f32 {
    if direction.y<=0.0001 {return 0.0;}
    var ceiling=p.origin.y+i32(p.dims.w);
    if lod_pages[0]!=0u {ceiling=max(ceiling,bitcast<i32>(lod_pages[6]));}
    var cell=ray_start_cell(start,direction,0.0);var t=0.0;var transmission=1.0;
    for(var step=0u;step<256u;step++) {
        // No rendered geometry exists above the dense/resident occupied bounds.
        // Requiring y=320 incorrectly blacked out exposed horizon directions.
        if cell.y>=ceiling || gi_outside_domain(cell) {return transmission;}
        if cell.y< -64 {return 0.0;}
        let node=gi_node(cell,1.0);
        if node.z==2u {return 0.0;}
        let crossing=gi_crossing(start,direction,cell,max(1u,node.y),t);
        let coverage=gi_coverage(node.x,vec3<f32>(0.0,1.0,0.0));
        if coverage>=0.999 {return 0.0;}
        if coverage>0.0 {
            transmission*=pow(1.0-coverage,max(0.0,crossing.face.w-t));
            if transmission<0.0001 {return 0.0;}
        }
        t=crossing.face.w;cell=crossing.cell;
    }
    return 0.0;
}
fn gi_source(point:vec3<f32>,n:vec3<f32>)->vec3<f32> {
    let outside=point+n*0.03125;
    // An entered face whose neighbor is solid is internal, not a radiating
    // surface. This is the queried face bit of an exposure mask, evaluated
    // from current data without adding bytes to every SVO node.
    let inside=gi_node(vec3<i32>(floor(point-n*0.03125)),1.0);
    if inside.z==2u || inside.x==0u || inside.x==5u {return vec3<f32>(0.0);}
    var exposed=false;
    if (u32(p.gi.x)&2u)!=0u && inside.w!=0u {
        let axis=select(select(2u,1u,abs(n.y)>0.5),0u,abs(n.x)>0.5);
        let cell=vec3<i32>(floor(point-n*0.03125));let low=cell&vec3<i32>(~i32(inside.y-1u));
        let plane=f32(low[axis])+select(0.0,f32(inside.y),n[axis]>0.0);
        if abs(point[axis]-plane)<0.002 {
            let face=axis*2u+select(0u,1u,n[axis]>0.0);
            if (inside.w&1u)!=0u || (inside.w&(1u<<(face+2u)))!=0u {return vec3<f32>(0.0);}
            exposed=(inside.w&2u)!=0u;
        }
    }
    if !exposed {
        let neighbor=gi_node(vec3<i32>(floor(outside)),1.0);
        if neighbor.z==2u || gi_coverage(neighbor.x,n)>=0.999 {return vec3<f32>(0.0);}
    }
    var ambient=vec3<f32>(0.0);
    // Two stable sky probes approximate SH visibility without an all-or-nothing
    // vertical-canopy gate. Leaf transmission uses the pack's average coverage.
    if n.y> -0.99 {
        let tangent=normalize(cross(n,select(vec3<f32>(0.0,1.0,0.0),vec3<f32>(1.0,0.0,0.0),abs(n.y)>0.9)));
        let up=n+vec3<f32>(0.0,1.0,0.0);
        let visibility=(gi_sky_visible(outside,normalize(up+tangent*0.7))+gi_sky_visible(outside,normalize(up-tangent*0.7)))*0.5;
        ambient=gi_source_ambient(n)*visibility;
    }
    var direct=0.0;
    let cosine=max(dot(n,p.sun.xyz),0.0);
    if cosine>0.0 {
        var visible=1.0;
        if p.effects.w>0.5 {visible=gi_sky_visible(outside,p.sun.xyz);}
        direct=cosine*visible;
    }
    var h=miss();h.material=inside.x;h.normal_depth=vec4<f32>(n,0.0);h.valid=9u;
    return hit_color(h,point)*(ambient+direct_light()*direct);
}
fn gi_cone(start:vec3<f32>,direction:vec3<f32>)->vec3<f32> {
    var t=0.0;var cell=ray_start_cell(start,direction,0.0);var radiance=vec3<f32>(0.0);var throughput=1.0;
    for(var step=0u;step<192u;step++) {
        if t>=p.gi.y {break;}
        // Cone diameter = 2*d*tan(half-angle), hence node solid angle
        // approximately matches the cone's solid angle rather than camera pixels.
        let node=gi_node(cell,max(1.0,2.0*t*p.gi.z));
        if node.z==2u {return radiance+gi_environment(direction)*gi_sky_visible(start,direction);}
        let material=node.x;let size=max(1u,node.y);
        if material!=0u && material!=5u {
            let low=vec3<f32>(cell&vec3<i32>(~i32(size-1u)));
            let safe=select(vec3<f32>(1.0),direction,abs(direction)>vec3<f32>(1e-20));
            let entry=select(vec3<f32>(-1e20),min((low-start)/safe,(low+f32(size)-start)/safe),abs(direction)>vec3<f32>(1e-20));
            var n=vec3<f32>(0.0,0.0,-sign(direction.z));
            if entry.x>=entry.y && entry.x>=entry.z {n=vec3<f32>(-sign(direction.x),0.0,0.0);}
            else if entry.y>=entry.z {n=vec3<f32>(0.0,-sign(direction.y),0.0);}
            let point=start+direction*max(t,max(entry.x,max(entry.y,entry.z)));
            let crossing=gi_crossing(start,direction,cell,size,t);
            let coverage=gi_coverage(material,n);
            let opacity=1.0-pow(1.0-coverage,max(0.001,crossing.face.w-t));
            radiance+=throughput*opacity*gi_source(point,n);
            throughput*=1.0-opacity;
            if throughput<0.0001 {return radiance;}
            t=crossing.face.w;cell=crossing.cell;continue;
        }
        // DDA skips the selected implicit SVO cell; no fixed step can jump
        // across a thin cell that should have been visited at this footprint.
        let crossing=gi_crossing(start,direction,cell,size,t);
        t=crossing.face.w;cell=crossing.cell;
    }
    // Restart from the original air-side origin: reconstructing a point at a
    // rounded DDA exit can land beyond a thin wall at range exhaustion.
    return radiance+gi_environment(direction)*gi_sky_visible(start,direction);
}
fn gi_sample_seed(point:vec3<f32>,n:vec3<f32>)->u32 {
    // Keep voxel-face rounding noise away from hash-cell boundaries.
    let cell=bitcast<vec3<u32>>(vec3<i32>(floor((point+n*0.125+vec3<f32>(0.073,0.137,0.211))*4.0)));
    let frame=select(p.history.z,42u,(u32(p.gi.x)&4u)!=0u);
    return shadow_hash(cell.x^shadow_hash(cell.y)^shadow_hash(cell.z)^shadow_hash(frame));
}
fn gi_evaluate(point:vec3<f32>,n:vec3<f32>,seed:u32)->vec3<f32> {
    let tangent=normalize(cross(select(vec3<f32>(0.0,1.0,0.0),vec3<f32>(1.0,0.0,0.0),abs(n.y)>0.9),n));
    let bitangent=cross(n,tangent);
    let rotation=f32(shadow_hash(seed)&65535u)/65536.0*6.283185307;var value=vec3<f32>(0.0);
    for(var cone=0u;cone<4u;cone++) {
        let radius=sqrt((f32(cone)+0.5)/4.0);let phi=rotation+f32(cone)*2.39996323;
        let direction=normalize(tangent*(radius*cos(phi))+bitangent*(radius*sin(phi))+n*sqrt(1.0-radius*radius));
        value+=gi_cone(point+n*0.03,direction)*0.25;
    }
    return clamp(value,vec3<f32>(0.0),vec3<f32>(32.0));
}
fn gi_ambient(pixel:vec2<u32>,h:Hit)->vec3<f32> {
    let size=gi_size();let point=point_at(pixel,h.normal_depth.w);
    let q=vec2<i32>(floor(vec2<f32>(pixel)*0.5));var sum=vec3<f32>(0.0);var weights=0.0;
    for(var y=-1;y<=1;y++) {for(var x=-1;x<=1;x++) {
        let c=clamp(q+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(size)-1);
        let base=gi_base(p.history.y,u32(c.x)+size.x*u32(c.y));
        let surface=gi_history[base];let value=gi_history[base+1u];let key=bitcast<u32>(surface.w);
        let stamp=bitcast<u32>(value.w);
        if (stamp>>8u)!=(p.history.z&0x00ffffffu) || (stamp&255u)==0u || (key>>24u)!=h.material || dot(unpack_history_normal(key),h.normal_depth.xyz)<0.98 {continue;}
        let plane=abs(dot(surface.xyz-point,h.normal_depth.xyz));
        let weight=exp(-plane*16.0-distance(surface.xyz,point)*0.25);
        sum+=value.xyz*weight;weights+=weight;
    }}
    // Trace unsupported thin surfaces instead of returning black or inventing sky.
    if weights<0.001 {return gi_evaluate(point,h.normal_depth.xyz,gi_sample_seed(point,h.normal_depth.xyz));}
    return sum/weights;
}

