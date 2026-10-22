//! NPC activation and animation, independent of rendering/terrain LODs.
//! Runtime instances live on the existing deferred world/AS worker.
use crate::meshes::{MeshScene, Triangle};
use glam::{IVec3, Mat4, Vec3};
use std::collections::{HashMap, HashSet};
pub const ACTIVE_SIDE: f32=64.0;
pub const REFRESH_SECONDS: f64=1.0;
pub const INACTIVE:u32=1;
const CELL:f32=16.0;
#[derive(Clone,Copy,Debug,PartialEq)]
pub struct ActivePlayer {pub id:u64,pub position:Vec3}
pub fn set_local(players:&mut Vec<ActivePlayer>,position:Vec3) {
    if let Some(p)=players.iter_mut().find(|p| p.id==0) {p.position=position;}
    else {players.push(ActivePlayer{id:0,position});}
}
pub(crate) fn inside(point:Vec3,players:&[ActivePlayer])->bool {
    players.iter().any(|p| (point-p.position).abs().max_element()<=ACTIVE_SIDE*0.5)
}
#[derive(Default)]
pub struct EntitySystem {
    buckets:HashMap<IVec3,Vec<usize>>,
    candidates:Vec<usize>,
    queried_players:Option<Vec<ActivePlayer>>,
    last_query:f64,
    pub queries:u64,
    pub active:usize,
}
impl EntitySystem {
    pub fn from_scene(scene:&MeshScene)->Self {
        let mut system=Self::default();
        for (i,fish) in scene.fish.iter().enumerate() {
            system.buckets.entry((fish.anchor/CELL).floor().as_ivec3()).or_default().push(i);
        }
        system
    }
    pub fn update(&mut self,scene:&mut MeshScene,players:&[ActivePlayer],time:f32,now:f64) {
        let first=self.queried_players.is_none();
        let membership_changed=self.queried_players.as_ref().is_some_and(|old| old.len()!=players.len() || old.iter().zip(players).any(|(a,b)| a.id!=b.id));
        let moved=self.queried_players.as_deref()!=Some(players);
        if first || membership_changed || (moved && now-self.last_query>=REFRESH_SECONDS) {
            let mut candidates=HashSet::new();
            for player in players {
                // Margin encloses the fish's entire .6 m swimming orbit.
                let low=((player.position-Vec3::splat(33.0))/CELL).floor().as_ivec3();
                let high=((player.position+Vec3::splat(33.0))/CELL).floor().as_ivec3();
                for z in low.z..=high.z {for y in low.y..=high.y {for x in low.x..=high.x {
                    if let Some(indices)=self.buckets.get(&IVec3::new(x,y,z)) {candidates.extend(indices.iter().copied());}
                }}}
            }
            self.candidates=candidates.into_iter().collect();self.candidates.sort_unstable();
            self.queried_players=Some(players.to_vec());self.last_query=now;self.queries+=1;
        }
        for fish in &scene.fish {scene.objects[fish.object].info[3]|=INACTIVE;}
        let mut active=HashSet::new();
        for &i in &self.candidates {
            let fish=&scene.fish[i];
            if inside(fish_position(fish.anchor,fish.phase,time),players) {
                active.insert(fish.object);scene.objects[fish.object].info[3]&=!INACTIVE;
            }
        }
        self.active=active.len();animate(scene,time,Some(&active));
    }
}
fn fish_position(anchor:Vec3,offset:f32,time:f32)->Vec3 {
    let phase=time*3.8+offset;let angle=time*0.38+offset;
    anchor+Vec3::new(angle.cos()*0.6,(phase*0.4).sin()*0.12,angle.sin()*0.6)
}
pub(crate) fn animate(scene:&mut MeshScene,time:f32,active:Option<&HashSet<usize>>) {
        for fish in &scene.fish {
            if scene.objects[fish.object].info[3]&INACTIVE!=0 || active.is_some_and(|set| !set.contains(&fish.object)) {continue;}
            let phase = time * 3.8 + fish.phase;
            let bone = |pivot: f32, angle: f32| {
                Mat4::from_translation(Vec3::X * pivot)
                    * Mat4::from_rotation_y(angle)
                    * Mat4::from_translation(-Vec3::X * pivot)
            };
            let bones = [
                Mat4::IDENTITY,
                bone(-0.25, phase.sin() * 0.24),
                bone(-0.65, (phase - 0.8).sin() * 0.65),
            ];
            let object = &mut scene.objects[fish.object];
            let first = object.info[0] as usize;
            for (i, t) in fish.bind.iter().enumerate() {
                let positions = t.map(|v| {
                    bones[v.joints[0]].transform_point3(v.point) * v.weights[0]
                        + bones[v.joints[1]].transform_point3(v.point) * v.weights[1]
                });
                let material = scene.triangles[first + i].material;
                scene.triangles[first + i] = Triangle::new(positions, t.map(|v| v.uv), material);
            }
            let angle = time * 0.38 + fish.phase;
            let position = fish.anchor
                + Vec3::new(
                    angle.cos() * 0.6,
                    (phase * 0.4).sin() * 0.12,
                    angle.sin() * 0.6,
                );
            object.world = (Mat4::from_translation(position)
                * Mat4::from_rotation_y(-angle - std::f32::consts::FRAC_PI_2))
            .to_cols_array_2d();
        }
        let animated:Vec<_>=scene.fish.iter().map(|f| f.object).filter(|i| active.is_none_or(|set| set.contains(i))).collect();
        for i in animated {scene.bounds(i);}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn player(id:u64,x:f32,y:f32,z:f32)->ActivePlayer {ActivePlayer{id,position:Vec3::new(x,y,z)}}
    fn scene()->MeshScene {
        let mut scene=MeshScene::empty();
        for anchor in [Vec3::ZERO,Vec3::new(100.0,0.0,0.0),Vec3::new(0.0,33.0,0.0)] {scene.add_fish(anchor,0.0);}
        scene
    }
    #[test]
    fn stationary_players_do_not_refresh_and_inactive_rigs_do_not_update() {
        let mut scene=scene();let mut system=EntitySystem::from_scene(&scene);
        let players=[player(0,0.0,0.0,0.0)];
        system.update(&mut scene,&players,0.0,0.0);
        let far=scene.objects[1].world;let tris=scene.triangles.clone();
        system.update(&mut scene,&players,5.0,5.0);
        assert_eq!(system.queries,1);assert_eq!(system.active,1);
        assert_eq!(scene.objects[1].world,far);
        let range=scene.objects[1].info;assert_eq!(scene.triangles[range[0] as usize..(range[0]+range[1]) as usize],tris[range[0] as usize..(range[0]+range[1]) as usize]);
    }
    #[test]
    fn moving_queries_are_throttled_but_outside_entities_disable_immediately() {
        let mut scene=scene();let mut system=EntitySystem::from_scene(&scene);
        system.update(&mut scene,&[player(0,0.0,0.0,0.0)],0.0,0.0);
        system.update(&mut scene,&[player(0,100.0,0.0,0.0)],0.2,0.2);
        assert_eq!(system.queries,1);assert_eq!(system.active,0);
        system.update(&mut scene,&[player(0,100.0,0.0,0.0)],1.0,1.0);
        assert_eq!(system.queries,2);assert_eq!(system.active,1);
        assert_ne!(scene.objects[0].info[3]&INACTIVE,0);assert_eq!(scene.objects[1].info[3]&INACTIVE,0);
    }
    #[test]
    fn players_union_is_deduplicated_and_membership_changes_are_immediate() {
        let mut scene=scene();let mut system=EntitySystem::from_scene(&scene);
        let mut players=vec![player(0,0.0,0.0,0.0),player(1,100.0,0.0,0.0),player(2,0.0,0.0,0.0)];
        system.update(&mut scene,&players,0.0,0.0);assert_eq!(system.active,2);
        players.clear();system.update(&mut scene,&players,0.1,0.1);assert_eq!(system.active,0);assert_eq!(system.queries,2);
    }
    #[test]
    fn full_box_includes_corners_and_negative_coordinates() {
        assert!(inside(Vec3::new(-32.0,32.0,-32.0),&[player(0,0.0,0.0,0.0)]));
        assert!(!inside(Vec3::new(0.0,32.01,0.0),&[player(0,0.0,0.0,0.0)]));
        let mut scene=MeshScene::empty();scene.add_fish(Vec3::new(-100.0,-40.0,-100.0),0.0);
        let mut system=EntitySystem::from_scene(&scene);
        system.update(&mut scene,&[player(0,-100.0,-40.0,-100.0)],0.0,0.0);assert_eq!(system.active,1);
    }
}
