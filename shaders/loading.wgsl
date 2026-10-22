// Startup image -> swapchain buffer. No world resources or rendered-image readback.
struct LoadingParams {screen:vec4<u32>, progress:vec4<f32>};
@group(0) @binding(0) var<storage,read> image:array<u32>;
@group(0) @binding(1) var<storage,read_write> output:array<u32>;
var<push_constant> p:LoadingParams;
fn texel(q:vec2<i32>)->vec3<f32> {
    let xy=vec2<u32>(clamp(q,vec2<i32>(0),vec2<i32>(p.screen.zw)-1));
    let c=image[xy.x+p.screen.z*xy.y];
    return vec3<f32>(f32((c>>16u)&255u),f32((c>>8u)&255u),f32(c&255u))/255.0;
}
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=p.screen.xy) {return;}
    let screen=vec2<f32>(p.screen.xy);let source=vec2<f32>(p.screen.zw);
    let scale=min(screen.x/source.x,screen.y/source.y);
    let size=source*scale;let origin=(screen-size)*0.5;
    let pixel=vec2<f32>(id.xy)+0.5;let uv=(pixel-origin)/size;
    var color=vec3<f32>(0.012,0.02,0.025);
    if all(uv>=vec2<f32>(0.0)) && all(uv<vec2<f32>(1.0)) {
        let q=uv*source-0.5;let base=vec2<i32>(floor(q));let f=fract(q);
        color=mix(mix(texel(base),texel(base+vec2<i32>(1,0)),f.x),mix(texel(base+vec2<i32>(0,1)),texel(base+vec2<i32>(1,1)),f.x),f.y);
    }
    let half_width=size.x*0.25;let height=clamp(size.y*0.004,3.0,6.0);
    let delta=pixel-vec2<f32>(screen.x*0.5,origin.y+size.y*0.65);
    if abs(delta.x)<=half_width+1.0 && abs(delta.y)<=height*0.5+1.0 {color=mix(color,vec3<f32>(0.02,0.025,0.02),0.85);}
    if abs(delta.x)<=half_width && abs(delta.y)<=height*0.5 {
        color=vec3<f32>(0.12,0.14,0.12);
        if delta.x+half_width<=2.0*half_width*clamp(p.progress.x,0.0,1.0) {color=vec3<f32>(1.0,0.64,0.22);}
    }
    let rgb=vec3<u32>(clamp(color,vec3<f32>(0.0),vec3<f32>(1.0))*255.0);
    output[id.x+p.screen.x*id.y]=(rgb.x<<16u)|(rgb.y<<8u)|rgb.z;
}
