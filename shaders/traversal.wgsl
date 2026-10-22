// Included in visibility and shade; shadows always use full occupancy, never the frustum mask.
@group(0) @binding(0) var<storage,read> voxels:array<u32>;
@group(0) @binding(5) var<storage,read> sections:array<Section>;
@group(0) @binding(6) var<storage,read> visible:array<u32>;
fn in_world(c:vec3<i32>)->bool {
    let q=c-p.origin.xyz;return all(q>=vec3<i32>(0)) && all(q<vec3<i32>(p.dims.xyz)) && q.y>=p.origin.w && q.y<i32(p.dims.w);
}
fn block_at(c:vec3<i32>)->u32 {
    if !in_world(c) {return 0u;}
    let q=vec3<u32>(c-p.origin.xyz);
    let index=q.x+p.dims.x*(q.z+p.dims.z*q.y);
    return (voxels[index>>2u]>>((index&3u)*8u))&255u;
}
// Keep integer cell ownership across crossings. A forward epsilon can jump
// over a short but visible cube-edge interval; at large coordinates it can
// also round to zero. Advancing an integer axis handles both without a nudge.
struct GridCrossing {cell:vec3<i32>,face:vec4<f32>};
fn ray_start_cell(origin:vec3<f32>,ray:vec3<f32>,t:f32)->vec3<i32> {
    let point=fma(ray,vec3<f32>(t),origin);var cell=vec3<i32>(floor(point));
    for(var axis=0u;axis<3u;axis++) {
        if ray[axis]<0.0 && point[axis]==f32(cell[axis]) {cell[axis]-=1;}
    }
    return cell;
}
fn plane_crossing(origin:vec3<f32>,ray:vec3<f32>,cell:vec3<i32>,anchor:vec3<i32>,size:u32,axis:u32,plane:i32,t:f32)->GridCrossing {
    let low=anchor+((cell-anchor)&vec3<i32>(~i32(size-1u)));
    // At tied/rounded crossings retain the old side on the other axes. Their
    // own next step resolves the tie, rather than skipping a touched cell.
    var next=clamp(vec3<i32>(floor(fma(ray,vec3<f32>(t),origin))),low,low+vec3<i32>(i32(size)-1));
    next[axis]=plane-select(0,1,ray[axis]<0.0);
    var normal=vec3<f32>(0.0);normal[axis]=-sign(ray[axis]);
    return GridCrossing(next,vec4<f32>(normal,t));
}
fn grid_step(origin:vec3<f32>,ray:vec3<f32>,cell:vec3<i32>,anchor:vec3<i32>,size:u32,t:f32)->GridCrossing {
    let low=anchor+((cell-anchor)&vec3<i32>(~i32(size-1u)));
    let edge=low+select(vec3<i32>(0),vec3<i32>(i32(size)),ray>vec3<f32>(0.0));
    let moving=abs(ray)>vec3<f32>(1e-20);let safe=select(vec3<f32>(1.0),ray,moving);
    let distance=select(vec3<f32>(1e30),(vec3<f32>(edge)-origin)/safe,moving);
    var axis=2u;
    if distance.x<=distance.y && distance.x<=distance.z {axis=0u;}
    else if distance.y<=distance.z {axis=1u;}
    return plane_crossing(origin,ray,cell,anchor,size,axis,edge[axis],max(t,distance[axis]));
}
// Slab entry lets primary rays enter the loaded volume even from above MAX_Y.
fn volume_entry(origin:vec3<f32>,ray:vec3<f32>,limit:f32)->vec4<f32> {
    var low=vec3<f32>(p.origin.xyz);var high=low+vec3<f32>(p.dims.xyz);
    low.y+=f32(p.origin.w);high.y=f32(p.origin.y)+f32(p.dims.w);
    var near=0.0;var far=limit;var normal=-ray;
    for(var a=0u;a<3u;a++) {
        if abs(ray[a])<1e-20 {
            if origin[a]<low[a] || origin[a]>=high[a] {return vec4<f32>(0.0,0.0,0.0,limit+1.0);}
        } else {
            let t1=(low[a]-origin[a])/ray[a];let t2=(high[a]-origin[a])/ray[a];
            let entry=min(t1,t2);far=min(far,max(t1,t2));
            if entry>near {near=entry;normal=vec3<f32>(0.0);normal[a]=-sign(ray[a]);}
        }
    }
    if near>=far {return vec4<f32>(0.0,0.0,0.0,limit+1.0);}
    return vec4<f32>(normal,near);
}
// Resolve hit depth analytically; cell stepping and shading use the same planes.
fn block_entry(origin:vec3<f32>,ray:vec3<f32>,cell:vec3<i32>,material:u32)->vec4<f32> {
    let moving=abs(ray)>vec3<f32>(1e-20);
    let safe=select(vec3<f32>(1.0),ray,moving);
    let low=vec3<f32>(cell);
    let entries=select(vec3<f32>(-1e20),min((low-origin)/safe,(low+1.0-origin)/safe),moving);
    let near=max(entries.x,max(entries.y,entries.z));
    var normal=vec3<f32>(0.0,0.0,-sign(ray.z));
    if entries.x>=entries.y && entries.x>=entries.z {normal=vec3<f32>(-sign(ray.x),0.0,0.0);}
    else if entries.y>=entries.z {normal=vec3<f32>(0.0,-sign(ray.y),0.0);}
    var distance=near;
    if near>=0.0 {
        // Rounded near ties can land on a shared edge. The later
        // plane may be internal to adjacent solid voxels (e.g. a continuous floor).
        let neighbor=block_at(cell+vec3<i32>(normal));
        if neighbor!=0u && (neighbor!=5u || material==5u) {
            var exposed=-1e20;var exposed_normal=normal;
            for(var a=0u;a<3u;a++) {
                if !moving[a] || near-entries[a]>0.0003 {continue;}
                var face=vec3<i32>(0);face[a]=-i32(sign(ray[a]));
                let adjacent=block_at(cell+face);
                if (adjacent==0u || (adjacent==5u && material!=5u)) && entries[a]>exposed {
                    exposed=entries[a];exposed_normal=vec3<f32>(face);
                }
            }
            if exposed> -1e19 {normal=exposed_normal;distance=exposed;}
        }
    } else {normal=-ray;}
    return vec4<f32>(normal,max(distance,0.0));
}
fn trace_near(origin:vec3<f32>,ray:vec3<f32>,limit:f32,primary:bool,water_path:bool)->Hit {
    var h=miss();let entry=volume_entry(origin,ray,limit);var t=entry.w;var normal=entry.xyz;
    var cell=ray_start_cell(origin,ray,t);
    let low=p.origin.xyz+vec3<i32>(0,p.origin.w,0);
    let high=p.origin.xyz+vec3<i32>(i32(p.dims.x),i32(p.dims.w),i32(p.dims.z))-vec3<i32>(1);
    cell=clamp(cell,low,high);
    var water_exit=false;
    for(var iteration=0u;iteration<2048u;iteration++) {
        if t>=limit {break;}
        if !in_world(cell) {break;}
        h.water.w+=1.0;
        let q=vec3<u32>(cell-p.origin.xyz);
        let side=p.dims.x/16u;
        let section=q.x/16u+side*(q.z/16u+side*(q.y/16u));
        var masks=sections[section].opaque;
        if water_path {masks=sections[section].occupied;}
        var size=1.0;
        if p.flags.x!=0u {
            let brick=(q.x%16u)/4u+4u*((q.z%16u)/4u+4u*((q.y%16u)/4u));
            if (masks.x|masks.y)==0u {size=16.0;}
            else if (masks[brick/32u] & (1u<<(brick%32u)))==0u {size=4.0;}
        }
        if primary && p.flags.y!=0u && visible[section]==0u {size=16.0;}
        var material=0u;
        if size==1.0 {material=block_at(cell);}
        // If starting underwater, the first exit is the visible water/air interface.
        if h.water.z>0.5 && !water_exit && material==0u {
            h.water_normal_depth=vec4<f32>(normal,t);water_exit=true;
        }
        if material!=0u && (material!=5u || (primary && p.flags.w==0u)) {
            let resolved=block_entry(origin,ray,cell,material);
            let point=origin+ray*resolved.w;var uv=point.xz;
            if abs(resolved.x)>0.5 {uv=point.zy;}
            if abs(resolved.z)>0.5 {uv=point.xy;}
            if texture_covered(material,fract(uv),resolved.xyz,point) {
                h.normal_depth=resolved;h.uv=fract(uv);h.material=material;
                h.valid=1u|((texture_records[texture_record(material,resolved.xyz)+3u]&1u)<<1u);break;
            }
        }
        let crossing=grid_step(origin,ray,cell,p.origin.xyz,u32(size),t);
        let next=crossing.face;
        if water_path && material==5u {
            if h.water.y==0.0 {
                h.water.y=1.0;h.water_normal_depth=vec4<f32>(normal,t);
                if t==0.0 {h.water.z=1.0;}
            }
            h.water.x+=max(0.0,min(next.w,limit)-t);
        }
        t=next.w;normal=next.xyz;cell=crossing.cell;
    }
    return h;
}

fn trace(origin:vec3<f32>,ray:vec3<f32>,limit:f32,primary:bool)->Hit {
    return trace_internal(origin,ray,limit,primary,primary);
}
// Secondary optical rays retain water lengths but never use the camera frustum mask.
fn optical_trace(origin:vec3<f32>,ray:vec3<f32>,limit:f32)->Hit {
    return trace_internal(origin,ray,limit,false,true);
}

// Toroidal page table: signed chunk tags reject stale/wrapped entries. Payloads
// are packed RGBA8-sized words in the same device-local sparse page pool.
@group(0) @binding(26) var<storage,read> lod_pages:array<u32>;
// Random-access stackless descent: contiguous present children use popcount rank.
// The ray's integer DDA cell selects octants; empty branches skip their full AABB.
fn svo_projected_pixels(cell:vec3<i32>,size:u32)->f32 {
        let low=vec3<f32>(cell&vec3<i32>(~i32(size-1u)));
        let delta=low+f32(size)*0.5-p.eye.xyz;
        let half=f32(size)*0.5;
        let center=vec3<f32>(dot(delta,p.right.xyz),dot(delta,p.up.xyz),dot(delta,p.forward.xyz));
        let extent=vec3<f32>(dot(abs(p.right.xyz),vec3<f32>(half)),dot(abs(p.up.xyz),vec3<f32>(half)),dot(abs(p.forward.xyz),vec3<f32>(half)));
        let nearest=max(0.01,center.z-extent.z);let farthest=max(nearest,center.z+extent.z);
        let a=center.xy-extent.xy;let b=center.xy+extent.xy;
        let span=max(b/nearest,b/farthest)-min(a/nearest,a/farthest);
        return max(span.x,span.y)*f32(p.screen.y)/(2.0*p.forward.w);
}
fn svo_cell(base:u32,cell:vec3<i32>,minimum:u32)->vec3<u32> {return svo_cone_cell(base,cell,minimum,0.0);}
fn svo_cone_cell(base:u32,cell:vec3<i32>,minimum:u32,diameter:f32)->vec3<u32> {return svo_cone_sample(base,cell,minimum,diameter).xyz;}
fn svo_cone_sample(base:u32,cell:vec3<i32>,minimum:u32,diameter:f32)->vec4<u32> {
    let roots=lod_pages[base];let section=u32(cell.y+64)/16u;
    if (roots&(1u<<section))==0u {return vec4<u32>(0u,16u,0u,0u);}
    var node=countOneBits(roots&((1u<<section)-1u));var size=16u;
    let local=vec3<u32>(bitcast<vec3<u32>>(cell))&vec3<u32>(15u);
    for(var depth=0u;depth<5u;depth++) {
        let address=base+1u+node*2u;let bits=lod_pages[address+1u];let mask=bits&255u;
        let material=(bits>>8u)&255u;
        var pixels=1e30;
        if diameter==0.0 {pixels=svo_projected_pixels(cell,size);}
        if mask==0u {
            let coarse=minimum>1u || pixels/f32(size)<=max(0.0001,p.lod.x);
            return vec4<u32>(material,size,select(0u,1u,coarse),(bits>>16u)&255u);
        }
        if (diameter>0.0 && f32(size)<=diameter) || (diameter==0.0 && pixels<=max(0.0001,p.lod.x)) {return vec4<u32>(material,size,1u,(bits>>16u)&255u);}
        size/=2u;
        let octant=select(0u,1u,(local.x&size)!=0u)|select(0u,2u,(local.y&size)!=0u)|select(0u,4u,(local.z&size)!=0u);
        if (mask&(1u<<octant))==0u {return vec4<u32>(0u,size,0u,0u);}
        node=lod_pages[address]+countOneBits(mask&((1u<<octant)-1u));
    }
    return vec4<u32>(0u,1u,0u,0u);
}
fn trace_lod(origin:vec3<f32>,ray:vec3<f32>,limit:f32,water_path:bool)->Hit {
    var hit=miss();if lod_pages[0]==0u {return hit;}var t=0.0;var normal=-ray;
    let low=vec3<f32>(f32(i32(lod_pages[1])),f32(i32(lod_pages[5])),f32(i32(lod_pages[2])));
    let high=vec3<f32>(f32(i32(lod_pages[3])),f32(i32(lod_pages[6])),f32(i32(lod_pages[4])));
    var end=limit;
    for(var axis=0u;axis<3u;axis++) {
        if abs(ray[axis])<1e-20 {
            if origin[axis]<low[axis] || origin[axis]>=high[axis] {return hit;}
        } else {
            let a=(low[axis]-origin[axis])/ray[axis];let b=(high[axis]-origin[axis])/ray[axis];
            let entry=min(a,b);end=min(end,max(a,b));
            if entry>t {t=entry;normal=vec3<f32>(0.0);normal[axis]=-sign(ray[axis]);}
        }
    }
    if t>=end {return hit;}
    let local=origin.xz-vec2<f32>(p.origin.xz);
    if all(local>=vec2<f32>(0.0)) && all(local<vec2<f32>(p.dims.xz)) {
        let safe=select(vec2<f32>(1e-20),ray.xz,abs(ray.xz)>vec2<f32>(1e-20));
        let edges=select(vec2<f32>(0.0),vec2<f32>(p.dims.xz),ray.xz>vec2<f32>(0.0));
        let exits=select(vec2<f32>(1e20),(edges-local)/safe,abs(ray.xz)>vec2<f32>(1e-20));
        let exit=min(exits.x,exits.y);if exit>=end {return hit;}t=max(t,exit);
    }
    var cell=clamp(ray_start_cell(origin,ray,t),vec3<i32>(low),vec3<i32>(high)-vec3<i32>(1));
    var exited=false;
    for(var iteration=0u;iteration<4096u;iteration++) {
        if t>=end {break;}
        if cell.y< -64 || cell.y>=320 {break;}
        // Exact voxels always win in their entire X/Z footprint, including air.
        hit.water.w+=1.0;
        let q=cell-p.origin.xyz;
        let near=all(q.xz>=vec2<i32>(0)) && all(q.xz<vec2<i32>(p.dims.xz));
        var size=16.0;var material=0u;var scale=1u;var aggregate=0u;var slab_t=1e20;var slab_plane=0;
        if !near {
            let chunk=vec2<i32>(floor(vec2<f32>(cell.xz)/16.0));
            let wrapped=vec2<u32>(bitcast<u32>(chunk.x),bitcast<u32>(chunk.y))&vec2<u32>(__LOD_PAGE_SIDE__u-1u);
            let slot=8u+(wrapped.x+__LOD_PAGE_SIDE__u*wrapped.y)*8u;
            if lod_pages[slot+6u]!=0u && i32(lod_pages[slot])==chunk.x && i32(lod_pages[slot+1u])==chunk.y {
                scale=lod_pages[slot+3u];size=f32(scale);
                let local=vec3<u32>(vec3<i32>(cell.x-chunk.x*16,cell.y+64,cell.z-chunk.y*16))/scale;
                let low=lod_pages[slot+4u];let high=lod_pages[slot+5u];
                if local.y>=low && local.y<high {
                    if lod_pages[slot+6u]==2u {
                        let node=svo_cell(lod_pages[slot+2u],cell,scale);material=node.x;scale=node.y;size=f32(scale);aggregate=node.z;
                    } else {
                        let side=16u/scale;let index=local.x+side*(local.z+side*(local.y-low));
                        material=(lod_pages[lod_pages[slot+2u]+(index>>2u)]>>((index&3u)*8u))&255u;
                    }
                } else {
                    size=16.0;
                    if local.y<low && ray.y>1e-20 {slab_plane= -64+i32(low*scale);slab_t=(f32(slab_plane)-origin.y)/ray.y;}
                    if local.y>=high && ray.y< -1e-20 {slab_plane= -64+i32(high*scale);slab_t=(f32(slab_plane)-origin.y)/ray.y;}
                }
                // Cross-chunk homogeneous parents; never step across the exact window.
                let merge_index=(slot-8u)/8u;
                let merged=lod_pages[8u+__LOD_PAGE_COUNT__u*8u+merge_index*24u+u32(cell.y+64)/16u];
                let merged_size=merged>>8u;
                if merged_size>16u && p.lod.y>0.5 {
                    let ml=cell&vec3<i32>(~i32(merged_size-1u));
                    let mh=ml+vec3<i32>(i32(merged_size));
                    let outside=mh.x<=p.origin.x || ml.x>=p.origin.x+i32(p.dims.x) || mh.z<=p.origin.z || ml.z>=p.origin.z+i32(p.dims.z);
                    if outside {material=merged&255u;scale=merged_size;size=f32(scale);aggregate=0u;slab_t=1e20;}
                }
            }
        }
        if hit.water.z>0.5 && !exited && material==0u {hit.water_normal_depth=vec4<f32>(normal,t);exited=true;}
        if material!=0u && material!=5u {
            let low=floor(vec3<f32>(cell)/f32(scale))*f32(scale);
            let moving=abs(ray)>vec3<f32>(1e-20);let safe=select(vec3<f32>(1.0),ray,moving);
            let entries=select(vec3<f32>(-1e20),min((low-origin)/safe,(low+f32(scale)-origin)/safe),moving);
            var n=vec3<f32>(0.0,0.0,-sign(ray.z));
            if entries.x>=entries.y && entries.x>=entries.z {n=vec3<f32>(-sign(ray.x),0.0,0.0);}
            else if entries.y>=entries.z {n=vec3<f32>(0.0,-sign(ray.y),0.0);}
            let depth=max(0.0,max(entries.x,max(entries.y,entries.z)));
            let point=origin+ray*depth;var uv=point.xz;
            if abs(n.x)>0.5 {uv=point.zy;}if abs(n.z)>0.5 {uv=point.xy;}
            // Filtered coarse leaves represent coverage, rather than applying a
            // single full-size leaf cutout to an aggregate canopy cell.
            hit.normal_depth=vec4<f32>(n,depth);hit.material=material;hit.uv=fract(uv);hit.valid=1u|(aggregate<<3u);break;
        }
        let anchor=select(vec3<i32>(0),p.origin.xyz,near);
        var crossing=grid_step(origin,ray,cell,anchor,u32(size),t);
        if slab_t>=t && slab_t<crossing.face.w {crossing=plane_crossing(origin,ray,cell,anchor,u32(size),1u,slab_plane,slab_t);}
        let next=crossing.face;
        if water_path && material==5u {
            if hit.water.y==0.0 {hit.water.y=1.0;hit.water_normal_depth=vec4<f32>(normal,t);if t==0.0{hit.water.z=1.0;}}
            hit.water.x+=max(0.0,min(next.w,limit)-t);
        }
        normal=next.xyz;t=next.w;cell=crossing.cell;
    }
    return hit;
}
fn trace_internal(origin:vec3<f32>,ray:vec3<f32>,limit:f32,primary:bool,water_path:bool)->Hit {
    var near=trace_near(origin,ray,limit,primary,water_path);
    var far_limit=limit;if near.valid!=0u {far_limit=min(limit,near.normal_depth.w);}
    let far=trace_lod(origin,ray,far_limit,water_path);
    if far.valid!=0u {
        var result=far;
        if near.water.y>0.0 {result.water.x+=near.water.x;result.water.y=near.water.y;result.water.z=near.water.z;result.water_normal_depth=near.water_normal_depth;}
        return result;
    }
    near.water.w+=far.water.w;
    if far.water.y>0.0 && near.water.y==0.0 {near.water=far.water;near.water_normal_depth=far.water_normal_depth;}
    return near;
}
