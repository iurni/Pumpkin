use super::EnderDragonPhase;
use crate::entity::boss::ender_dragon::{EnderDragonEntity, Vector3Ext};

pub struct LandingPhase;

impl super::Phase for LandingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::Landing
    }

    fn begin(&self, dragon: &EnderDragonEntity) {
        dragon.set_target_location(None);
    }

    fn tick(&self, dragon: &EnderDragonEntity) {
        let target = *dragon
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let target = target.unwrap_or_else(|| dragon.podium_position());
        let pos = dragon.mob_entity.living_entity.entity.pos.load();
        if pos.distance_squared(target) < 1.0 {
            *dragon
                .flame_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
            dragon.set_phase(EnderDragonPhase::SitScanning);
        } else {
            dragon.set_target_location(Some(target));
        }
    }

    fn get_fly_speed(&self) -> f32 {
        1.5
    }

    fn get_turn_speed(&self, horizontal_speed: f32) -> f32 {
        let speed = horizontal_speed + 1.0;
        speed.min(40.0) / speed
    }
}
