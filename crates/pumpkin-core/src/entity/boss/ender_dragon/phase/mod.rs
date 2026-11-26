use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Arc;

mod charging;
mod circling;
mod dying;
mod hovering;
mod landing;
mod landing_approach;
mod sit_attacking;
mod sit_breathing;
mod sit_scanning;
mod strafing;
mod taking_off;

pub use charging::ChargingPhase;
pub use circling::CirclingPhase;
pub use dying::DyingPhase;
pub use hovering::HoveringPhase;
pub use landing::LandingPhase;
pub use landing_approach::LandingApproachPhase;
pub use sit_attacking::SitAttackingPhase;
pub use sit_breathing::SitBreathingPhase;
pub use sit_scanning::SitScanningPhase;
pub use strafing::StrafingPhase;
pub use taking_off::TakingOffPhase;

pub trait Phase: Send + Sync {
    fn get_type(&self) -> EnderDragonPhase;
    fn tick(&self, dragon: &EnderDragonEntity);
    fn begin(&self, _dragon: &EnderDragonEntity) {}
    fn end(&self, _dragon: &EnderDragonEntity) {}
    fn is_sitting(&self) -> bool {
        false
    }
    fn get_fly_speed(&self) -> f32 {
        0.6
    }
    fn get_turn_speed(&self, horizontal_speed: f32) -> f32 {
        let speed = horizontal_speed + 1.0;
        0.7 / speed.min(40.0) / speed
    }
    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EnderDragonPhase {
    #[default]
    Circling = 0,
    Strafing = 1,
    LandingApproach = 2,
    Landing = 3,
    TakingOff = 4,
    SitBreathing = 5,
    SitScanning = 6,
    SitAttacking = 7,
    Charging = 8,
    Dying = 9,
    Hovering = 10,
}

impl EnderDragonPhase {
    #[must_use]
    pub const fn is_sitting(self) -> bool {
        matches!(
            self,
            Self::SitScanning | Self::SitAttacking | Self::SitBreathing | Self::Hovering
        )
    }

    #[must_use]
    pub const fn network_id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn from_network_id(id: i32) -> Self {
        match id {
            1 => Self::Strafing,
            2 => Self::LandingApproach,
            3 => Self::Landing,
            4 => Self::TakingOff,
            5 => Self::SitBreathing,
            6 => Self::SitScanning,
            7 => Self::SitAttacking,
            8 => Self::Charging,
            9 => Self::Dying,
            10 => Self::Hovering,
            _ => Self::Circling,
        }
    }
}

pub struct PhaseManager {
    phases: [Arc<dyn Phase>; 11],
}

impl Default for PhaseManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PhaseManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            phases: [
                Arc::new(CirclingPhase),
                Arc::new(StrafingPhase),
                Arc::new(LandingApproachPhase),
                Arc::new(LandingPhase),
                Arc::new(TakingOffPhase),
                Arc::new(SitBreathingPhase),
                Arc::new(SitScanningPhase),
                Arc::new(SitAttackingPhase),
                Arc::new(ChargingPhase::default()),
                Arc::new(DyingPhase),
                Arc::new(HoveringPhase),
            ],
        }
    }

    #[must_use]
    pub fn get_phase(&self, phase_type: EnderDragonPhase) -> Arc<dyn Phase> {
        self.phases[phase_type as usize].clone()
    }
}
