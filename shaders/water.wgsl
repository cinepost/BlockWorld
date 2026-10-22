// Composition reads a completed opaque HDR buffer, never its own output.
@group(0) @binding(1) var<storage,read> hits:array<Hit>;
@group(0) @binding(2) var<storage,read_write> pixels:array<u32>;
@group(0) @binding(4) var<uniform> p:Params;
@group(0) @binding(11) var<storage,read> mesh_shadows:array<vec2<f32>>;
@group(0) @binding(14) var<storage,read> scene_color:array<vec4<f32>>;
fn mesh_sun(index:u32,water:bool)->f32 {
    if p.meshes.x==0u {return 1.0;}
    return select(mesh_shadows[index].x,mesh_shadows[index].y,water);
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
    if cascade_clear(start,false) {return 1.0;}
    // Local exact contact shadows; the cache can reject the entire remaining ray.
    let near_limit=min(8.0,p.up.w);
    if p.acceleration.y!=0u && cascade_clear(start+p.sun.xyz*near_limit,false) {
        return select(1.0,0.0,trace(start,p.sun.xyz,near_limit,false).valid!=0u);
    }
    let shadow=trace(start,p.sun.xyz,p.up.w,false);
    return select(1.0,0.0,shadow.valid!=0u);
}

// xyz: screen UV and signed view-depth difference, w: valid screen/depth sample.
fn projected_delta(point:vec3<f32>)->vec4<f32> {
    let v=point-p.eye.xyz;let z=dot(v,p.forward.xyz);
    if z<=0.01 {return vec4<f32>(0.0);}
    let uv=vec2<f32>(dot(v,p.right.xyz)/(z*p.eye.w*p.forward.w),-dot(v,p.up.xyz)/(z*p.forward.w))*0.5+0.5;
    if any(uv<vec2<f32>(0.0)) || any(uv>=vec2<f32>(1.0)) {return vec4<f32>(uv,0.0,0.0);}
    let q=vec2<u32>(uv*vec2<f32>(p.screen.xy));let h=hits[q.x+p.screen.x*q.y];
    if h.valid==0u {return vec4<f32>(uv,-1e6,1.0);}
    let observed=dot(point_at(q,h.normal_depth.w)-p.eye.xyz,p.forward.xyz);
    return vec4<f32>(uv,z-observed,1.0);
}
fn screen_reflection(point:vec3<f32>,direction:vec3<f32>,limit:f32)->vec4<f32> {
    // Nearby screen-space polygons supplement the full-range voxel reflection.
    if p.meshes.x==0u || p.optics.z<0.5 {return vec4<f32>(0.0);}
    let origin=point+direction*0.025;
    var previous=0.02;var was_front=false;
    let range=min(32.0,limit);
    for(var step=0u;step<32u;step++) {
        let f=f32(step+1u)/32.0;let travel=0.02+range*f*f;
        let sample=projected_delta(origin+direction*travel);
        if sample.w==0.0 {break;}
        if sample.z>=0.0 && was_front {
            var low=previous;var high=travel;
            for(var refine=0u;refine<6u;refine++) {
                let middle=(low+high)*0.5;let projected=projected_delta(origin+direction*middle);
                if projected.w==0.0 || projected.z<0.0 {low=middle;}else{high=middle;}
            }
            let result=projected_delta(origin+direction*high);
            let q=min(vec2<u32>(result.xy*vec2<f32>(p.screen.xy)),p.screen.xy-vec2<u32>(1u));
            let h=hits[q.x+p.screen.x*q.y];
            let surface=point_at(q,h.normal_depth.w);
            if result.w>0.5 && result.z>=0.0 && result.z<0.12+high*0.005 && (h.valid&4u)!=0u && distance(surface,origin+direction*high)<0.15+high*0.008 && distance(surface,point)>0.15 && dot(h.normal_depth.xyz,-direction)>0.01 {
                let edge=min(min(result.x,1.0-result.x),min(result.y,1.0-result.y));
                return vec4<f32>(scene_color[q.x+p.screen.x*q.y].xyz,smoothstep(0.0,0.04,edge));
            }
        }
        was_front=sample.z<0.0;previous=travel;
    }
    return vec4<f32>(0.0);
}
fn water_light()->vec3<f32> {
    // Optical water uses absorption/scattering and wave normals, never block albedo.
    let tint=vec3<f32>(0.025,0.19,0.26);
    return tint*(0.5+0.5*max(p.sun.y,0.0))*p.sun.w;
}
fn attenuate(color:vec3<f32>,length:f32,density:f32)->vec3<f32> {
    let transmission=exp(-vec3<f32>(0.45,0.16,0.08)*length*density);
    return color*transmission+water_light()*(vec3<f32>(1.0)-transmission);
}
fn secondary_color(origin:vec3<f32>,direction:vec3<f32>,h:Hit)->vec3<f32> {
    var color=sky_color(direction);
    if h.valid!=0u {
        let n=h.normal_depth.xyz;
        let albedo=hit_color(h,origin+direction*h.normal_depth.w);
        let sun=sunlight(origin+direction*h.normal_depth.w,n);
        color=albedo*(sky_diffuse(n)+direct_light()*water_direct(origin+direction*h.normal_depth.w,n)*sun);
        color=mix(color,sky_color(direction),smoothstep(p.up.w*0.65,p.up.w,h.normal_depth.w));
    }
    return color;
}
// Unpolarized dielectric Fresnel, including the water->air critical angle.
fn dielectric(cosine:f32,n1:f32,n2:f32)->f32 {
    let sin2=(n1/n2)*(n1/n2)*(1.0-cosine*cosine);
    if sin2>=1.0 {return 1.0;}
    let transmitted=sqrt(max(0.0,1.0-sin2));
    let rs=(n1*cosine-n2*transmitted)/max(0.00001,n1*cosine+n2*transmitted);
    let rp=(n1*transmitted-n2*cosine)/max(0.00001,n1*transmitted+n2*cosine);
    return (rs*rs+rp*rp)*0.5;
}
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=p.screen.x || id.y>=p.screen.y {return;}
    let i=id.x+p.screen.x*id.y;
    if (pixel_flags(i)&2u)==0u {
        var color=scene_color[i].xyz;
        if p.screen.z>=10u {color=vec3<f32>(0.0);}
        pixels[i]=pack_display(id.xy,color);return;
    }
    let h=hits[i];let ray=ray_at(vec2<f32>(id.xy));
    var color=scene_color[i].xyz;var confidence=0.0;var normal=vec3<f32>(0.0);var fresnel=0.0;var reflected_material=0u;
    if h.water.y>0.5 && (p.screen.z==0u || p.screen.z>=10u) {
        let under=h.water.z>0.5;
        let density=select(p.effects.z,p.optics.y,under);
        // No interface (bed or fish before the exit): fog only, never reflect the bed.
        color=attenuate(color,h.water.x,density);
        let boundary_depth=h.water_normal_depth.w;
        if boundary_depth>0.001 && boundary_depth<p.up.w {
            let point=p.eye.xyz+ray*boundary_depth;
            normal=water_normal(id.xy,point,h.water_normal_depth.xyz,boundary_depth);
            if dot(ray,normal)>0.0 {normal=-normal;}
            let n1=select(1.0,p.optics.x,under);let n2=select(p.optics.x,1.0,under);
            fresnel=dielectric(clamp(dot(-ray,normal),0.0,1.0),n1,n2);
            let reflected_direction=reflect(ray,normal);
            if p.water.z>0.5 {
                let reflected_origin=point+normal*0.003;
                let reflected_hit=optical_trace(reflected_origin,reflected_direction,p.up.w);
                var reflected=secondary_color(reflected_origin,reflected_direction,reflected_hit);
                reflected_material=reflected_hit.material;
                confidence=select(0.0,1.0,reflected_hit.valid!=0u);
                let ssr=screen_reflection(point,reflected_direction,reflected_hit.normal_depth.w);
                // Reuse screen color only for polygons; voxel rays supply hills/trees at any height.
                confidence=max(confidence,ssr.w);
                reflected=mix(reflected,ssr.xyz,ssr.w);
                reflected=attenuate(reflected,reflected_hit.water.x,select(p.effects.z,p.optics.y,under));
                var transmitted=water_light();
                if fresnel<0.99999 {
                    let direction=refract(ray,normal,n1/n2);
                    let origin=point-normal*0.003;
                    let hit=optical_trace(origin,direction,p.up.w);
                    transmitted=secondary_color(origin,direction,hit);
                    let screen=screen_reflection(point,direction,hit.normal_depth.w);
                    transmitted=mix(transmitted,screen.xyz,screen.w);
                    transmitted=attenuate(transmitted,hit.water.x,density);
                }
                color=mix(transmitted,reflected,fresnel);
                // The camera-side segment is in water only when looking out from below.
                if under {color=attenuate(color,boundary_depth,p.optics.y);}
            } else {
                // Disable optical rays for profiling; retain the physically correct Fresnel sky.
                color=mix(color,sky_color(reflected_direction),fresnel);
            }
            if !under && fresnel<1.0 {
                let highlight=pow(max(dot(reflect(-p.sun.xyz,normal),-ray),0.0),160.0);
                color+=vec3<f32>(1.0,0.91,0.72)*highlight*p.sun.w*shadow_raw[i].w*0.75;
            }
        }
    }
    if p.screen.z==10u {color=vec3<f32>(confidence);}
    if p.screen.z==11u {color=select(vec3<f32>(0.0),normal*0.5+0.5,h.water.y>0.5);}
    if p.screen.z==12u {color=vec3<f32>(fresnel);}
    if p.screen.z==14u {color=vec3<f32>(f32(reflected_material)/255.0);}
    if p.screen.z==13u {color=vec3<f32>(1.0-exp(-h.water.x/8.0));}
    pixels[i]=pack_display(id.xy,color);
}
