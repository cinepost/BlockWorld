@group(0) @binding(1) var<storage,read> hits:array<Hit>;
@group(0) @binding(14) var<storage,read_write> scene_color:array<vec4<f32>>;
@group(0) @binding(11) var<storage,read> mesh_shadows:array<vec2<f32>>;
@group(0) @binding(4) var<uniform> p:Params;
@group(0) @binding(7) var<storage,read> ao:array<f32>;
fn unpack(v:u32)->vec3<f32> {return vec3<f32>(f32((v>>16u)&255u),f32((v>>8u)&255u),f32(v&255u))/255.0;}
fn mesh_sun(index:u32,water:bool)->f32 {
    if p.meshes.x==0u {return 1.0;}
    return select(mesh_shadows[index].x,mesh_shadows[index].y,water);
}
// Use the same disk directions and intersect the per-sample visibility of both
// occluder classes. Multiplying their averages biases overlapping penumbrae.
fn primary_sunlight(point:vec3<f32>,normal:vec3<f32>,i:u32,water:bool)->vec2<f32> {
    if p.effects.w<0.5 || dot(normal,p.sun.xyz)<=0.0 {return vec2<f32>(1.0,0.0);}
    if p.soft.y<0.5 {return vec2<f32>(sunlight(point,normal)*mesh_sun(i,water),0.0);}
    if p.sun.y>tan(p.soft.x) && point.y>=p.scene_max.w && p.meshes.x==0u {return vec2<f32>(1.0,0.0);}
    let blend=shadow_blend(point);
    let voxel_map=far_sunlight(point,normal,false);
    if blend>=1.0 {
        let visibility=voxel_map*mesh_sun(i,water);
        return vec2<f32>(visibility,select(0.0,1.0,visibility>0.001 && visibility<0.999));
    }
    let voxel_edge=voxel_map>0.001 && voxel_map<0.999;
    var mesh_mask=15u;
    if p.meshes.x!=0u {mesh_mask=u32(select(shadow_raw[i].x,shadow_raw[i].w,water));}
    let mesh_edge=(mesh_mask&16u)!=0u;
    let count=select(1u,u32(clamp(p.soft.z,2.0,4.0)),voxel_edge || mesh_edge);
    var sum=0.0;
    for(var sample=0u;sample<count;sample++) {
        let bit=select(0u,sample,mesh_edge);
        if (mesh_mask&(1u<<bit))==0u {continue;}
        let visibility=select(1.0,0.0,trace(point+normal*0.003,disk_direction(point,sample),p.up.w,false).valid!=0u);
        sum+=visibility;
    }
    var visibility=sum/f32(count);
    if blend>0.0 {var mesh_map=1.0;if p.meshes.x!=0u {mesh_map=far_sunlight(point,normal,true);}visibility=mix(visibility,voxel_map*mesh_map,blend);}
    // A binary map value is not proof of geometric umbra. Keep a stochastic
    // disk sample and temporal history even when the classifier misses an edge.
    return vec2<f32>(visibility,1.0);
}
fn sunlight(point:vec3<f32>,normal:vec3<f32>)->f32 {
    if p.effects.w<0.5 || dot(normal,p.sun.xyz)<=0.0 {return 1.0;}
    let blend=shadow_blend(point);
    if blend>=1.0 {return far_sunlight(point,normal,false);}
    var exact=0.0;
    if penumbra(point,normal,false) {
        let samples=u32(clamp(p.soft.z,2.0,4.0));
        for(var sample=0u;sample<samples;sample++) {
            let ray=disk_direction(point,sample);
            let hit=trace(point+normal*0.003,ray,p.up.w,false);
            exact+=select(1.0,0.0,hit.valid!=0u);
        }
        exact/=f32(samples);
    } else {exact=exact_sunlight(point,normal);}
    if blend<=0.0 {return exact;}
    return mix(exact,far_sunlight(point,normal,false),blend);
}
fn exact_sunlight(point:vec3<f32>,normal:vec3<f32>)->f32 {
    if p.effects.w<0.5 || dot(normal,p.sun.xyz)<=0.0 {return 1.0;}
    if p.sun.y>=0.0 && point.y>=p.scene_max.w {return 1.0;}
    let start=point+normal*0.003;
    if p.soft.y>0.5 {return select(1.0,0.0,trace(start,disk_direction(point,0u),p.up.w,false).valid!=0u);}
    if cascade_clear(start,false) {return 1.0;}
    // Local exact contact shadows; the cache can reject the entire remaining ray.
    let near_limit=min(8.0,p.up.w);
    if p.acceleration.y!=0u && cascade_clear(start+p.sun.xyz*near_limit,false) {
        return select(1.0,0.0,trace(start,p.sun.xyz,near_limit,false).valid!=0u);
    }
    let shadow=trace(start,p.sun.xyz,p.up.w,false);
    return select(1.0,0.0,shadow.valid!=0u);
}
fn ambient(id:vec2<u32>,h:Hit)->f32 {
    if p.flags.z==0u {return 1.0;}
    // Exact fast path: filtering any set of all-one samples returns one.
    // Avoid nine 64-byte visibility reads and nonlinear weights on flat lit areas.
    var unoccluded=true;
    for(var y=-1;y<=1;y++) {for(var x=-1;x<=1;x++) {
        let q=vec2<i32>(id)+vec2<i32>(x,y);
        if all(q>=vec2<i32>(0)) && all(q<vec2<i32>(p.screen.xy)) {
            if ao[u32(q.x)+p.screen.x*u32(q.y)]!=1.0 {unoccluded=false;}
        }
    }}
    if unoccluded {return 1.0;}
    let center=point_at(id,h.normal_depth.w);var sum=0.0;var weights=0.0;
    for(var y=-1;y<=1;y++) {for(var x=-1;x<=1;x++) {
        let q=vec2<i32>(id)+vec2<i32>(x,y);
        if any(q<vec2<i32>(0)) || any(q>=vec2<i32>(p.screen.xy)) {continue;}
        let i=u32(q.x)+p.screen.x*u32(q.y);let neighbor=hits[i];
        if neighbor.valid==0u {continue;}
        let normal_weight=pow(max(dot(h.normal_depth.xyz,neighbor.normal_depth.xyz),0.0),8.0);
        let plane_distance=abs(dot(point_at(vec2<u32>(q),neighbor.normal_depth.w)-center,h.normal_depth.xyz));
        let weight=normal_weight*exp(-plane_distance*16.0);
        sum+=ao[i]*weight;weights+=weight;
    }}
    return sum/max(weights,0.0001);
}
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=p.screen.x || id.y>=p.screen.y {return;}
    let i=id.x+p.screen.x*id.y;
    var h=miss();if pixel_flags(i)!=0u || p.screen.z!=0u {h=hits[i];}
    let ray=ray_at(vec2<f32>(id.xy));
    let sky=sky_color(ray);var color=sky;var accessibility=1.0;var sun=1.0;var edge=false;
    if h.valid!=0u {
        let albedo=hit_color(h,point_at(id.xy,h.normal_depth.w));
        let n=h.normal_depth.xyz;let point=point_at(id.xy,h.normal_depth.w);
        accessibility=ambient(id.xy,h);let visibility=primary_sunlight(point,n,i,false);sun=visibility.x;edge=visibility.y>0.5;
        var indirect=sky_diffuse(n)*accessibility;
        if p.gi.x>0.5 {indirect=gi_ambient(id.xy,h);}
        color=albedo*(indirect+direct_light()*water_direct(point,n)*sun);
        color=mix(color,sky,smoothstep(p.up.w*0.65,p.up.w,h.normal_depth.w));
    }
    if p.screen.z==1u {color=select(vec3<f32>(0.0),h.normal_depth.xyz*0.5+0.5,h.valid!=0u);}
    if p.screen.z==2u {color=vec3<f32>(h.normal_depth.w/p.up.w);}
    if p.screen.z==3u {color=vec3<f32>(0.0);if h.valid!=0u {color=hit_color(h,point_at(id.xy,h.normal_depth.w));}}
    if p.screen.z==4u {color=vec3<f32>(h.uv,0.0);}
    if p.screen.z==5u {color=vec3<f32>(accessibility);}
    if p.screen.z==6u {color=vec3<f32>(h.water.x/12.0,h.water.y*0.15,h.water.y);}
    if p.screen.z==7u {color=vec3<f32>(sun);}
    if p.screen.z==9u {color=select(vec3<f32>(0.2),vec3<f32>(0.1,0.8,0.9),(h.valid&4u)!=0u);if (h.valid&2u)!=0u {color=vec3<f32>(0.2,1.0,0.1);}}
    if p.screen.z==8u {color=mix(vec3<f32>(0.0,0.1,0.3),vec3<f32>(1.0,0.2,0.0),min(h.water.w/128.0,1.0));}
    var water_sun=1.0;
    if h.water.y>0.5 {let point=point_at(id.xy,h.water_normal_depth.w);water_sun=primary_sunlight(point,h.water_normal_depth.xyz,i,true).x;}
    shadow_raw[i]=vec4<f32>(sun,select(0.0,1.0,edge),sun,water_sun);
    scene_color[i]=vec4<f32>(color,1.0);
}
