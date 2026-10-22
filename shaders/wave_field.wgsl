@group(0) @binding(15) var wave_texture:texture_2d<f32>;
@group(0) @binding(16) var wave_sampler:sampler;
fn plane_gradient(pixel:vec2<f32>,point:vec3<f32>)->vec2<f32> {
    let ray=ray_at(pixel);
    if abs(ray.y)<0.0001 {return vec2<f32>(128.0);}
    let distance=(point.y-p.eye.y)/ray.y;
    if distance<=0.0 {return vec2<f32>(128.0);}
    return clamp((p.eye.xyz+ray*distance).xz-point.xz,vec2<f32>(-128.0),vec2<f32>(128.0));
}
fn slope(uv:vec2<f32>,dx:vec2<f32>,dy:vec2<f32>)->vec2<f32> {
    return (textureSampleGrad(wave_texture,wave_sampler,uv,dx,dy).rg*255.0-128.0)/127.0;
}
// Three independently transformed samples at equilateral-lattice vertices.
// The lattice moves with UV advection: random choices never change per frame.
fn stochastic_slope(uv:vec2<f32>,dx:vec2<f32>,dy:vec2<f32>,salt:u32)->vec2<f32> {
    let skew=vec2<f32>(uv.x-uv.y*0.577350269,uv.y*1.154700538)/1.37;
    let cell=vec2<i32>(floor(skew));let f=fract(skew);
    var vertices=array<vec2<i32>,3>(cell,cell+vec2<i32>(1,0),cell+vec2<i32>(0,1));
    var weights=vec3<f32>(1.0-f.x-f.y,f.x,f.y);
    if f.x+f.y>1.0 {
        vertices=array<vec2<i32>,3>(cell+vec2<i32>(1,1),cell+vec2<i32>(0,1),cell+vec2<i32>(1,0));
        weights=vec3<f32>(f.x+f.y-1.0,1.0-f.x,1.0-f.y);
    }
    // Smooth shared-edge weights avoid a visible triangular derivative crease.
    weights=weights*weights*(3.0-2.0*weights);weights/=dot(weights,vec3<f32>(1.0));
    var gradient=vec2<f32>(0.0);
    for(var vertex=0u;vertex<3u;vertex++) {
        let key=shadow_hash(bitcast<u32>(vertices[vertex].x)^shadow_hash(bitcast<u32>(vertices[vertex].y)^salt));
        let phase=f32(key&65535u)/65536.0*6.283185307;
        let rotation=mat2x2<f32>(vec2<f32>(cos(phase),sin(phase)),vec2<f32>(-sin(phase),cos(phase)));
        let offset=vec2<f32>(f32(shadow_hash(key^0x51ed270bu)&65535u),f32(shadow_hash(key^0x68bc21ebu)&65535u))/65536.0;
        // Rotate both explicit footprints for anisotropic filtering, then pull
        // slopes back through the UV transform before combining normals.
        gradient+=weights[vertex]*(transpose(rotation)*slope(rotation*uv+offset,rotation*dx,rotation*dy));
    }
    // Independent zero-mean slope fields otherwise flatten between vertices.
    return gradient*inverseSqrt(max(dot(weights,weights),0.0001));
}
// Quintic gradient noise and its exact spatial derivative. Time advects a
// smooth field instead of selecting new random phases each frame.
fn warp_gradient(cell:vec2<i32>,salt:u32)->vec2<f32> {
    let h=shadow_hash(bitcast<u32>(cell.x)^shadow_hash(bitcast<u32>(cell.y)^salt));
    let gradients=array<vec2<f32>,8>(vec2<f32>(1.0,0.0),vec2<f32>(-1.0,0.0),vec2<f32>(0.0,1.0),vec2<f32>(0.0,-1.0),vec2<f32>(0.70710678,0.70710678),vec2<f32>(-0.70710678,0.70710678),vec2<f32>(0.70710678,-0.70710678),vec2<f32>(-0.70710678,-0.70710678));
    return gradients[h&7u];
}
fn warp_noise(q:vec2<f32>,salt:u32)->vec3<f32> {
    let cell=vec2<i32>(floor(q));let f=fract(q);
    let g00=warp_gradient(cell,salt);let g10=warp_gradient(cell+vec2<i32>(1,0),salt);
    let g01=warp_gradient(cell+vec2<i32>(0,1),salt);let g11=warp_gradient(cell+vec2<i32>(1,1),salt);
    let n00=dot(g00,f);let n10=dot(g10,f-vec2<f32>(1.0,0.0));
    let n01=dot(g01,f-vec2<f32>(0.0,1.0));let n11=dot(g11,f-vec2<f32>(1.0));
    let u=f*f*f*(f*(f*6.0-15.0)+10.0);let du=30.0*f*f*(f-1.0)*(f-1.0);
    let a=mix(n00,n10,u.x);let b=mix(n01,n11,u.x);
    let gradient=mix(mix(g00,g10,u.x),mix(g01,g11,u.x),u.y)+vec2<f32>(mix(n10-n00,n11-n01,u.y)*du.x,(b-a)*du.y);
    return vec3<f32>(mix(a,b,u.y),gradient);
}
fn octave_waves(world:vec2<f32>,dx:vec2<f32>,dy:vec2<f32>,time:f32,distance:f32)->vec2<f32> {
    var warped=world;var jacobian=mat2x2<f32>(vec2<f32>(1.0,0.0),vec2<f32>(0.0,1.0));
    if p.wave_detail.y>0.0 {
        let rate=0.021;
        let q=world*rate+vec2<f32>(0.006,-0.004)*time;
        let a=warp_noise(q,0x2c1b3c6du);let b=warp_noise(q+vec2<f32>(19.17,-7.63),0x297a2d39u);
        warped+=vec2<f32>(a.x,b.x)*p.wave_detail.y;
        jacobian=mat2x2<f32>(vec2<f32>(1.0+p.wave_detail.y*rate*a.y,p.wave_detail.y*rate*b.y),vec2<f32>(p.wave_detail.y*rate*a.z,1.0+p.wave_detail.y*rate*b.z));
    }
    // Non-dyadic frequency ratios (~1+sqrt(2)) decorrelate the periodic source.
    // Finite floats are not mathematically aperiodic, but no short common tile
    // is deliberately introduced on the block grid.
    let frequencies=array<f32,6>(0.0137,0.0330706,0.0798403,0.192754,0.465395,1.12359);
    let weights=array<f32,6>(0.16,0.21,0.26,0.32,0.34,0.26);
    let orientations=array<vec2<f32>,6>(vec2<f32>(0.9916,0.1293),vec2<f32>(0.9272,0.3746),vec2<f32>(0.7501,0.6613),vec2<f32>(0.9659,-0.2588),vec2<f32>(0.8645,0.5026),vec2<f32>(0.5885,-0.8085));
    let travel=array<vec2<f32>,6>(vec2<f32>(0.9990,0.0447),vec2<f32>(0.9808,0.1951),vec2<f32>(0.9455,0.3256),vec2<f32>(0.9081,0.4187),vec2<f32>(0.9781,-0.2079),vec2<f32>(0.8480,0.5299));
    let distant=smoothstep(max(1.0,p.wave_detail.z*0.25),max(2.0,p.wave_detail.z),distance);
    var result=vec2<f32>(0.0);
    for(var octave=0u;octave<6u;octave++) {
        let axis=normalize(orientations[octave]);let rotation=mat2x2<f32>(axis,vec2<f32>(-axis.y,axis.x));
        let frequency=frequencies[octave];let speed=0.2+0.12*f32(5u-octave);
        let uv=rotation*(warped+travel[octave]*speed*time)*frequency;
        let gx=rotation*(jacobian*dx)*frequency;let gy=rotation*(jacobian*dy)*frequency;
        // Suppress unresolved detail at grazing angles as well as with distance.
        let resolved=1.0-smoothstep(0.12,0.65,max(length(gx),length(gy)));
        let amplitude=weights[octave]*resolved*select(1.0+0.4*distant,1.0-distant,octave>=3u);
        if amplitude<=0.00001 {continue;}
        var sample=vec2<f32>(0.0);
        if p.optics.w>0.5 && octave>=3u {sample=stochastic_slope(uv,gx,gy,shadow_hash(octave^0x63d83595u));}
        else {sample=slope(uv,gx,gy);}
        // Amplitudes specify slope energy (height amplitude scales as 1/f).
        result+=amplitude*(transpose(jacobian)*(transpose(rotation)*sample));
    }
    return result;
}
fn wave_gradient(world:vec2<f32>,dx:vec2<f32>,dy:vec2<f32>,time:f32,depth:f32)->vec2<f32> {
    if p.water.w<0.5 {return vec2<f32>(0.0);}
    if p.wave_detail.x>0.5 {
        return octave_waves(world,dx,dy,time,depth)*p.water.y;
    }
    let rotate=mat2x2<f32>(vec2<f32>(0.8,0.6),vec2<f32>(-0.6,0.8));
    let small_uv=world+vec2<f32>(0.045,0.018)*time;
    let medium_uv=rotate*world*0.23+vec2<f32>(-0.017,0.025)*time;
    var small=vec2<f32>(0.0);var medium=vec2<f32>(0.0);
    if p.optics.w>0.5 {
        small=stochastic_slope(small_uv,dx,dy,0xa511e9b3u);
        medium=transpose(rotate)*stochastic_slope(medium_uv,rotate*dx*0.23,rotate*dy*0.23,0x63d83595u);
    } else {
        small=slope(small_uv,dx,dy);
        medium=transpose(rotate)*slope(medium_uv,rotate*dx*0.23,rotate*dy*0.23);
    }
    let large=slope(world*0.071+vec2<f32>(0.009,-0.012)*time,dx*0.071,dy*0.071);
    let gradient=(small*0.35+medium*0.45+large*0.3)*p.water.y;
    return gradient;
}
fn water_normal(pixel:vec2<u32>,point:vec3<f32>,flat:vec3<f32>,depth:f32)->vec3<f32> {
    if p.water.w<0.5 || abs(flat.y)<0.9 || depth<=0.001 {return flat;}
    let dx=plane_gradient(vec2<f32>(pixel)+vec2<f32>(1.0,0.0),point);
    let dy=plane_gradient(vec2<f32>(pixel)+vec2<f32>(0.0,1.0),point);
    let gradient=wave_gradient(point.xz,dx,dy,p.water.x,depth);
    return normalize(vec3<f32>(-gradient.x,1.0,-gradient.y))*sign(flat.y);
}
