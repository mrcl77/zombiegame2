// Stub zone module — purchasable mall zones are gone. The wave-based Metro map
// uses a single segment for now (Platform); future segments will be unlocked
// via doors instead of barriers. We keep `ZoneState` and `ZonesPlugin` so that
// existing imports (`achievements`, `zombie`) continue to compile.

use bevy::prelude::*;

#[derive(Resource)]
pub struct ZoneState {
    pub unlocked: [bool; 4],
}

impl Default for ZoneState {
    fn default() -> Self {
        Self {
            unlocked: [true, true, true, true],
        }
    }
}

pub struct ZonesPlugin;

impl Plugin for ZonesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneState>();
    }
}
