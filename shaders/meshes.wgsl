struct Triangle {a:vec4<f32>,b:vec4<f32>,c:vec4<f32>,uv01:vec4<f32>,uv2:vec2<f32>,material:u32,flags:u32};
struct Object {world:mat4x4<f32>,low:vec4<f32>,high:vec4<f32>,info:vec4<u32>};
struct MeshHit {normal_depth:vec4<f32>,uv:vec2<f32>,material:u32,flags:u32,ids:vec4<u32>};
struct Material {flags:u32,alpha_cutoff:f32,ao_weight:f32,pad:u32};
@group(0) @binding(8) var<storage,read> objects:array<Object>;
@group(0) @binding(9) var<storage,read> triangles:array<Triangle>;
@group(0) @binding(13) var<storage,read> materials:array<Material>;
fn covered(material:u32,uv:vec2<f32>)->bool {
    if p.meshes.w==0u || (materials[material-1u].flags&1u)==0u {return true;}
    return texture_color(material,uv,vec3<f32>(1.0,0.0,0.0)).a>=materials[material-1u].alpha_cutoff;
}
fn mesh_miss()->MeshHit {return MeshHit(vec4<f32>(0.0,0.0,0.0,p.up.w),vec2<f32>(0.0),0u,0u,vec4<u32>(0u));}
fn box_test(origin:vec3<f32>,ray:vec3<f32>,low:vec3<f32>,high:vec3<f32>,limit:f32)->bool {
    var near=0.0;var far=limit;
    for(var a=0u;a<3u;a++) {
        if abs(ray[a])<0.000001 {if origin[a]<low[a] || origin[a]>high[a] {return false;}}
        else {let a1=(low[a]-origin[a])/ray[a];let a2=(high[a]-origin[a])/ray[a];near=max(near,min(a1,a2));far=min(far,max(a1,a2));}
    }return far>=near;
}
fn mesh_trace(origin:vec3<f32>,ray:vec3<f32>,limit:f32)->MeshHit {
    var hit=mesh_miss();hit.normal_depth.w=limit;
    if !box_test(origin,ray,p.scene_min.xyz,p.scene_max.xyz,limit) {return hit;}
    for(var oi=0u;oi<p.meshes.x;oi++) {
        let object=objects[oi];
        if (object.info.w&1u)!=0u || (object.info.z==0u && !entity_in_range(object.world[3].xyz)) {continue;}
        if (object.info.z==0u && p.meshes.y==0u) || (object.info.z!=0u && p.meshes.z==0u) {continue;}
        if !box_test(origin,ray,object.low.xyz-0.0001,object.high.xyz+0.0001,hit.normal_depth.w) {continue;}
        for(var i=0u;i<object.info.y;i++) {
            let tri=triangles[object.info.x+i];
            let a=(object.world*vec4<f32>(tri.a.xyz,1.0)).xyz;
            let b=(object.world*vec4<f32>(tri.b.xyz,1.0)).xyz;
            let c=(object.world*vec4<f32>(tri.c.xyz,1.0)).xyz;
            let e1=b-a;let e2=c-a;let q=cross(ray,e2);let det=dot(e1,q);
            if abs(det)<0.0000001 {continue;}
            let inv=1.0/det;let s=origin-a;let u=dot(s,q)*inv;
            if u<0.0 || u>1.0 {continue;}
            let v=dot(ray,cross(s,e1))*inv;
            if v<0.0 || u+v>1.0 {continue;}
            let t=dot(e2,cross(s,e1))*inv;
            if t<=0.001 || t>=hit.normal_depth.w {continue;}
            let uv=tri.uv01.xy*(1.0-u-v)+tri.uv01.zw*u+tri.uv2*v;
            if !covered(tri.material,uv) {continue;}
            var normal=normalize(cross(e1,e2));if dot(normal,ray)>0.0 {normal=-normal;}
            let flags=1u|4u|((materials[tri.material-1u].flags&1u)<<1u)|((oi+1u)<<8u);
            hit=MeshHit(vec4<f32>(normal,t),fract(uv),tri.material,flags,vec4<u32>(oi,i,0u,p.screen.w>>1u));
        }
    }return hit;
}
