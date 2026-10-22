// Experimental one-bounce voxel cone sampling. No feedback from last frame's
// shaded image: history filters indirect irradiance, never recursively adds it.
@group(0) @binding(4) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read> hits:array<Hit>;
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    let size=gi_size();if any(id.xy>=size) {return;}
    let i=id.x+size.x*id.y;let base=gi_base(p.history.y,i);
    var pixel=min(id.xy*2u,p.screen.xy-1u);var h=hits[pixel.x+p.screen.x*pixel.y];
    // Select an actual visible receiver in this 2x2 block; never discard a
    // thin surface just because the old top-left sample missed it.
    for(var y=0u;y<2u;y++) {for(var x=0u;x<2u;x++) {
        let candidate=min(id.xy*2u+vec2<u32>(x,y),p.screen.xy-1u);
        let hit=hits[candidate.x+p.screen.x*candidate.y];
        if hit.valid!=0u && (h.valid==0u || hit.normal_depth.w<h.normal_depth.w) {pixel=candidate;h=hit;}
    }}
    if (pixel_flags(pixel.x+p.screen.x*pixel.y)&1u)==0u {
        gi_history[base]=vec4<f32>(0.0);gi_history[base+1u]=vec4<f32>(0.0);return;
    }
    let point=point_at(pixel,h.normal_depth.w);let n=h.normal_depth.xyz;
    let seed=gi_sample_seed(point,n);
    var value=gi_evaluate(point,n,seed);var count=1u;
    let clip=p.previous_vp*vec4<f32>(point,1.0);let uv=vec2<f32>(clip.x,-clip.y)/max(clip.w,0.00001)*0.5+0.5;
    if p.gi.w>0.5 && p.history.x==0u && clip.w>0.0 && all(uv>=vec2<f32>(0.0)) && all(uv<vec2<f32>(1.0)) {
        let at=uv*vec2<f32>(p.screen.xy)*0.5-0.5;let cell=vec2<i32>(floor(at));let f=fract(at);
        let footprint=4.0*h.normal_depth.w*p.forward.w/f32(p.screen.y);
        let tolerance=max(0.06,footprint*2.0);var sum=vec3<f32>(0.0);var weights=0.0;var old_count=0.0;
        for(var y=0;y<2;y++) {for(var x=0;x<2;x++) {
            let q=cell+vec2<i32>(x,y);if any(q<vec2<i32>(0)) || any(q>=vec2<i32>(size)) {continue;}
            let old=gi_base(1u-p.history.y,u32(q.x)+size.x*u32(q.y));
            let surface=gi_history[old];let previous=gi_history[old+1u];let key=bitcast<u32>(surface.w);let stamp=bitcast<u32>(previous.w);
            let delta=surface.xyz-point;
            if (stamp>>8u)!=(p.history.w&0x00ffffffu) || (stamp&255u)==0u || (key>>24u)!=h.material || dot(unpack_history_normal(key),n)<0.98 || abs(dot(delta,n))>0.025 || length(delta)>tolerance {continue;}
            let weight=select(1.0-f.x,f.x,x==1)*select(1.0-f.y,f.y,y==1)*exp(-dot(delta,delta)/(tolerance*tolerance))/(0.0001+dot(delta,delta));
            sum+=previous.xyz*weight;old_count+=f32(stamp&255u)*weight;weights+=weight;
        }}
        if weights>0.0001 {
            count=min(u32(round(old_count/weights))+1u,16u);value=mix(sum/weights,value,1.0/f32(count));
        }
    }
    gi_history[base]=vec4<f32>(point,bitcast<f32>(history_surface(h)));
    gi_history[base+1u]=vec4<f32>(value,bitcast<f32>(((p.history.z&0x00ffffffu)<<8u)|count));
}
