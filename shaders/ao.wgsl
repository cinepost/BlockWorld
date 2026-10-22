const KERNEL=array<vec3<f32>,12>(
vec3<f32>(0.276483039,0.000000000,0.051562500),
vec3<f32>(-0.245421755,0.224826535,0.085937500),
vec3<f32>(0.033688724,-0.383867662,0.128645833),
vec3<f32>(0.263419107,0.343583089,0.179687500),
vec3<f32>(-0.467169246,-0.082635219,0.239062500),
vec3<f32>(0.428931816,-0.272852003,0.306770833),
vec3<f32>(-0.138375606,0.514753187,0.382812500),
vec3<f32>(-0.251749446,-0.484726334,0.467187500),
vec3<f32>(0.511795324,0.186905765,0.559895833),
vec3<f32>(-0.484797183,0.200118403,0.660937500),
vec3<f32>(0.202341733,-0.432395118,0.770312500),
vec3<f32>(0.115872717,0.369417211,0.888020833)
);
@group(0) @binding(1) var<storage,read> hits:array<Hit>;
@group(0) @binding(4) var<uniform> p:Params;
@group(0) @binding(7) var<storage,read_write> ao:array<f32>;
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=p.screen.x || id.y>=p.screen.y {return;}
    let index=id.x+p.screen.x*id.y;
    if (pixel_flags(index)&1u)==0u || p.flags.z==0u {ao[index]=1.0;return;}
    let h=hits[index];
    let center=point_at(id.xy,h.normal_depth.w);
    let n=h.normal_depth.xyz;
    if p.acceleration.x!=0u {
        let nearest=depth_pyramid[p.acceleration.z].x;
        // Every corrected kernel tap lies inside the configured world-space sphere.
        let max_forward=p.effects.y;
        if dot(center-p.eye.xyz,p.forward.xyz)+max_forward<nearest+0.035-0.0001 {
            ao[index]=1.0;return;
        }
    }
    let helper=select(vec3<f32>(0.0,1.0,0.0),vec3<f32>(1.0,0.0,0.0),abs(n.y)>0.9);
    let tangent=normalize(cross(helper,n));let bitangent=cross(n,tangent);
    let angle=fract(sin(dot(vec2<f32>(id.xy),vec2<f32>(12.9898,78.233)))*43758.5453)*6.283185;
    let radius=p.effects.y;var occlusion=0.0;
    let rotated_tangent=tangent*cos(angle)+bitangent*sin(angle);
    let rotated_bitangent=-tangent*sin(angle)+bitangent*cos(angle);
    // Project a rotated hemisphere kernel. Depth in Hit is ray length, not view-space Z.
    for(var k=0u;k<12u;k++) {
        let tap=KERNEL[k];
        // Old lateral reach was only .55 * radius. Preserve its stratification
        // while making radius the lateral footprint in metres, inside a sphere.
        let lateral=tap.xy/0.55;
        let direction=vec3<f32>(lateral,tap.z*sqrt(max(0.0,1.0-dot(lateral,lateral))));
        let sample_point=center+(rotated_tangent*direction.x+rotated_bitangent*direction.y+n*direction.z)*radius;
        let v=sample_point-p.eye.xyz;let depth=dot(v,p.forward.xyz);
        if depth<=0.01 {continue;}
        let ndc=vec2<f32>(dot(v,p.right.xyz)/(depth*p.eye.w*p.forward.w),-dot(v,p.up.xyz)/(depth*p.forward.w));
        let uv=ndc*0.5+0.5;
        if any(uv<vec2<f32>(0.0)) || any(uv>=vec2<f32>(1.0)) {continue;}
        let pixel=vec2<u32>(uv*vec2<f32>(p.screen.xy));
        var observed=0.0;var surface=vec3<f32>(0.0);
        if p.acceleration.x!=0u {
            // Inspect a footprint proportional to this kernel sample's screen displacement.
            // Only the minimum can prove the entire footprint unoccluding; never average depth.
            let delta=max(abs(i32(pixel.x)-i32(id.x)),abs(i32(pixel.y)-i32(id.y)));
            if delta>=8 {
                let level=select(2u,4u,delta>=32);
                let mip=depth_level(level);let q=pixel>>vec2<u32>(level);
                let bounds=depth_pyramid[mip.z+q.x+q.y*mip.x];
                if bounds.x>=depth-0.035+0.0001 {continue;}
            }
            observed=depth_pyramid[pixel.x+p.screen.x*pixel.y].x;
            if observed>=depth-0.035+0.0001 {continue;}
            let neighbor=hits[pixel.x+p.screen.x*pixel.y];
            if neighbor.valid==0u {continue;}
            surface=point_at(pixel,neighbor.normal_depth.w);
            observed=dot(surface-p.eye.xyz,p.forward.xyz);
        } else {
            let neighbor=hits[pixel.x+p.screen.x*pixel.y];
            if neighbor.valid==0u {continue;}
            surface=point_at(pixel,neighbor.normal_depth.w);
            observed=dot(surface-p.eye.xyz,p.forward.xyz);
        }
        // A projected tap reads the center of a discrete pixel, not its exact UV.
        // At grazing angles this can put a coplanar surface in front of the tap.
        // Only geometry in the receiver's outward hemisphere can occlude it.
        if dot(surface-center,n)<=0.001 {continue;}
        let range=1.0-smoothstep(radius*0.8,radius,distance(surface,center));
        if observed<depth-0.035 {occlusion+=range;}
    }
    ao[index]=clamp(1.0-p.effects.x*occlusion/12.0,0.15,1.0);
}
