use super::EnderDragonPhase;
use crate::entity::boss::ender_dragon::EnderDragonEntity;

const ROAR_DURATION: i32 = 40;

pub struct SitAttackingPhase;

impl super::Phase for SitAttackingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::SitAttacking
    }

    fn begin(&self, dragon: &EnderDragonEntity) {
        dragon.set_target_location(None);
        *dragon
            .sit_attack_timer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
    }

    fn tick(&self, dragon: &EnderDragonEntity) {
        let elapsed = {
            let mut timer = dragon
                .sit_attack_timer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *timer += 1;
            *timer
        };
        if elapsed > ROAR_DURATION {
            dragon.set_phase(EnderDragonPhase::SitBreathing);
        }
    }
}
