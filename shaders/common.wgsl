// Shared ABI and camera math, prepended by build.rs to every compute stage.
struct Params {
    eye:vec4<f32>, forward:vec4<f32>, right:vec4<f32>, up:vec4<f32>,
    origin:vec4<i32>, dims:vec4<u32>, screen:vec4<u32>,
    sun:vec4<f32>, effects:vec4<f32>, flags:vec4<u32>, meshes:vec4<u32>, water:vec4<f32>, optics:vec4<f32>, scene_min:vec4<f32>, scene_max:vec4<f32>, acceleration:vec4<u32>, light_right:vec4<f32>, light_up:vec4<f32>, cascade_center:vec4<f32>, shadow_controls:vec4<f32>, soft:vec4<f32>, history:vec4<u32>, previous_vp:mat4x4<f32>, sky_light:vec4<f32>, sky_build:vec4<f32>, sky_state:vec4<u32>, display:vec4<f32>, lod:vec4<f32>, gi:vec4<f32>, wave_detail:vec4<f32>, caustics:vec4<f32>, caustic_map:vec4<f32>,
};
// First screen-pixel region: mesh epochs. Second: compact opaque/water validity.
@group(0) @binding(17) var<storage,read_write> aux:array<u32>;
fn pixel_flags(i:u32)->u32 {return aux[p.screen.x*p.screen.y+i];}
fn store_pixel_flags(i:u32,h:Hit) {
    aux[p.screen.x*p.screen.y+i]=select(0u,1u,h.valid!=0u)|select(0u,2u,h.water.y>0.5);
}
struct Hit {
    normal_depth:vec4<f32>, uv:vec2<f32>, material:u32, valid:u32,
    water_normal_depth:vec4<f32>,
    // x: integrated water path in metres, y: water valid, z: camera underwater, w: traversal steps
    water:vec4<f32>,
};
struct Section {occupied:vec2<u32>,opaque:vec2<u32>};
fn ray_at(pixel:vec2<f32>)->vec3<f32> {
    let xy=(pixel+0.5)/vec2<f32>(p.screen.xy)*2.0-1.0;
    return normalize(p.forward.xyz+p.right.xyz*xy.x*p.eye.w*p.forward.w-p.up.xyz*xy.y*p.forward.w);
}
fn point_at(pixel:vec2<u32>,depth:f32)->vec3<f32> {return p.eye.xyz+ray_at(vec2<f32>(pixel))*depth;}
fn miss()->Hit {
    return Hit(vec4<f32>(0.0,0.0,0.0,p.up.w),vec2<f32>(0.0),0u,0u,vec4<f32>(0.0,1.0,0.0,p.up.w),vec4<f32>(0.0));
}

fn sky_color(ray:vec3<f32>)->vec3<f32> {
    var sky=mix(vec3<f32>(0.65,0.79,0.86),vec3<f32>(0.26,0.51,0.77),clamp(ray.y*0.7+0.35,0.0,1.0));
    if p.sky_state.z!=0u {sky=sky_radiance(ray);}
    let radius=select(0.00465,p.soft.x,p.soft.y>0.5);
    let disk_light=select(p.sky_light.xyz,-p.sky_light.xyz,p.soft.w>0.5);
    let disk=smoothstep(cos(radius*1.08),cos(radius*0.92),dot(ray,disk_light));
    let tint=select(vec3<f32>(1.0,0.8,0.5),vec3<f32>(0.65,0.76,1.0),p.soft.w>0.5);
    return sky+tint*disk*p.sun.w;
}

// Uncharted 2 filmic curve, normalized to white=11.2, followed by sRGB OETF.
// Fused into the final sky/water composition dispatch: HDR stays linear through
// lighting, shadow denoising and reflection sampling; UI/debug values bypass it.
fn hable_uncharted2(x:vec3<f32>)->vec3<f32> {
    // John Hable: filmicworlds.com/blog/filmic-tonemapping-operators/
    let a=0.15;let b=0.50;let c=0.10;let d=0.20;let e=0.02;let f=0.30;
    return ((x*(a*x+c*b)+d*e)/(x*(a*x+b)+d*f))-e/f;
}
fn display_color(input:vec3<f32>)->vec3<f32> {
    if p.screen.z!=0u || p.display.y<0.5 {return input;}
    let exposed=max(input,vec3<f32>(0.0))*exp2(clamp(p.display.x,-8.0,8.0));
    let curve=hable_uncharted2(exposed*2.0)/hable_uncharted2(vec3<f32>(11.2));
    let luminance=dot(curve,vec3<f32>(0.2126,0.7152,0.0722));
    let mapped=clamp(mix(vec3<f32>(luminance),curve,clamp(p.display.z,0.0,2.0)),vec3<f32>(0.0),vec3<f32>(1.0));
    return select(1.055*pow(mapped,vec3<f32>(1.0/2.4))-0.055,mapped*12.92,mapped<=vec3<f32>(0.0031308));
}
fn pack_display(pixel:vec2<u32>,input:vec3<f32>)->u32 {
    var color=display_color(input);
    let d=abs(vec2<i32>(pixel)-vec2<i32>(p.screen.xy/2u));
    if p.screen.z==0u && ((d.x<=5 && d.y==0) || (d.y<=5 && d.x==0)) {color=vec3<f32>(1.0)-color;}
    let rgb=vec3<u32>(clamp(color,vec3<f32>(0.0),vec3<f32>(1.0))*255.0);
    return (rgb.x<<16u)|(rgb.y<<8u)|rgb.z;
}

// Linear camera-forward min/max depth. Invalid pixels are (+infinity,-infinity).
@group(0) @binding(18) var<storage,read_write> depth_pyramid:array<vec2<f32>>;
var<push_constant> dispatch:vec4<u32>;
fn depth_level(level:u32)->vec3<u32> {
    var size=p.screen.xy;var offset=0u;
    for(var l=0u;l<level;l++) {offset+=size.x*size.y;size=(size+1u)/2u;}
    return vec3<u32>(size,offset);
}
// Four light-space cascades, separate opaque-voxel and polygon upper depth bounds.
// Zero is negative infinity in ordered-float encoding. Bounds can only reject rays.
@group(0) @binding(19) var<storage,read> shadow_cascades:array<u32>;
fn shadow_bound(i:u32)->u32 {return shadow_cascades[i];}
fn ordered_depth(v:f32)->u32 {
    let bits=bitcast<u32>(v);return select(bits^0x80000000u,~bits,(bits&0x80000000u)!=0u);
}
fn cascade_radius(c:u32)->f32 {return select(16.0*pow(3.0,f32(c)),max(432.0,p.cascade_center.w),c==3u);}
fn cascade_clear(point:vec3<f32>,polygon:bool)->bool {
    if p.acceleration.y==0u {return false;}
    let v=point-p.cascade_center.xyz;
    let xy=vec2<f32>(dot(v,p.light_right.xyz),dot(v,p.light_up.xyz));
    for(var c=0u;c<4u;c++) {
        let r=cascade_radius(c);let uv=xy/(2.0*r)+0.5;
        if any(uv<vec2<f32>(0.0)) || any(uv>=vec2<f32>(1.0)) {continue;}
        let q=min(vec2<u32>(uv*128.0),vec2<u32>(127u));
        let i=q.x+128u*q.y+16384u*(c+select(0u,4u,polygon));
        return shadow_bound(i)<ordered_depth(dot(v,p.sun.xyz)-0.002);
    }
    return false;
}

fn store_linear_depth(i:u32,h:Hit,ray:vec3<f32>) {
    if p.acceleration.x==0u || p.flags.z==0u {return;}
    var bounds=vec2<f32>(1e30,-1e30);
    if h.valid!=0u {bounds=vec2<f32>(dot(ray,p.forward.xyz)*h.normal_depth.w);}
    depth_pyramid[i]=bounds;
}

// Actual first-hit depths are distinct from conservative caster upper bounds.
@group(0) @binding(20) var<storage,read> far_depth:array<u32>;
fn decode_depth(key:u32)->f32 {
    return bitcast<f32>(select(~key,key^0x80000000u,(key&0x80000000u)!=0u));
}
fn cascade_offset(c:u32)->vec2<f32> {
    let texel=2.0*cascade_radius(c)/f32(p.sky_state.w);
    let center=vec2<f32>(dot(p.cascade_center.xyz,p.light_right.xyz),dot(p.cascade_center.xyz,p.light_up.xyz));
    return floor(center/texel+0.5)*texel-center;
}
fn cascade_ray_origin(pixel:vec2<u32>,c:u32)->vec3<f32> {
    let xy=((vec2<f32>(pixel)+0.5)/f32(p.sky_state.w)*2.0-1.0)*cascade_radius(c)+cascade_offset(c);
    return p.cascade_center.xyz+p.light_right.xyz*xy.x+p.light_up.xyz*xy.y+p.sun.xyz*p.shadow_controls.z;
}
fn shadow_blend(point:vec3<f32>)->f32 {
    if p.shadow_controls.y<0.5 {return 0.0;}
    if p.shadow_controls.y>1.5 {return 1.0;}
    let end=max(0.001,p.shadow_controls.x);
    return smoothstep(max(0.0,end-8.0),end,distance(point,p.eye.xyz));
}
// Single-mip depth comparisons are bilinearly reconstructed before filtering.
fn shadow_key(pixel:vec2<i32>,c:u32,polygon:bool)->u32 {
    if any(pixel<vec2<i32>(0)) || any(pixel>=vec2<i32>(i32(p.sky_state.w))) {return 0u;}
    return far_depth[u32(pixel.x+pixel.y*i32(p.sky_state.w))+(p.sky_state.w*p.sky_state.w)*(c+select(0u,4u,polygon))];
}
fn shadow_compare(pixel:vec2<i32>,xy:vec2<f32>,plane:vec2<f32>,z:f32,r:f32,c:u32,polygon:bool)->f32 {
    let sample_xy=((vec2<f32>(pixel)+0.5)/f32(p.sky_state.w)*2.0-1.0)*r;
    let receiver=z-dot(plane,sample_xy-xy)+0.02;
    let key=shadow_key(pixel,c,polygon);
    return select(0.0,1.0,key==0u || decode_depth(key)<=receiver);
}
fn shadow_bilinear(at:vec2<f32>,xy:vec2<f32>,plane:vec2<f32>,z:f32,r:f32,c:u32,polygon:bool)->f32 {
    let base=vec2<i32>(floor(at));let f=fract(at);
    let a=shadow_compare(base,xy,plane,z,r,c,polygon);
    let b=shadow_compare(base+vec2<i32>(1,0),xy,plane,z,r,c,polygon);
    let d=shadow_compare(base+vec2<i32>(0,1),xy,plane,z,r,c,polygon);
    let e=shadow_compare(base+vec2<i32>(1,1),xy,plane,z,r,c,polygon);
    return mix(mix(a,b,f.x),mix(d,e,f.x),f.y);
}
fn shadow_search_world(point:vec3<f32>,polygon:bool)->f32 {
    let top=select(p.scene_max.w,p.scene_max.y,polygon);
    let gap=max(0.0,top-point.y)/max(p.sun.y-tan(p.soft.x),0.01);
    return tan(p.soft.x)*gap;
}
fn shadow_pattern_angle(point:vec3<f32>)->f32 {
    let delta=point-p.eye.xyz;let depth=max(0.001,dot(delta,p.forward.xyz));
    let uv=vec2<f32>(dot(delta,p.right.xyz)/(depth*p.eye.w*p.forward.w),-dot(delta,p.up.xyz)/(depth*p.forward.w))*0.5+0.5;
    let pixel=vec2<i32>(floor(uv*vec2<f32>(p.screen.xy)));
    let seed=shadow_hash(bitcast<u32>(pixel.x)^shadow_hash(bitcast<u32>(pixel.y))^p.history.z*0x9e3779b9u);
    return f32(seed&0x00ffffffu)/16777216.0*6.283185307;
}
fn sample_shadow_cascade(point:vec3<f32>,normal:vec3<f32>,polygon:bool,c:u32)->f32 {
    let v=point-p.cascade_center.xyz;let r=cascade_radius(c);
    let xy=vec2<f32>(dot(v,p.light_right.xyz),dot(v,p.light_up.xyz))-cascade_offset(c);
    let center=(xy/(2.0*r)+0.5)*f32(p.sky_state.w)-0.5;
    let q=vec2<i32>(floor(center+0.5));let texel=2.0*r/f32(p.sky_state.w);
    let plane=vec2<f32>(dot(normal,p.light_right.xyz),dot(normal,p.light_up.xyz))/max(dot(normal,p.sun.xyz),0.01);
    let z=dot(v,p.sun.xyz);
    if p.soft.y<0.5 {return shadow_bilinear(center,xy,plane,z,r,c,polygon);}
    var blocker_sum=0.0;var blockers=0.0;
    let search=max(1.0,shadow_search_world(point,polygon)/texel);
    // Always cover the local neighborhood, then search the physical disk cone.
    // Missing centre texels do not imply that the penumbra is empty.
    for(var i=0u;i<25u;i++) {
        var pixel=q+vec2<i32>(i32(i%3u)-1,i32(i/3u)-1);
        if i>=9u {
            if search<=1.5 {break;}
            let k=f32(i-9u);let angle=k*2.39996323;
            pixel=vec2<i32>(floor(center+vec2<f32>(cos(angle),sin(angle))*sqrt((k+0.5)/16.0)*search+0.5));
        }
        let key=shadow_key(pixel,c,polygon);
        let sample_xy=((vec2<f32>(pixel)+0.5)/f32(p.sky_state.w)*2.0-1.0)*r;
        let receiver=z-dot(plane,sample_xy-xy);
        if key!=0u {let gap=decode_depth(key)-receiver;if gap>0.02 {blocker_sum+=gap;blockers+=1.0;}}
    }
    if blockers==0.0 {return 1.0;}
    let footprint=max(0.5,tan(p.soft.x)*(blocker_sum/blockers)/texel);
    if footprint<=0.5 {return shadow_bilinear(center,xy,plane,z,r,c,polygon);}
    // Bounded sampling cost, unbounded physical width: widening the disk never
    // saturates at four texels. Wide filters rotate and accumulate temporally.
    let count=select(16u,32u,footprint>4.0);
    let phase=select(0.35,shadow_pattern_angle(point),footprint>4.0);
    var visibility=0.0;
    for(var i=0u;i<count;i++) {
        let angle=f32(i)*2.39996323+phase;
        let offset=vec2<f32>(cos(angle),sin(angle))*sqrt((f32(i)+0.5)/f32(count))*footprint;
        visibility+=shadow_bilinear(center+offset,xy,plane,z,r,c,polygon);
    }
    return visibility/f32(count);
}
fn far_sunlight(point:vec3<f32>,normal:vec3<f32>,polygon:bool)->f32 {
    if p.soft.y<0.5 && cascade_clear(point,polygon) {return 1.0;}
    let v=point-p.cascade_center.xyz;
    let xy=vec2<f32>(dot(v,p.light_right.xyz),dot(v,p.light_up.xyz));
    for(var c=0u;c<4u;c++) {
        let r=cascade_radius(c);let guard=select(0.0,shadow_search_world(point,polygon),p.soft.y>0.5);
        let extent=(max(abs(xy.x-cascade_offset(c).x),abs(xy.y-cascade_offset(c).y))+guard)/r;
        if extent>=0.98 {continue;}
        let a=sample_shadow_cascade(point,normal,polygon,c);
        if c<3u {return mix(a,sample_shadow_cascade(point,normal,polygon,c+1u),smoothstep(0.75,0.95,extent));}
        return a;
    }
    // Beyond loaded cascade coverage, do not start an unbounded distant ray.
    return 1.0;
}

@group(0) @binding(21) var<storage,read_write> shadow_history:array<vec4<f32>>;
@group(0) @binding(22) var<storage,read> sky_cache:array<vec4<f32>>;
@group(0) @binding(23) var<storage,read_write> shadow_raw:array<vec4<f32>>;
const SKY_BANK:u32=558604u; // sky + importance CDF + 128 row reductions + final SH/direct
// Orthonormal real SH, y-up. Cache contains radiance coefficients, not display colors.
fn sky_basis(d:vec3<f32>,i:u32)->f32 {
    switch i {
        case 0u: {return 0.282094792;}
        case 1u: {return 0.488602512*d.x;}
        case 2u: {return 0.488602512*d.y;}
        case 3u: {return 0.488602512*d.z;}
        case 4u: {return 1.092548431*d.x*d.y;}
        case 5u: {return 1.092548431*d.y*d.z;}
        case 6u: {return 0.315391565*(3.0*d.y*d.y-1.0);}
        case 7u: {return 1.092548431*d.x*d.z;}
        default: {return 0.546274215*(d.x*d.x-d.z*d.z);}
    }
}
fn sky_diffuse(n:vec3<f32>)->vec3<f32> {
    if p.sky_state.z==0u {return vec3<f32>(0.21,0.25,0.30)*(0.7+0.3*max(n.y,0.0))*mix(0.04,1.0,smoothstep(-0.12,0.15,p.sky_light.y));}
    var e=vec3<f32>(0.0);let base=p.sky_state.x*SKY_BANK+557184u+1280u;
    for(var i=0u;i<9u;i++) {
        let convolution=select(select(0.25,0.666666667,i<4u),1.0,i==0u);
        e+=sky_cache[base+i].xyz*sky_basis(n,i)*convolution;
    }
    // Uniform Lambertian ground radiance integrates analytically over the
    // lower hemisphere; this avoids low-order SH ringing at the horizon.
    let ground=sky_cache[p.sky_state.x*SKY_BANK+558603u].xyz;
    let bounce=ground*(0.5*(1.0-clamp(n.y,-1.0,1.0)))*p.display.w;
    return (max(e,vec3<f32>(0.0))+bounce)*(p.sky_light.w*0.8*3.141592654/22.0);
}
fn direct_light()->vec3<f32> {
    if p.sky_state.z==0u {return select(vec3<f32>(1.0,0.94,0.82),vec3<f32>(0.65,0.76,1.0),p.soft.w>0.5)*p.sun.w*0.8;}
    return sky_cache[p.sky_state.x*SKY_BANK+557184u+1289u].xyz*(p.sky_light.w*0.8/22.0);
}
fn sky_radiance(ray:vec3<f32>)->vec3<f32> {
    let uv=vec2<f32>(atan2(ray.z,ray.x)/6.283185307+0.5,acos(clamp(ray.y,-1.0,1.0))/3.141592654);
    let q=uv*vec2<f32>(1024.0,512.0)-0.5;let base=vec2<i32>(floor(q));let f=fract(q);
    var color=vec3<f32>(0.0);
    for(var y=0;y<2;y++) {for(var x=0;x<2;x++) {
        let pixel=vec2<u32>(u32((base.x+x+1024)%1024),u32(clamp(base.y+y,0,511)));
        let weight=select(1.0-f.x,f.x,x==1)*select(1.0-f.y,f.y,y==1);
        color+=sky_cache[p.sky_state.x*SKY_BANK+pixel.x+1024u*pixel.y].xyz*weight;
    }}
    return color;
}
fn shadow_hash(value:u32)->u32 {
    var h=value;h=(h^(h>>16u))*0x7feb352du;h=(h^(h>>15u))*0x846ca68bu;return h^(h>>16u);
}
fn disk_direction(point:vec3<f32>,sample:u32)->vec3<f32> {
    let to_eye=point-p.eye.xyz;let depth=max(0.001,dot(to_eye,p.forward.xyz));
    let uv=vec2<f32>(dot(to_eye,p.right.xyz)/(depth*p.eye.w*p.forward.w),-dot(to_eye,p.up.xyz)/(depth*p.forward.w))*0.5+0.5;
    let pixel=vec2<i32>(floor(uv*vec2<f32>(p.screen.xy)));
    let seed=shadow_hash(bitcast<u32>(pixel.x)^shadow_hash(bitcast<u32>(pixel.y))^p.history.z*0x9e3779b9u);
    let u=(f32(shadow_hash(seed+sample*2u)&0x00ffffffu)+0.5)/16777216.0;
    let v=(f32(shadow_hash(seed+sample*2u+1u)&0x00ffffffu)+0.5)/16777216.0;
    let radius=sqrt(u)*tan(p.soft.x);let angle=6.283185307*v;
    return normalize(p.sun.xyz+radius*(p.light_right.xyz*cos(angle)+p.light_up.xyz*sin(angle)));
}
fn penumbra(point:vec3<f32>,normal:vec3<f32>,polygon:bool)->bool {
    if p.soft.y<0.5 || p.effects.w<0.5 || dot(normal,p.sun.xyz)<=0.0 {return false;}
    let visibility=far_sunlight(point,normal,polygon);
    return visibility>0.001 && visibility<0.999;
}
fn history_surface(h:Hit)->u32 {
    let n=vec3<u32>(clamp(h.normal_depth.xyz*127.0+128.0,vec3<f32>(0.0),vec3<f32>(255.0)));
    return n.x|(n.y<<8u)|(n.z<<16u)|(h.material<<24u);
}
fn unpack_history_normal(key:u32)->vec3<f32> {return (vec3<f32>(f32(key&255u),f32((key>>8u)&255u),f32((key>>16u)&255u))-128.0)/127.0;}

@group(0) @binding(27) var<storage,read> entity_players:array<vec4<f32>>;
fn entity_in_range(point:vec3<f32>)->bool {
    if entity_players[0].y<0.5 {return true;}
    for(var i=0u;i<bitcast<u32>(entity_players[0].x);i++) {
        if all(abs(point-entity_players[i+1u].xyz)<=vec3<f32>(32.0)) {return true;}
    }
    return false;
}

// Half-resolution, two banks, two vec4s/sample: world surface/key and RGB/stamp.
@group(0) @binding(28) var<storage,read_write> gi_history:array<vec4<f32>>;
fn gi_size()->vec2<u32> {return (p.screen.xy+1u)/2u;}
fn gi_base(bank:u32,index:u32)->u32 {let size=gi_size();return (bank*size.x*size.y+index)*2u;}
fn gi_ambient(pixel:vec2<u32>,h:Hit)->vec3<f32> {
    let size=gi_size();let point=point_at(pixel,h.normal_depth.w);
    let q=vec2<i32>(floor(vec2<f32>(pixel)*0.5));var sum=vec3<f32>(0.0);var weights=0.0;
    for(var y=-1;y<=1;y++) {for(var x=-1;x<=1;x++) {
        let c=clamp(q+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(size)-1);
        let base=gi_base(p.history.y,u32(c.x)+size.x*u32(c.y));
        let surface=gi_history[base];let value=gi_history[base+1u];let key=bitcast<u32>(surface.w);
        let stamp=bitcast<u32>(value.w);
        if (stamp>>8u)!=(p.history.z&0x00ffffffu) || (stamp&255u)==0u || (key>>24u)!=h.material || dot(unpack_history_normal(key),h.normal_depth.xyz)<0.98 {continue;}
        let plane=abs(dot(surface.xyz-point,h.normal_depth.xyz));
        let weight=exp(-plane*16.0-distance(surface.xyz,point)*0.25);
        sum+=value.xyz*weight;weights+=weight;
    }}
    // Thin/disoccluded geometry without compatible half-res support stays stable.
    if weights<0.001 {return sky_diffuse(h.normal_depth.xyz);}
    return sum/weights;
}

// Far: height + four density planes. Near: height + up to five density planes.
@group(0) @binding(29) var<storage,read> caustic_data:array<u32>;
fn caustic_load(i:u32)->u32 {return caustic_data[i];}
const CAUSTIC_SIDE:u32=256u;
const CAUSTIC_PIXELS:u32=65536u;
fn caustic_depth(layer:u32)->f32 {return array<f32,4>(1.0,3.0,7.0,15.0)[layer];}
fn caustic_near()->bool {return (u32(p.caustic_map.w)&1u)!=0u;}
fn caustic_adaptive()->bool {return (u32(p.caustic_map.w)&2u)!=0u;}
fn caustic_shallow()->bool {return (u32(p.caustic_map.w)&4u)!=0u;}
fn caustic_origin(map:u32)->vec2<f32> {
    if map==1u {return floor(p.eye.xz/2.0)*2.0-vec2<f32>(16.0);}
    return p.caustic_map.xy;
}
fn caustic_cell(map:u32)->f32 {return select(p.caustic_map.z,0.125,map==1u);}
fn caustic_base(map:u32)->u32 {return map*5u*CAUSTIC_PIXELS;}
fn caustic_layers(map:u32)->u32 {return select(4u,5u,map==1u && caustic_shallow());}
fn caustic_layer_depth(map:u32,layer:u32)->f32 {
    if map==1u && caustic_shallow() {return array<f32,5>(0.25,0.5,1.0,2.0,4.0)[layer];}
    return caustic_depth(layer);
}
fn caustics_enabled()->bool {return p.caustics.x>0.5 && p.caustics.y>0.0 && p.water.z>0.5 && p.water.w>0.5 && p.water.y>0.0001 && p.flags.w!=0u && p.sun.y>0.06;}
