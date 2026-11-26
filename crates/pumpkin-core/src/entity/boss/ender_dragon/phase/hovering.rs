use super::EnderDragonPhase;
use crate::entity::boss::ender_dragon::EnderDragonEntity;

pub struct HoveringPhase;

impl super::Phase for HoveringPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::Hovering
    }

    fn begin(&self, dragon: &EnderDragonEntity) {
        dragon.set_target_location(None);
    }

    fn tick(&self, _dragon: &EnderDragonEntity) {}

    fn get_fly_speed(&self) -> f32 {
        1.0
    }
}
