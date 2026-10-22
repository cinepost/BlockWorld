// Metadata: current xy/page/flags, next xy/page/blend, RGB tint + reserved.
@group(0) @binding(3) var<storage,read> texture_records:array<u32>;
@group(0) @binding(24) var pack_images:binding_array<texture_2d<f32>,8>;
@group(0) @binding(25) var pack_sampler:sampler;
fn texture_face(normal:vec3<f32>)->u32 {
    if normal.y>0.5 {return 2u;}if normal.y< -0.5 {return 3u;}
    if normal.x>0.5 {return 0u;}if normal.x< -0.5 {return 1u;}
    return select(5u,4u,normal.z>0.0);
}
fn texture_record(material:u32,normal:vec3<f32>)->u32 {return ((material-1u)*6u+texture_face(normal))*12u;}
fn pack_texel(page:u32,origin:vec2<u32>,uv:vec2<f32>,linear:bool,lod:f32)->vec4<f32> {
    let local=fract(uv)*16.0;
    if linear {
        let size=vec2<f32>(textureDimensions(pack_images[page]));
        return textureSampleLevel(pack_images[page],pack_sampler,(vec2<f32>(origin)+local)/size,clamp(lod,0.0,0.25));
    }
    return textureLoad(pack_images[page],vec2<i32>(origin+min(vec2<u32>(local),vec2<u32>(15u))),0);
}
fn texture_color_lod(material:u32,uv:vec2<f32>,normal:vec3<f32>,lod:f32)->vec4<f32> {
    let r=texture_record(material,normal);let flags=texture_records[r+3u];
    var mapped_uv=uv;
    // Voxel side UVs use world-up; PNG V points down. Mesh card UVs already
    // have the correct orientation, and generated fallbacks retain their look.
    if (flags&2u)!=0u && (material<=6u || material>=12u) && abs(normal.y)<0.5 {
        mapped_uv.y=1.0-fract(uv.y)-0.0000001;
    }
    let a=pack_texel(texture_records[r+2u],vec2<u32>(texture_records[r],texture_records[r+1u]),mapped_uv,(flags&4u)!=0u,lod);
    let blend=bitcast<f32>(texture_records[r+7u]);var color=a;
    if blend>0.0 {
        let b=pack_texel(texture_records[r+6u],vec2<u32>(texture_records[r+4u],texture_records[r+5u]),mapped_uv,(flags&4u)!=0u,lod);
        color=mix(a,b,blend);
    }
    let tint=vec3<f32>(bitcast<f32>(texture_records[r+8u]),bitcast<f32>(texture_records[r+9u]),bitcast<f32>(texture_records[r+10u]));
    return vec4<f32>(color.rgb*tint,color.a);
}
// Compute shaders have no implicit derivatives. Differentiate the camera ray's
// intersection with the surface plane, before UV wrapping or atlas coordinates.
// One world-space block face repeats the 16x16 sprite once per metre.
fn block_texture_lod(point:vec3<f32>,normal:vec3<f32>)->f32 {
    let delta=point-p.eye.xyz;let depth=dot(delta,p.forward.xyz);
    let plane=dot(normal,delta);
    if depth<=0.0 || abs(plane)<0.000001 {return 0.25;}
    let dx=p.right.xyz*(2.0*p.eye.w*p.forward.w/f32(p.screen.x));
    let dy=p.up.xyz*(-2.0*p.forward.w/f32(p.screen.y));
    let px=depth*(dx-delta*(dot(normal,dx)/plane));
    let py=depth*(dy-delta*(dot(normal,dy)/plane));
    var ux=px.xz;var uy=py.xz;
    if abs(normal.x)>0.5 {ux=px.zy;uy=py.zy;}
    if abs(normal.z)>0.5 {ux=px.xy;uy=py.xy;}
    let rho=16.0*max(length(ux),length(uy));
    // Only the sign is needed without mipmaps; the sampler clamps positive
    // lambda to .25 and uses linear minification, retaining base mip zero.
    return log2(max(rho,0.000001));
}
fn texture_color(material:u32,uv:vec2<f32>,normal:vec3<f32>)->vec4<f32> {
    return texture_color_lod(material,uv,normal,0.0);
}
fn texture_color_at(material:u32,uv:vec2<f32>,normal:vec3<f32>,point:vec3<f32>)->vec4<f32> {
    return texture_color_lod(material,uv,normal,block_texture_lod(point,normal));
}
fn texture_pixel(material:u32,uv:vec2<f32>)->u32 {
    let color=vec4<u32>(round(clamp(texture_color(material,uv,vec3<f32>(1.0,0.0,0.0)),vec4<f32>(0.0),vec4<f32>(1.0))*255.0));
    return (color.a<<24u)|(color.r<<16u)|(color.g<<8u)|color.b;
}
fn texture_covered(material:u32,uv:vec2<f32>,normal:vec3<f32>,point:vec3<f32>)->bool {
    let flags=texture_records[texture_record(material,normal)+3u];
    return p.meshes.w==0u || (flags&1u)==0u || texture_color_at(material,uv,normal,point).a>=0.5;
}
fn imported_texture(material:u32,normal:vec3<f32>)->bool {return (texture_records[texture_record(material,normal)+3u]&2u)!=0u;}

// SVO aggregate geometry only carries an appearance-palette ID. The palette
// follows resource-pack/animation changes without rebuilding distant geometry.
fn hit_color(h:Hit,point:vec3<f32>)->vec3<f32> {
    if (h.valid&8u)!=0u {
        let rgb=texture_records[texture_record(h.material,h.normal_depth.xyz)+11u];
        return vec3<f32>(f32((rgb>>16u)&255u),f32((rgb>>8u)&255u),f32(rgb&255u))/255.0;
    }
    return texture_color_at(h.material,h.uv,h.normal_depth.xyz,point).rgb;
}
