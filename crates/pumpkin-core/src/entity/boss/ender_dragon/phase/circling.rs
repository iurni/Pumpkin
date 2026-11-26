use super::EnderDragonPhase;
use crate::entity::EntityBase;
use crate::entity::boss::ender_dragon::{EnderDragonEntity, Vector3Ext};

pub struct CirclingPhase;

impl super::Phase for CirclingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::Circling
    }

    fn begin(&self, dragon: &EnderDragonEntity) {
        dragon.reset_flight_path();
    }

    fn tick(&self, dragon: &EnderDragonEntity) {
        if !dragon.flight_target_reached() {
            return;
        }

        if dragon.follow_next_flight_node(None) {
            return;
        }

        let alive_crystals = dragon.alive_crystals();
        let crystals = alive_crystals.unwrap_or(0);
        let has_flown = dragon
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some();
        if has_flown {
            if rand::random_range(0..crystals + 3) == 0 {
                dragon.set_phase(EnderDragonPhase::LandingApproach);
                return;
            }
            let podium = dragon.podium_position();
            if let Some(player) = dragon.nearest_combat_player(podium, 192.0, f64::INFINITY, false)
            {
                let distance = podium.distance_squared(player.get_entity().pos.load());
                if rand::random_range(0..(distance / 512.0 + 2.0) as i32) == 0
                    || rand::random_range(0..crystals + 2) == 0
                {
                    *dragon
                        .target_player
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(player.gameprofile.id);
                    dragon.set_phase(EnderDragonPhase::Strafing);
                    return;
                }
            }
        }

        let from = dragon.find_closest_node();
        let mut next = from as i32;
        let mut clockwise = dragon
            .holding_pattern_clockwise
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if rand::random_range(0..8) == 0 {
            *clockwise = !*clockwise;
            next += 6;
        }
        next += if *clockwise { 1 } else { -1 };
        drop(clockwise);
        // Holding pattern selects the outer ring while linked to a fight. With no crystals
        // the path search still excludes those outer nodes and may return a partial route.
        let to = if alive_crystals.is_some() {
            next.rem_euclid(12) as usize
        } else {
            ((next - 12).rem_euclid(8) + 12) as usize
        };
        dragon.plan_flight_path(from, to);
        dragon.follow_next_flight_node(Some(from));
    }
}
