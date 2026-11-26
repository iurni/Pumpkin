use super::EnderDragonPhase;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use std::sync::atomic::{AtomicI32, Ordering};

const CHARGE_RECOVERY_TIME: i32 = 10;

#[derive(Default)]
pub struct ChargingPhase {
    time_since_charge: AtomicI32,
}

impl super::Phase for ChargingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::Charging
    }

    fn begin(&self, dragon: &EnderDragonEntity) {
        dragon.set_target_location(None);
        *dragon
            .charging_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.time_since_charge.store(0, Ordering::Relaxed);
    }

    fn tick(&self, dragon: &EnderDragonEntity) {
        let target = *dragon
            .charging_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(target) = target else {
            dragon.set_phase(EnderDragonPhase::Circling);
            return;
        };
        if self.time_since_charge.load(Ordering::Relaxed) > 0
            && self.time_since_charge.fetch_add(1, Ordering::Relaxed) >= CHARGE_RECOVERY_TIME
        {
            dragon.set_phase(EnderDragonPhase::Circling);
        } else {
            dragon.set_target_location(Some(target));
            if dragon.flight_target_reached() {
                self.time_since_charge.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn get_fly_speed(&self) -> f32 {
        3.0
    }
}
