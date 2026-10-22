//! Stateless world-space fields, shared by streaming, terrain LODs and inspection.
use crate::{config::*, world::Block};
#[repr(u8)]
#[derive(Clone,Copy,Debug,PartialEq,Eq,Hash)]
pub enum Biome {Ocean,DeepOcean,Meadow,JaggedPeaks,Forest,Plains,Desert}
impl Biome {
    pub const ALL:[Self;7]=[Self::Ocean,Self::DeepOcean,Self::Meadow,Self::JaggedPeaks,Self::Forest,Self::Plains,Self::Desert];
    pub fn name(self)->&'static str {match self {Self::Ocean=>"ocean",Self::DeepOcean=>"deep ocean",Self::Meadow=>"meadow",Self::JaggedPeaks=>"jagged peaks",Self::Forest=>"forest",Self::Plains=>"plains",Self::Desert=>"desert"}}
}
#[derive(Clone,Copy)]
pub struct TerrainSettings {pub base_height:f32,pub erosion:f32,pub roughness:f32}
pub const SETTINGS:TerrainSettings=TerrainSettings{base_height:0.0,erosion:1.0,roughness:1.0};
#[derive(Clone,Copy,Debug)]
pub struct TerrainSample {pub height:i32,pub biome:Biome,pub temperature:f32,pub humidity:f32,pub erosion:f32,pub roughness:f32}
pub(crate) fn hash(x:i32,z:i32,seed:u32)->u32 {
    let mut n=(x as u32).wrapping_mul(374761393)^(z as u32).wrapping_mul(668265263)^seed;
    n=(n^(n>>13)).wrapping_mul(1274126177);n^(n>>16)
}
pub(crate) fn noise(x:f32,z:f32,scale:f32,seed:u32)->f32 {
    let q=glam::Vec2::new(x,z)/scale;let cell=q.floor().as_ivec2();let f=q-q.floor();
    let f=f*f*(glam::Vec2::splat(3.0)-f*2.0);
    let h=|x,z|(hash(x,z,seed)&65535) as f32/65535.0;
    let a=h(cell.x,cell.y)*(1.0-f.x)+h(cell.x+1,cell.y)*f.x;
    let b=h(cell.x,cell.y+1)*(1.0-f.x)+h(cell.x+1,cell.y+1)*f.x;
    a*(1.0-f.y)+b*f.y
}
fn smooth(a:f32,b:f32,v:f32)->f32 {let t=((v-a)/(b-a)).clamp(0.0,1.0);t*t*(3.0-2.0*t)}
pub fn sample(x:i32,z:i32,seed:u32)->TerrainSample {
    let (x,z)=(x as f32,z as f32);
    let temperature=0.8*noise(x,z,1024.0,seed^0x74656d70)+0.2*noise(x,z,256.0,seed^0xabc1);
    let humidity=0.8*noise(x,z,896.0,seed^0x68756d69)+0.2*noise(x,z,256.0,seed^0xabc2);
    let continental=0.8*noise(x,z,1536.0,seed^0x636f6e74)+0.2*noise(x,z,512.0,seed^73);
    let erosion=noise(x,z,512.0,seed^0x65726f73);
    let erosion_effect=(erosion*SETTINGS.erosion).clamp(0.0,1.0);
    let roughness=noise(x,z,192.0,seed^0x726f7567);
    let base= -42.0+108.0*smooth(0.24,0.76,continental)+SETTINGS.base_height;
    let hills=(noise(x,z,192.0,seed)*2.0-1.0)*17.0*(1.0-0.65*erosion_effect);
    let eroded=(1.0-(noise(x,z,384.0,seed^91)*2.0-1.0).abs()).powi(6)*18.0*erosion_effect;
    let massif=smooth(0.46,0.78,noise(x,z,768.0,seed^1001))*(1.0-smooth(0.38,0.78,erosion_effect))*smooth(0.39,0.62,continental)*(1.0-smooth(0.48,0.64,temperature));
    let ridge=1.0-(noise(x,z,128.0,seed^719)*2.0-1.0).abs();
    let detail=(noise(x,z,32.0,seed^17)*2.0-1.0)*5.0*roughness*SETTINGS.roughness;
    let height=(base+hills-eroded+massif*ridge*185.0+detail).floor().clamp((MIN_Y+8) as f32,(MAX_Y-20) as f32) as i32;
    let biome=classify(temperature,humidity,height);
    TerrainSample{height,biome,temperature,humidity,erosion,roughness}
}
fn classify(temperature:f32,humidity:f32,height:i32)->Biome {
    if height<SEA_LEVEL-24 {Biome::DeepOcean}
    else if height<SEA_LEVEL {Biome::Ocean}
    else if temperature>0.66 && humidity<0.4 && height<90 {Biome::Desert}
    else if temperature<0.58 && height>85 {Biome::JaggedPeaks}
    else if humidity>0.64 {Biome::Forest}
    else if humidity>0.43 && temperature>0.28 && temperature<0.68 && height>24 {Biome::Meadow}
    else {Biome::Plains}
}
pub fn terrain_height(x:i32,z:i32,seed:u32)->i32 {sample(x,z,seed).height}
pub(crate) fn entrance(x:i32,z:i32,seed:u32)->bool {noise(x as f32,z as f32,64.0,seed^0xca7e)>0.79}
pub(crate) fn layer(t:TerrainSample,y:i32)->Block {
    if y>t.height {return Block::Water;}
    let depth=t.height-y;
    if depth>8 {return Block::Stone;}
    match t.biome {
        Biome::Ocean|Biome::DeepOcean=>if depth<4 {if t.height<SEA_LEVEL-8 {Block::Gravel} else {Block::Sand}} else {Block::Stone},
        Biome::Desert=>if depth<3 {Block::Sand} else {Block::Sandstone},
        Biome::JaggedPeaks=>if depth==0 && t.height>=SNOW_LEVEL {Block::Snow} else {Block::Granite},
        _=>if depth>3 {Block::Stone} else if t.height<=SEA_LEVEL+1 {Block::Sand} else if depth>0 {Block::Dirt} else {match t.biome {Biome::Forest=>Block::Podzol,Biome::Meadow=>Block::MeadowGrass,_=>Block::Grass}},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn climate_has_all_biomes_and_smooth_transitions() {
        let mut found=std::collections::HashSet::new();
        for z in (-8192..8192).step_by(64) {for x in (-8192..8192).step_by(64) {
            let a=sample(x,z,42);let b=sample(x+1,z,42);
            found.insert(a.biome);
            assert!((a.temperature-b.temperature).abs()<0.01 && (a.humidity-b.humidity).abs()<0.01);
            assert!((a.height-b.height).abs()<=5);
            assert!(!(a.biome==Biome::Desert && b.biome==Biome::JaggedPeaks));
            assert!((0.0..=1.0).contains(&a.erosion) && (0.0..=1.0).contains(&a.roughness));
        }}
        for b in Biome::ALL {assert!(found.contains(&b),"Missing {}",b.name());}
    }
    #[test]
    fn biome_layers_use_specific_materials() {
        for (biome,expected) in [(Biome::Ocean,Block::Gravel),(Biome::DeepOcean,Block::Gravel),(Biome::Meadow,Block::MeadowGrass),(Biome::Forest,Block::Podzol),(Biome::JaggedPeaks,Block::Granite),(Biome::Desert,Block::Sand)] {
            let t=TerrainSample{height:if matches!(biome,Biome::Ocean|Biome::DeepOcean){-24}else{80},biome,temperature:0.5,humidity:0.5,erosion:0.5,roughness:0.5};
            assert_eq!(layer(t,t.height),expected);
        }
    }
}
