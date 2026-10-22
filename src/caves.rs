//! World-aligned 4m density lattice: shared borders, no neighbor load dependency.
use crate::{config::*,terrain::{hash,noise}};
const STEP:usize=4;
const SIDE:usize=EDGE/STEP+1;
fn noise3(x:f32,y:f32,z:f32,scale:f32,seed:u32)->f32 {
    let p=glam::Vec3::new(x,y,z)/scale;let cell=p.floor().as_ivec3();let f=p-p.floor();
    let f=f*f*(glam::Vec3::splat(3.0)-2.0*f);
    let mut result=0.0;
    for dy in 0..2 {for dz in 0..2 {for dx in 0..2 {
        let h=hash(cell.x+dx,cell.z+dz,seed^(cell.y+dy).cast_unsigned().wrapping_mul(0x9e3779b9));
        let w=|v:f32,d|if d==0 {1.0-v}else{v};
        result+=(h&65535) as f32/65535.0*w(f.x,dx)*w(f.y,dy)*w(f.z,dz);
    }}}
    result
}
fn density(x:i32,y:i32,z:i32,seed:u32)->f32 {
    let (x,y,z)=(x as f32,y as f32,z as f32);
    let a=noise3(x,y,z,42.0,seed^0xcab1)-0.5;
    let b=noise3(x,y,z,42.0,seed^0xcab2)-0.5;
    let tunnel=0.075-(a*a+b*b).sqrt();
    let chamber=(noise3(x,y,z,31.0,seed^0xcab3)-0.73)*0.7;
    tunnel.max(chamber)
}
pub(crate) struct CaveField {data:Vec<f32>,height:usize}
impl CaveField {
    pub fn new(cx:i32,cz:i32,max_y:i32,seed:u32)->Self {
        let height=((max_y-MIN_Y).max(0) as usize).div_ceil(STEP)+2;
        let mut data=vec![0.0;SIDE*SIDE*height];
        for y in 0..height {for z in 0..SIDE {for x in 0..SIDE {
            data[x+SIDE*(z+SIDE*y)]=density(cx*16+(x*STEP) as i32,MIN_Y+(y*STEP) as i32,cz*16+(z*STEP) as i32,seed);
        }}}
        Self{data,height}
    }
    pub fn density(&self,x:usize,y:usize,z:usize)->f32 {
        let q=[x as f32/STEP as f32,y as f32/STEP as f32,z as f32/STEP as f32];
        let base=[(q[0] as usize).min(SIDE-2),(q[1] as usize).min(self.height-2),(q[2] as usize).min(SIDE-2)];
        let f=std::array::from_fn::<_,3,_>(|i|q[i]-base[i] as f32);
        let mut result=0.0;
        for dy in 0..2 {for dz in 0..2 {for dx in 0..2 {
            let w=|v:f32,d|if d==0 {1.0-v}else{v};
            result+=self.data[base[0]+dx+SIDE*(base[2]+dz+SIDE*(base[1]+dy))]*w(f[0],dx)*w(f[1],dy)*w(f[2],dz);
        }}}
        result
    }
}
pub(crate) fn aquifer(x:i32,z:i32,seed:u32)->bool {noise(x as f32,z as f32,192.0,seed^0xa91f)>0.65}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caves_cross_signed_chunk_borders_and_contain_open_networks() {
        let a=CaveField::new(-1,-1,100,42);let b=CaveField::new(0,-1,100,42);
        let c=CaveField::new(-1,0,100,42);let mut open=0;let mut connections=0;
        for y in 4..150 {for q in 0..16 {
            assert!((a.density(16,y,q)-b.density(0,y,q)).abs()<1e-7);
            assert!((a.density(q,y,16)-c.density(q,y,0)).abs()<1e-7);
            if a.density(8,y,q)>0.0 {open+=1;}
            if a.density(15,y,q)>0.0 && b.density(0,y,q)>0.0 {connections+=1;}
        }}
        assert!(open>10,"No open cave cells in fixture");
        assert!(connections>0,"No traversable cave connection across the chunk border");
    }
}
