@group(0) @binding(1) var<storage,read_write> hits:array<Hit>;
@group(0) @binding(4) var<uniform> p:Params;
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=p.screen.x || id.y>=p.screen.y {return;}
    let i=id.x+p.screen.x*id.y;
    let ray=ray_at(vec2<f32>(id.xy));
    let h=trace(p.eye.xyz,ray,p.up.w,true);
    hits[i]=h;store_pixel_flags(i,h);store_linear_depth(i,h,ray);
}
