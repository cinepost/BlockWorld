@group(0) @binding(4) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read> hits:array<Hit>;
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=p.screen.xy) {return;}
    let i=id.x+p.screen.x*id.y;let raw=shadow_raw[i];
    if p.soft.y<0.5 || raw.y<0.5 || (pixel_flags(i)&1u)==0u {return;}
    let h=hits[i];let point=point_at(id.xy,h.normal_depth.w);let key=history_surface(h);
    var mean=raw.x;var second=mean*mean;var count=1.0;
    let clip=p.previous_vp*vec4<f32>(point,1.0);
    let uv=vec2<f32>(clip.x,-clip.y)/max(clip.w,0.00001)*0.5+0.5;
    if p.history.x==0u && clip.w>0.0 && all(uv>=vec2<f32>(0.0)) && all(uv<vec2<f32>(1.0)) {
        let q=vec2<u32>(uv*vec2<f32>(p.screen.xy));let old=q.x+p.screen.x*q.y;
        let base=(1u-p.history.y)*p.screen.x*p.screen.y*2u+old*2u;
        let surface=shadow_history[base];let moments=shadow_history[base+1u];
        let old_key=bitcast<u32>(surface.w);
        if bitcast<u32>(moments.w)==p.history.w && (old_key>>24u)==h.material && dot(unpack_history_normal(old_key),h.normal_depth.xyz)>0.98 && distance(surface.xyz,point)<max(0.035,h.normal_depth.w*0.002) {
            var lo=raw.x;var hi=raw.x;
            for(var y=-1;y<=1;y++) {for(var x=-1;x<=1;x++) {
                let pixel=clamp(vec2<i32>(id.xy)+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(p.screen.xy)-1);
                let j=u32(pixel.x)+p.screen.x*u32(pixel.y);
                if shadow_raw[j].y>0.5 {lo=min(lo,shadow_raw[j].x);hi=max(hi,shadow_raw[j].x);}
            }}
            let previous=clamp(moments.x,lo-0.1,hi+0.1);
            count=min(moments.z+1.0,16.0);let alpha=max(1.0/count,0.08);
            mean=mix(previous,raw.x,alpha);second=mix(moments.y,raw.x*raw.x,alpha);
        }
    }
    let base=p.history.y*p.screen.x*p.screen.y*2u+i*2u;
    shadow_history[base]=vec4<f32>(point,bitcast<f32>(key));
    shadow_history[base+1u]=vec4<f32>(mean,second,count,bitcast<f32>(p.history.z));
}
