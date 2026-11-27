use std::sync::{Arc, atomic::Ordering};

use pumpkin_data::dimension::Dimension;
use pumpkin_protocol::java::client::play::{CGameEvent, GameEvent};

use super::Player;
use crate::{entity::EntityBase, world::portal::PortalType};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EndCreditsState {
    Inactive,
    Watching,
    Returning,
}

impl Player {
    pub(crate) fn has_pending_end_credits(&self) -> bool {
        self.end_credits.load() != EndCreditsState::Inactive
    }

    /// Holds the first Java exit until the client closes the credits screen.
    pub(crate) fn begin_end_credits(&self) -> bool {
        let Some(client) = self.client.java() else {
            return false;
        };
        if self.has_pending_end_credits() {
            return true;
        }
        if self.seen_credits.load(Ordering::Relaxed) {
            return false;
        }
        if self
            .end_credits
            .compare_exchange(EndCreditsState::Inactive, EndCreditsState::Watching)
            .is_err()
        {
            return true;
        }

        self.seen_credits.store(true, Ordering::Relaxed);
        if client.try_send_packet(&CGameEvent::new(GameEvent::WinGame, 0.0)) {
            // The first credits screen is not a completed portal transition. A player
            // who reconnects inside the exit must be able to leave immediately.
            self.get_entity()
                .portal_cooldown
                .store(0, Ordering::Relaxed);
        } else {
            self.seen_credits.store(false, Ordering::Relaxed);
            self.end_credits.store(EndCreditsState::Inactive);
        }
        true
    }

    /// Consumes the same acknowledgement used by the death screen while credits are pending.
    pub(crate) fn finish_end_credits(self: &Arc<Self>) -> bool {
        if let Err(state) = self
            .end_credits
            .compare_exchange(EndCreditsState::Watching, EndCreditsState::Returning)
        {
            return state == EndCreditsState::Returning;
        }

        let player = self.clone();
        let source = self.world();
        let task = self.spawn_task(async move {
            if source.dimension == Dimension::THE_END
                && let Some(server) = source.server.upgrade()
            {
                let default_world = server.get_world_from_dimension(&Dimension::OVERWORLD);
                let transition = PortalType::get_end_exit_destination(default_world, &player).await;
                if Arc::ptr_eq(&player.world(), &source) && !player.client.closed() {
                    if Arc::ptr_eq(&transition.new_world, &source) {
                        player.teleport(
                            transition.position,
                            transition.yaw,
                            transition.pitch,
                            transition.new_world,
                        );
                    } else {
                        player
                            .teleport_world(
                                transition.new_world,
                                transition.position,
                                transition.yaw,
                                transition.pitch,
                            )
                            .await;
                    }
                }
            }
            player.end_credits.store(EndCreditsState::Inactive);
        });
        if task.is_none() {
            self.end_credits.store(EndCreditsState::Inactive);
        }
        true
    }
}
