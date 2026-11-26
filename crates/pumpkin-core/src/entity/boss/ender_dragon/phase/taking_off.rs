use super::EnderDragonPhase;
use crate::entity::boss::ender_dragon::{EnderDragonEntity, NODE_Y, Vector3Ext};
use pumpkin_util::math::vector3::Vector3;

pub struct TakingOffPhase;

impl super::Phase for TakingOffPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::TakingOff
    }

    fn begin(&self, dragon: &EnderDragonEntity) {
        dragon.reset_flight_path();
    }

    fn tick(&self, dragon: &EnderDragonEntity) {
        let entity = &dragon.mob_entity.living_entity.entity;
        let mut podium = dragon.podium_position();
        podium.y += 0.5;
        let pos = entity.pos.load();
        let distance = pos.distance_squared(podium);
        let has_target = dragon
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some();
        if has_target {
            if distance >= 100.0 {
                dragon.set_phase(EnderDragonPhase::Circling);
            }
            return;
        }

        let yaw = f64::from(entity.yaw.load()).to_radians();
        let head_tilt = (45.0 / (distance.sqrt() / 4.0).max(1.0)).to_radians();
        let stride = 40.0 * head_tilt.cos();
        let from = dragon.find_closest_node();
        let toward = Vector3::new(yaw.sin() * stride, f64::from(NODE_Y), -yaw.cos() * stride);
        let toward_index = dragon.find_closest_node_to(toward);
        let to = if dragon.first_flight_node() == 0 {
            toward_index % 12
        } else {
            (toward_index as i32 - 12).rem_euclid(8) as usize + 12
        };
        dragon.plan_flight_path(from, to);
        dragon.follow_next_flight_node(Some(to));
    }
}
