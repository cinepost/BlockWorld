@group(0) @binding(4) var<uniform> p:Params;
fn source_surface(xz:vec2<f32>)->f32 {
    let x=i32(floor(xz.x));let z=i32(floor(xz.y));
    var y=p.origin.y+i32(p.dims.w)-1;
    if !in_world(vec3<i32>(x,y,z)) {return -10000.0;}
    loop {
        if y<p.origin.y+p.origin.w {break;}
        let cell=vec3<i32>(x,y,z);let q=vec3<u32>(cell-p.origin.xyz);let side=p.dims.x/16u;
        let section=q.x/16u+side*(q.z/16u+side*(q.y/16u));let mask=sections[section].occupied;
        if (mask.x|mask.y)==0u {y=p.origin.y+i32(q.y/16u*16u)-1;continue;}
        let brick=q.x%16u/4u+4u*(q.z%16u/4u+4u*(q.y%16u/4u));
        if (mask[brick/32u]&(1u<<(brick%32u)))==0u {y=p.origin.y+i32(q.y/4u*4u)-1;continue;}
        let material=block_at(cell);
        if material!=0u {
            if material==5u {return f32(y+1);}
            return -10000.0;
        }
        y-=1;
    }
    return -10000.0;
}
// Neighboring source rays give the refracted footprint differential. Cache a
// 9x9 source tile for each 8x8 workgroup, including the positive-edge halo.
var<workgroup> photon_tile:array<vec4<f32>,81>;
fn source_photon(world:vec2<f32>,cell:f32)->vec4<f32> {
    let level=source_surface(world);if level< -999.0 {return vec4<f32>(0.0,0.0,level,0.0);}
    let point=vec3<f32>(world.x,level,world.y);
    let gradient=wave_gradient(world,vec2<f32>(cell,0.0),vec2<f32>(0.0,cell),p.water.x,distance(point,p.eye.xyz));
    let n=normalize(vec3<f32>(-gradient.x,1.0,-gradient.y));
    if p.optics.x>1.0001 && dot(n,p.sun.xyz)<=0.0 {return vec4<f32>(0.0,0.0,level,0.0);}
    let ray=refract(-p.sun.xyz,n,1.0/max(1.0,p.optics.x));
    if ray.y>= -0.01 {return vec4<f32>(0.0,0.0,level,0.0);}
    // Uniform flux per horizontal source area; IOR=1 must not focus light.
    return vec4<f32>(ray.xz/(-ray.y),level,1.0);
}
fn deposit(map:u32,layer:u32,pixel:vec2<i32>,weight:f32) {
    if any(pixel<vec2<i32>(0)) || any(pixel>=vec2<i32>(i32(CAUSTIC_SIDE))) {return;}
    let index=caustic_base(map)+(layer+1u)*CAUSTIC_PIXELS+u32(pixel.x)+CAUSTIC_SIDE*u32(pixel.y);
    atomicAdd(&caustic_data[index],u32(round(weight*8192.0)));
}
fn bilinear_photon(map:u32,layer:u32,q:vec2<f32>) {
    let base=vec2<i32>(floor(q));let f=fract(q);
    for(var y=0;y<2;y++) {for(var x=0;x<2;x++) {
        deposit(map,layer,base+vec2<i32>(x,y),select(1.0-f.x,f.x,x==1)*select(1.0-f.y,f.y,y==1));
    }}
}
fn ellipse_weight(delta:vec2<f32>,inverse:vec3<f32>)->f32 {
    let r2=inverse.x*delta.x*delta.x+2.0*inverse.y*delta.x*delta.y+inverse.z*delta.y*delta.y;
    return exp2(-0.72134752*max(0.0,r2))*(1.0-smoothstep(4.0,6.25,r2));
}
fn adaptive_photon(map:u32,layer:u32,q:vec2<f32>,a:vec2<f32>,b:vec2<f32>) {
    // Covariance of a transformed unit-area source pixel, plus a 0.3-texel
    // reconstruction floor. Compression retains thin folds; expansion spreads
    // flux instead of leaving a lattice of disconnected point splats.
    var covariance=vec3<f32>((a.x*a.x+b.x*b.x)/12.0,(a.x*a.y+b.x*b.y)/12.0,(a.y*a.y+b.y*b.y)/12.0);
    covariance*=min(1.0,2.47/max(max(covariance.x,covariance.z),0.0001));
    covariance+=vec3<f32>(0.09,0.0,0.09);
    let determinant=max(0.000001,covariance.x*covariance.z-covariance.y*covariance.y);
    let inverse=vec3<f32>(covariance.z,-covariance.y,covariance.x)/determinant;
    let radius=vec2<i32>(ceil(2.5*sqrt(covariance.xz)));
    let center=vec2<i32>(floor(q+0.5));var total=0.0;
    for(var y=-radius.y;y<=radius.y;y++) {for(var x=-radius.x;x<=radius.x;x++) {
        total+=ellipse_weight(vec2<f32>(center+vec2<i32>(x,y))-q,inverse);
    }}
    if total<0.00001 {bilinear_photon(map,layer,q);return;}
    // Normalize over the whole footprint, including out-of-map taps, so map
    // edges never concentrate energy. The receiver guard fades those edges.
    for(var y=-radius.y;y<=radius.y;y++) {for(var x=-radius.x;x<=radius.x;x++) {
        let pixel=center+vec2<i32>(x,y);let weight=ellipse_weight(vec2<f32>(pixel)-q,inverse)/total;
        if weight>0.00001 {deposit(map,layer,pixel,weight);}
    }}
}
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>,@builtin(local_invocation_index) lane:u32,@builtin(local_invocation_id) local:vec3<u32>,@builtin(workgroup_id) group:vec3<u32>) {
    if !caustics_enabled() {return;}
    let map=id.z;let cell=caustic_cell(map);let origin=caustic_origin(map);
    let world=origin+(vec2<f32>(id.xy)+0.5)*cell;
    let adaptive=map==1u && caustic_adaptive();var photon=vec4<f32>(0.0);
    if adaptive {
        for(var k=lane;k<81u;k+=64u) {
            let pixel=group.xy*8u+vec2<u32>(k%9u,k/9u);
            photon_tile[k]=source_photon(origin+(vec2<f32>(pixel)+0.5)*cell,cell);
        }
        workgroupBarrier();
        photon=photon_tile[local.x+9u*local.y];
    } else {photon=source_photon(world,cell);}
    if photon.z< -999.0 {return;}
    let i=caustic_base(map)+id.x+CAUSTIC_SIDE*id.y;
    atomicStore(&caustic_data[i],u32(photon.z+65.0));
    if photon.w<0.5 {return;}
    var dx=vec2<f32>(0.0);var dy=vec2<f32>(0.0);
    if adaptive {
        let right=photon_tile[local.x+1u+9u*local.y];let down=photon_tile[local.x+9u*(local.y+1u)];
        if right.w>0.5 && abs(right.z-photon.z)<0.01 {dx=right.xy-photon.xy;}
        if down.w>0.5 && abs(down.z-photon.z)<0.01 {dy=down.xy-photon.xy;}
    }
    for(var layer=0u;layer<caustic_layers(map);layer++) {
        let depth=caustic_layer_depth(map,layer);
        let receiver=world+photon.xy*depth;
        let q=(receiver-origin)/cell-0.5;
        if adaptive {adaptive_photon(map,layer,q,vec2<f32>(1.0,0.0)+dx*(depth/cell),vec2<f32>(0.0,1.0)+dy*(depth/cell));}
        else {bilinear_photon(map,layer,q);}
    }
}
