use super::EnderDragonPhase;
use crate::entity::EntityBase;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;

pub struct LandingApproachPhase;

impl super::Phase for LandingApproachPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::LandingApproach
    }

    fn begin(&self, dragon: &EnderDragonEntity) {
        dragon.reset_flight_path();
    }

    fn tick(&self, dragon: &EnderDragonEntity) {
        if !dragon.flight_target_reached() {
            return;
        }
        let has_approached = dragon
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some();
        if !has_approached {
            let podium = dragon.podium_position();
            let approach = dragon
                .nearest_combat_player(podium, 192.0, f64::INFINITY, false)
                .map_or(Vector3::new(40.0, podium.y, 0.0), |player| {
                    let pos = player.get_entity().pos.load();
                    let away = Vector3::new(-pos.x, 0.0, -pos.z).normalize();
                    Vector3::new(40.0 * away.x, 105.0, 40.0 * away.z)
                });
            let from = dragon.find_closest_node();
            let to = dragon.find_closest_node_to(approach);
            dragon.plan_flight_path(from, to);
        }
        // Advancing past the final path waypoint in Java switches to the precise podium
        // target immediately. The real landing pass flies to its bottom-center position.
        if !dragon.follow_next_flight_node(None) {
            dragon.set_phase(EnderDragonPhase::Landing);
        }
    }
}
