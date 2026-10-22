fn caustic_height_map(world:vec2<f32>,map:u32)->f32 {
    let q=vec2<i32>(floor((world-caustic_origin(map))/caustic_cell(map)));
    if any(q<vec2<i32>(0)) || any(q>=vec2<i32>(i32(CAUSTIC_SIDE))) {return -10000.0;}
    let encoded=caustic_load(caustic_base(map)+u32(q.x)+CAUSTIC_SIDE*u32(q.y));
    if encoded==0u {return -10000.0;}
    return f32(encoded)-65.0;
}
fn caustic_slice(world:vec2<f32>,layer:u32,map:u32)->f32 {
    let uv=(world-caustic_origin(map))/caustic_cell(map)-0.5;let q=vec2<i32>(floor(uv));let f=fract(uv);
    var value=0.0;
    // Coarse map retains its tent filter. Near photons are already footprint-
    // filtered: bilinear reconstruction preserves their narrow filaments.
    for(var y=0;y<2;y++) {for(var x=0;x<2;x++) {
        let c=clamp(q+vec2<i32>(x,y),vec2<i32>(1),vec2<i32>(i32(CAUSTIC_SIDE)-2));
        var sample=0.0;
        let taps=select(5u,1u,map==1u);
        for(var tap=0u;tap<taps;tap++) {
            let offsets=array<vec2<i32>,5>(vec2<i32>(0),vec2<i32>(1,0),vec2<i32>(-1,0),vec2<i32>(0,1),vec2<i32>(0,-1));
            let t=c+offsets[tap];let weight=select(select(0.125,0.5,tap==0u),1.0,map==1u);
            sample+=f32(caustic_load(caustic_base(map)+(layer+1u)*CAUSTIC_PIXELS+u32(t.x)+CAUSTIC_SIDE*u32(t.y)))*weight/8192.0;
        }
        value+=sample*select(1.0-f.x,f.x,x==1)*select(1.0-f.y,f.y,y==1);
    }}
    return value;
}
fn caustic_density(world:vec2<f32>,depth:f32,map:u32)->f32 {
    var layer=0u;let count=caustic_layers(map);
    for(var next=1u;next<count;next++) {if depth>caustic_layer_depth(map,next) {layer=next;}}
    let high=min(layer+1u,count-1u);
    let blend=clamp((depth-caustic_layer_depth(map,layer))/max(0.001,caustic_layer_depth(map,high)-caustic_layer_depth(map,layer)),0.0,1.0);
    return mix(caustic_slice(world,layer,map),caustic_slice(world,high,map),blend);
}
fn water_direct(point:vec3<f32>,normal:vec3<f32>)->vec3<f32> {
    let ordinary=vec3<f32>(max(dot(normal,p.sun.xyz),0.0));
    if !caustics_enabled() {return ordinary;}
    let camera_distance=distance(point,p.eye.xyz);
    if camera_distance>=p.caustics.z {return ordinary;}
    let adjacent=point+normal*0.025;
    if block_at(vec3<i32>(floor(adjacent)))!=5u {return ordinary;}
    let surface=caustic_height_map(adjacent.xz,0u);let depth=surface-point.y;
    if depth<=0.0 || depth>=p.caustics.w {return ordinary;}
    let uv=(point.xz-p.caustic_map.xy)/p.caustic_map.z;
    let edge=min(min(uv.x,uv.y),min(f32(CAUSTIC_SIDE)-uv.x,f32(CAUSTIC_SIDE)-uv.y))*p.caustic_map.z;
    let fade=(1.0-smoothstep(p.caustics.z*0.5,p.caustics.z,camera_distance))*smoothstep(10.0,18.0,edge)*smoothstep(0.0,0.4,depth)*(1.0-smoothstep(15.0,p.caustics.w,depth));
    if fade<=0.0001 {return ordinary;}
    // Reject covered cave pools and separated stacked water volumes.
    let column=vec2<i32>(floor(adjacent.xz));
    for(var y=i32(floor(adjacent.y));y<i32(surface);y++) {
        if block_at(vec3<i32>(column.x,y,column.y))!=5u {return ordinary;}
    }
    let flat_ray=refract(-p.sun.xyz,vec3<f32>(0.0,1.0,0.0),1.0/max(1.0,p.optics.x));
    let entry=adjacent.xz-flat_ray.xz*(depth/max(0.01,-flat_ray.y));
    if abs(caustic_height_map(entry,0u)-surface)>0.1 {return ordinary;}
    var focused=caustic_density(point.xz,depth,0u);
    if caustic_near() {
        let near_origin=caustic_origin(1u);
        let low=min(point.xz,entry)-near_origin;
        let high=near_origin+vec2<f32>(32.0)-max(point.xz,entry);
        let near_edge=min(min(low.x,low.y),min(high.x,high.y));
        let depth_fade=select(1.0-smoothstep(7.0,15.0,depth),1.0-smoothstep(3.0,4.0,depth),caustic_shallow());
        let near_weight=smoothstep(2.0,4.0,near_edge)*depth_fade;
        if near_weight>0.0001 && abs(caustic_height_map(point.xz,1u)-surface)<0.1 && abs(caustic_height_map(entry,1u)-surface)<0.1 {
            focused=mix(focused,caustic_density(point.xz,depth,1u),near_weight);
        }
    }
    let intensity=clamp(1.0+(focused-1.0)*p.caustics.y,0.0,6.0);
    let f0=pow((p.optics.x-1.0)/(p.optics.x+1.0),2.0);
    let transmission=1.0-(f0+(1.0-f0)*pow(1.0-clamp(p.sun.y,0.0,1.0),5.0));
    let attenuation=exp(-vec3<f32>(0.45,0.16,0.08)*p.effects.z*depth/max(0.01,-flat_ray.y));
    // Density is measured per horizontal area. Preserve incident horizontal
    // irradiance when rotating the direct-light direction through Snell's law.
    let beam_scale=p.sun.y/max(0.01,-flat_ray.y);
    let light=attenuation*(intensity*transmission*beam_scale*max(dot(normal,-flat_ray),0.0));
    return mix(ordinary,light,fade);
}
