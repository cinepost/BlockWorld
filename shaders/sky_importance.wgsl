@group(0) @binding(0) var<storage,read> ground_voxels:array<u32>;
fn ground_block(x:u32,y:u32,z:u32)->u32 {
    let index=x+p.dims.x*(z+p.dims.z*y);
    return (ground_voxels[index>>2u]>>((index&3u)*8u))&255u;
}
@group(0) @binding(4) var<uniform> p:Params;
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    let row=id.x;if row>=128u {return;}
    if dispatch.x>=3u {
        let bank=p.sky_state.x*SKY_BANK;let scratch=bank+558474u;
        if dispatch.x==3u {
            var sum=vec3<f32>(0.0);var count=0.0;
            for(var k=0u;k<2u;k++) {
                let column=row*2u+k;
                let low=max(vec2<i32>(0),vec2<i32>(floor(p.eye.xz))-p.origin.xz-vec2<i32>(32));
                let high=min(vec2<i32>(p.dims.xz)-1,low+vec2<i32>(63));
                let q=min(high,low+vec2<i32>(i32(column%16u)*4,i32(column/16u)*4));
                if any(q<vec2<i32>(0)) || any(q>=vec2<i32>(p.dims.xz)) {continue;}
                var material=0u;
                for(var y=i32(p.dims.w)-1;y>=p.origin.w;y--) {
                    let block=ground_block(u32(q.x),u32(y),u32(q.y));
                    if block==0u || block==6u || block==13u || block==14u {continue;}
                    material=block;break;
                }
                if material==0u {continue;}
                var reflectance=vec3<f32>(0.0);
                if material==5u {reflectance=vec3<f32>(0.02);}
                else {
                    for(var y=0u;y<16u;y++) {for(var x=0u;x<16u;x++) {
                        reflectance+=texture_color(material,(vec2<f32>(f32(x),f32(y))+0.5)/16.0,vec3<f32>(0.0,1.0,0.0)).rgb/256.0;
                    }}
                }
                sum+=clamp(reflectance,vec3<f32>(0.0),vec3<f32>(0.9));count+=1.0;
            }
            sky_cache[scratch+row]=vec4<f32>(sum,count);return;
        }
        if row!=0u {return;}
        var sum=vec3<f32>(0.0);var count=0.0;
        for(var r=0u;r<128u;r++) {let value=sky_cache[scratch+r];sum+=value.xyz;count+=value.w;}
        let albedo=sum/max(count,1.0);
        let base=bank+558464u;
        let sky=max(vec3<f32>(0.0),3.141592654*(sky_cache[base].xyz*0.282094792+sky_cache[base+2u].xyz*(0.488602512*2.0/3.0)+sky_cache[base+6u].xyz*(0.630783130*0.25)));
        // Secondary energy is reflected once, never subtracted from the primary
        // sky/direct split again. Global average assumes unshadowed terrain.
        let ground=(sky+sky_cache[base+9u].xyz*max(p.sun.y,0.0))*albedo/3.141592654;
        sky_cache[bank+558602u]=vec4<f32>(albedo,count);
        sky_cache[bank+558603u]=vec4<f32>(ground,1.0);return;
    }
    let bank=p.sky_state.y*SKY_BANK;let importance=bank+524288u;let rows=importance+32768u;
    let partial=bank+557184u+row*10u;
    if dispatch.x==2u {
        if row>=10u {return;}
        var total=vec3<f32>(0.0);
        for(var y=0u;y<128u;y++) {total+=sky_cache[bank+557184u+y*10u+row].xyz;}
        var horizontal=vec3<f32>(0.0);
        for(var y=0u;y<128u;y++) {
            let b=bank+557184u+y*10u;
            horizontal+=3.141592654*(sky_cache[b].xyz*0.282094792+sky_cache[b+2u].xyz*(0.488602512*2.0/3.0)+sky_cache[b+6u].xyz*(0.630783130*0.25));
        }
        horizontal=max(horizontal,vec3<f32>(0.0));
        let budget=select(vec3<f32>(22.0),vec3<f32>(0.035),p.sky_build.y< -0.02);
        let height=select(max(p.sky_build.y,0.0),-p.sky_build.y,p.sky_build.y< -0.02);
        // Keep residual twilight when the primary disk is at/below the horizon.
        let available=budget*max(height,0.02);
        let scale=min(vec3<f32>(1.0),available/max(horizontal,vec3<f32>(1e-9)));
        if row<9u {total*=scale;}
        if row==9u {
            // Horizontal incident energy budget. Atmosphere redistributes part
            // into diffuse sky; only the remainder belongs to the direct disk.
            total=max(vec3<f32>(0.0),budget*height-horizontal*scale)/max(height,0.02);
        }
        sky_cache[bank+557184u+1280u+row]=vec4<f32>(total,1.0);
        return;
    }
    if dispatch.x==0u {
        let solid_angle=(6.283185307/256.0)*(cos(f32(row)*3.141592654/128.0)-cos(f32(row+1u)*3.141592654/128.0));
        var sum=0.0;var coefficients:array<vec3<f32>,10>;
        for(var x=0u;x<256u;x++) {
            var color=vec3<f32>(0.0);
            for(var dy=0u;dy<4u;dy++) {for(var dx=0u;dx<4u;dx++) {color+=sky_cache[bank+x*4u+dx+(row*4u+dy)*1024u].xyz;}}
            let radiance=color/16.0;
            let phi=((f32(x)+0.5)/256.0-0.5)*6.283185307;
            let theta=(f32(row)+0.5)/128.0*3.141592654;
            let direction=vec3<f32>(cos(phi)*sin(theta),cos(theta),sin(phi)*sin(theta));
            // Terrain blocks the lower hemisphere; do not integrate underground sky.
            if direction.y>0.0 {
                for(var k=0u;k<9u;k++) {coefficients[k]+=radiance*(solid_angle*sky_basis(direction,k));}
                coefficients[9]+=radiance*(solid_angle*direction.y);
            }
            let luminance=max(1e-8,dot(color/16.0,vec3<f32>(0.2126,0.7152,0.0722)));
            let mass=luminance*solid_angle;sum+=mass;
            sky_cache[importance+x+row*256u]=vec4<f32>(luminance,mass,sum,solid_angle);
        }
        for(var k=0u;k<10u;k++) {sky_cache[partial+k]=vec4<f32>(coefficients[k],0.0);}
        sky_cache[rows+row]=vec4<f32>(sum,0.0,0.0,0.0);
    } else {
        var total=0.0;var prefix=0.0;
        for(var y=0u;y<128u;y++) {let mass=sky_cache[rows+y].x;total+=mass;if y<=row {prefix+=mass;}}
        let mass=sky_cache[rows+row].x;
        for(var x=0u;x<256u;x++) {
            let value=sky_cache[importance+x+row*256u];
            sky_cache[importance+x+row*256u]=vec4<f32>(value.x,value.x/max(total,1e-12),value.z/max(mass,1e-12),value.w);
        }
        // Only this row's x is read by other invocations, never overwritten.
        sky_cache[rows+row].y=prefix/max(total,1e-12);sky_cache[rows+row].z=total;
    }
}
