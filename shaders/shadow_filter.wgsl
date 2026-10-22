@group(0) @binding(4) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read> hits:array<Hit>;
@group(0) @binding(14) var<storage,read_write> scene_color:array<vec4<f32>>;
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=p.screen.xy) {return;}
    let i=id.x+p.screen.x*id.y;let raw=shadow_raw[i];
    if p.soft.y<0.5 || raw.y<0.5 || (pixel_flags(i)&1u)==0u {return;}
    let h=hits[i];let point=point_at(id.xy,h.normal_depth.w);
    let bank=p.history.y*p.screen.x*p.screen.y*2u;
    let center=shadow_history[bank+i*2u+1u];let variance=max(0.001,center.y-center.x*center.x);
    // Fully stable lit/umbra regions need neither spatial history fetches nor
    // a shading correction. Keep their history available for entering edges.
    if variance<=0.0011 && abs(center.x-raw.x)<0.0001 && (raw.x<0.001 || raw.x>0.999) {
        var uniform=true;
        for(var y=-1;y<=1;y++) {for(var x=-1;x<=1;x++) {
            let q=clamp(vec2<i32>(id.xy)+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(p.screen.xy)-1);
            let j=u32(q.x)+p.screen.x*u32(q.y);
            if shadow_raw[j].y>0.5 && abs(shadow_raw[j].x-raw.x)>0.001 {uniform=false;}
        }}
        if uniform {shadow_raw[i].z=raw.x;return;}
    }
    var sum=0.0;var weights=0.0;
    for(var y=-1;y<=1;y++) {for(var x=-1;x<=1;x++) {
        let q=clamp(vec2<i32>(id.xy)+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(p.screen.xy)-1);
        let j=u32(q.x)+p.screen.x*u32(q.y);
        if shadow_raw[j].y<0.5 {continue;}
        let surface=shadow_history[bank+j*2u];let moments=shadow_history[bank+j*2u+1u];
        if bitcast<u32>(moments.w)!=p.history.z {continue;}
        let key=bitcast<u32>(surface.w);
        if (key>>24u)!=h.material || dot(unpack_history_normal(key),h.normal_depth.xyz)<0.98 {continue;}
        let plane=abs(dot(surface.xyz-point,h.normal_depth.xyz));
        let weight=exp(-plane*32.0-abs(moments.x-center.x)/(sqrt(variance)*4.0+0.1));
        sum+=moments.x*weight;weights+=weight;
    }}
    let filtered=clamp(sum/max(weights,0.00001),0.0,1.0);
    let albedo=hit_color(h,point);
    let fog=1.0-smoothstep(p.up.w*0.65,p.up.w,h.normal_depth.w);
    var color=scene_color[i].xyz;
    if p.screen.z==0u {color+=albedo*direct_light()*water_direct(point,h.normal_depth.xyz)*(filtered-raw.x)*fog;}
    if p.screen.z==7u {color=vec3<f32>(filtered);}
    scene_color[i]=vec4<f32>(color,1.0);shadow_raw[i].z=filtered;
}
