// Single-scattering Rayleigh/Mie integration; reference: wwwtyro/glsl-atmosphere
// (Unlicense). Kilometres avoid cancellation in Earth-radius sphere intersections.
@group(0) @binding(4) var<uniform> p:Params;
fn sphere(o:vec3<f32>,d:vec3<f32>,radius:f32)->vec2<f32> {
    let b=dot(o,d);let c=dot(o,o)-radius*radius;let disc=b*b-c;
    if disc<0.0 {return vec2<f32>(1e20,-1e20);}
    return vec2<f32>(-b-sqrt(disc),-b+sqrt(disc));
}
fn scatter(ray:vec3<f32>,light:vec3<f32>,intensity:f32)->vec3<f32> {
    let origin=vec3<f32>(0.0,6371.0+p.sky_build.w*0.001,0.0);
    var distance=sphere(origin,ray,6471.0).y;
    let ground=sphere(origin,ray,6371.0).x;
    if ground>0.0 {distance=min(distance,ground);}
    let step=max(distance,0.0)/16.0;var optical=vec2<f32>(0.0);
    var rayleigh=vec3<f32>(0.0);var mie=vec3<f32>(0.0);
    let beta=vec3<f32>(0.0055,0.0130,0.0224);let beta_m=0.021;
    for(var i=0u;i<16u;i++) {
        let pos=origin+ray*(f32(i)+0.5)*step;
        let height=max(0.0,length(pos)-6371.0);
        let density=exp(-height/vec2<f32>(8.0,1.2))*step;optical+=density;
        let planet=sphere(pos,light,6371.0);
        if planet.x>0.0 {continue;}
        let light_step=max(0.0,sphere(pos,light,6471.0).y)/8.0;
        var light_depth=vec2<f32>(0.0);
        for(var j=0u;j<8u;j++) {
            let h=max(0.0,length(pos+light*(f32(j)+0.5)*light_step)-6371.0);
            light_depth+=exp(-h/vec2<f32>(8.0,1.2))*light_step;
        }
        let attenuation=exp(-beta*(optical.x+light_depth.x)-beta_m*(optical.y+light_depth.y));
        rayleigh+=density.x*attenuation;mie+=density.y*attenuation;
    }
    let mu=dot(ray,light);let g=0.758;let gg=g*g;
    let phase_r=3.0/(16.0*3.141592654)*(1.0+mu*mu);
    let phase_m=3.0/(8.0*3.141592654)*((1.0-gg)*(1.0+mu*mu))/(pow(max(0.001,1.0+gg-2.0*mu*g),1.5)*(2.0+gg));
    return intensity*(phase_r*beta*rayleigh+phase_m*beta_m*mie);
}
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    let y=id.y+dispatch.x;if id.x>=1024u || y>=512u {return;}
    let phi=((f32(id.x)+0.5)/1024.0-0.5)*6.283185307;
    let theta=(f32(y)+0.5)/512.0*3.141592654;
    let ray=vec3<f32>(cos(phi)*sin(theta),cos(theta),sin(phi)*sin(theta));
    var color=scatter(ray,p.sky_build.xyz,22.0);
    if p.sky_build.y<0.1 {color+=scatter(ray,-p.sky_build.xyz,0.035);}
    color+=vec3<f32>(0.0003,0.0005,0.001);
    sky_cache[p.sky_state.y*SKY_BANK+id.x+y*1024u]=vec4<f32>(color,1.0);
}
