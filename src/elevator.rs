// Stub elevator module — the multi-floor mall is gone. The Metro Platform map
// uses doors between segments (to be implemented). We keep an empty plugin so
// `main.rs` registration stays unchanged.

use bevy::prelude::*;

pub struct ElevatorPlugin;

impl Plugin for ElevatorPlugin {
    fn build(&self, _app: &mut App) {}
}
