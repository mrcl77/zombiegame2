use std::collections::VecDeque;

use bevy::prelude::*;

use crate::map::{building_floor_count, segment_name, PlayerFloorState, SegmentUnlockHint};
use crate::map_data::BUILDINGS;
use crate::net::{NetContext, NetMode, PlayerNicknames};
use crate::player::{
    player_palette_color, Player, PlayerDamagedEvent, PLAYER_ARMOR_MAX, PLAYER_MAX_HP,
};
use crate::settings::GraphicsSettings;
use crate::wave::WaveState;
use crate::zombie::{SpawnZombieEvent, ZombieKilledEvent, ZombieKind};
use crate::weapon::{PickupPromptHint, ThrowableAssets, ThrowableKind, WeaponAssets};
use crate::zombie::Zombie;
use crate::pixelart::Canvas;
use crate::{GameState, Score, UiAssets};

#[derive(Component)]
pub struct HudRoot;

#[derive(Component)]
pub struct HpBarFill;

#[derive(Component)]
pub struct HpText;

#[derive(Component)]
pub struct ArmorBarFill;

#[derive(Component)]
pub struct ArmorText;

#[derive(Component)]
pub struct FloorIndicatorRoot;

#[derive(Component)]
pub struct FloorIndicatorText;

#[derive(Component)]
pub struct PickupPromptRoot;

#[derive(Component)]
pub struct PickupPromptText;

#[derive(Component)]
pub struct WaveIntroRoot;

#[derive(Component)]
pub struct WaveIntroText;

/// Tracks the last wave we showed an intro for so we re-fire on each
/// new wave start (and not every frame the wave is active).
#[derive(Resource, Default)]
pub struct WaveIntroState {
    pub last_wave: u32,
    pub flash_remaining: f32,
}

const WAVE_INTRO_DURATION: f32 = 2.4;

#[derive(Component)]
#[allow(dead_code)]
pub struct ScreenVignette;

#[derive(Component)]
#[allow(dead_code)]
pub struct FilmGrain;

#[derive(Component)]
pub struct ComboCounterRoot;

#[derive(Component)]
pub struct ComboCounterText;

#[derive(Component)]
pub struct BossAlertRoot;

#[derive(Component)]
pub struct BossAlertText;

/// Tracks the boss-alert flash so it can be triggered from the spawn
/// listener and animated by `update_boss_alert`.  Holds the remaining
/// flash time in seconds.
#[derive(Resource, Default)]
pub struct BossAlertTimer {
    pub remaining: f32,
}

const BOSS_ALERT_DURATION: f32 = 2.8;

/// Tracks the running kill chain for the combo HUD.  Resets after
/// `COMBO_TIMEOUT` seconds without a fresh kill.
#[derive(Resource, Default)]
pub struct ComboState {
    pub count: u32,
    pub time_since_kill: f32,
    pub punch_time: f32, // counts up since last kill, used for the bump anim
}

const COMBO_TIMEOUT: f32 = 2.6;

/// 4-frame procedural noise textures cycled to animate grain.  Built once
/// at startup; the cycler swaps the active frame every ~80 ms.  Currently
/// unused (overlay nodes disabled because they fogged the screen) but
/// kept around so a milder revival is a one-line edit.
#[derive(Resource)]
#[allow(dead_code)]
pub struct PostprocessAssets {
    pub vignette: Handle<Image>,
    pub grain_frames: [Handle<Image>; 4],
}

/// Full-screen overlay used for the red hit-flash and the low-HP pulsing
/// vignette.  Both share the same node — alpha gets composed each frame
/// from `HitFlashTimer` (transient) and the static low-HP factor.
#[derive(Component)]
pub struct DamageOverlay;

/// Decaying flash timer in seconds that drives the red hit-overlay alpha
/// after a `PlayerDamagedEvent`.  Set in `update_damage_overlay`.
#[derive(Resource, Default)]
pub struct HitFlashTimer {
    pub remaining: f32,
}

const HIT_FLASH_DURATION: f32 = 0.3;

#[derive(Component)]
pub struct WaveTitleText;

#[derive(Component)]
pub struct WaveStatusText;

#[derive(Component)]
pub struct ScoreValueText;

#[derive(Component)]
pub struct WeaponText;

#[derive(Component)]
pub struct AmmoText;

#[derive(Component)]
pub struct FpsCounterRoot;

#[derive(Component)]
pub struct FpsCounterText;

#[derive(Component)]
pub struct GameOverUi;

#[derive(Component)]
pub struct SegmentPromptRoot;

#[derive(Component)]
pub struct SegmentPromptText;

#[derive(Component)]
pub struct PlayerListRoot;

#[derive(Component)]
pub struct PlayerListSlot {
    pub slot: u8,
}

#[derive(Component)]
pub struct PlayerListNickText {
    pub slot: u8,
}

#[derive(Component)]
pub struct PlayerListHpFill {
    pub slot: u8,
}

#[derive(Component)]
pub struct SlotIcon(pub u8);

#[derive(Component)]
pub struct SlotBorder(pub u8);

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HitFlashTimer>()
            .init_resource::<WaveIntroState>()
            .init_resource::<ComboState>()
            .init_resource::<BossAlertTimer>()
            .add_systems(Startup, setup_postprocess_assets)
            .add_systems(OnEnter(GameState::Playing), (spawn_hud, reset_combo))
            .add_systems(
                OnExit(GameState::Playing),
                (despawn_hud, reset_wave_intro, reset_combo),
            )
            .add_systems(
                Update,
                (
                    update_hud,
                    update_slot_icons,
                    update_floor_indicator,
                    update_pickup_prompt,
                    update_wave_intro,
                    update_combo,
                    update_boss_alert,
                    update_damage_overlay,
                    update_fps_counter,
                    update_segment_prompt,
                    update_player_list,
                )
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(OnEnter(GameState::GameOver), spawn_game_over)
            .add_systems(OnExit(GameState::GameOver), despawn_game_over)
            .add_systems(
                Update,
                game_over_input.run_if(in_state(GameState::GameOver)),
            );
    }
}

fn spawn_hud(
    mut commands: Commands,
    assets: Res<UiAssets>,
    weapon_assets: Res<WeaponAssets>,
    throwable_assets: Res<ThrowableAssets>,
    postprocess: Res<PostprocessAssets>,
) {
    let font = assets.font.clone();

    // ── Postprocess overlays disabled — vignette + film grain made the
    // screen unreadable.  Keep `PostprocessAssets` allocated in case we
    // want to re-enable a milder version later, but don't spawn the
    // ImageBundle nodes.  Bloom still runs from the camera config.
    let _ = &postprocess;

    // ── Bottom-left: Slots + Weapon + Ammo + HP ──
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(24.0),
                    left: Val::Px(24.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(8.0),
                    ..default()
                },
                ..default()
            },
            HudRoot,
        ))
        .with_children(|parent| {
            // Slot icons row
            parent
                .spawn(NodeBundle {
                    style: Style {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(5.0),
                        ..default()
                    },
                    ..default()
                })
                .with_children(|row| {
                    for i in 0..3u8 {
                        let active = Color::srgba(1.0, 0.85, 0.2, 0.95);
                        let inactive = Color::srgba(0.4, 0.4, 0.45, 0.4);
                        let bc = if i == 0 { active } else { inactive };
                        let img = match i {
                            0 => weapon_assets.images[0].clone(),
                            2 => throwable_assets.grenade.clone(),
                            _ => weapon_assets.images[0].clone(),
                        };
                        let vis = if i == 1 {
                            Visibility::Hidden
                        } else {
                            Visibility::Inherited
                        };
                        row.spawn((
                            NodeBundle {
                                style: Style {
                                    width: Val::Px(51.0),
                                    height: Val::Px(51.0),
                                    border: UiRect::all(Val::Px(2.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    ..default()
                                },
                                border_color: BorderColor(bc),
                                background_color: BackgroundColor(
                                    Color::srgba(0.06, 0.06, 0.08, 0.5),
                                ),
                                ..default()
                            },
                            SlotBorder(i),
                        ))
                        .with_children(|slot| {
                            slot.spawn((
                                ImageBundle {
                                    style: Style {
                                        width: Val::Px(36.0),
                                        height: Val::Px(36.0),
                                        ..default()
                                    },
                                    image: UiImage::new(img),
                                    visibility: vis,
                                    ..default()
                                },
                                SlotIcon(i),
                            ));
                        });
                    }
                });
            // Weapon name
            parent.spawn((
                TextBundle::from_section(
                    "[1] PISTOL",
                    TextStyle {
                        font: font.clone(),
                        font_size: 15.0,
                        color: Color::srgba(1.0, 0.88, 0.35, 0.9),
                    },
                ),
                WeaponText,
            ));
            // Ammo
            parent.spawn((
                TextBundle::from_section(
                    "",
                    TextStyle {
                        font: font.clone(),
                        font_size: 14.0,
                        color: Color::srgba(0.65, 0.65, 0.6, 0.8),
                    },
                ),
                AmmoText,
            ));
            // Armor bar row — sits above HP, light/whitish so it reads as
            // "shield over flesh".  Width starts at 0 (no armor) and fills
            // proportionally to PLAYER_ARMOR_MAX.
            parent
                .spawn(NodeBundle {
                    style: Style {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(9.0),
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    ..default()
                })
                .with_children(|row| {
                    row.spawn(NodeBundle {
                        style: Style {
                            width: Val::Px(210.0),
                            height: Val::Px(6.0),
                            ..default()
                        },
                        background_color: BackgroundColor(Color::srgba(
                            0.30, 0.32, 0.34, 0.45,
                        )),
                        ..default()
                    })
                    .with_children(|bar| {
                        bar.spawn((
                            NodeBundle {
                                style: Style {
                                    width: Val::Percent(0.0),
                                    height: Val::Percent(100.0),
                                    ..default()
                                },
                                background_color: BackgroundColor(Color::srgba(
                                    0.95, 0.96, 0.99, 0.95,
                                )),
                                ..default()
                            },
                            ArmorBarFill,
                        ));
                    });
                    row.spawn((
                        TextBundle::from_section(
                            "0",
                            TextStyle {
                                font: font.clone(),
                                font_size: 12.0,
                                color: Color::srgba(0.92, 0.93, 0.96, 0.85),
                            },
                        ),
                        ArmorText,
                    ));
                });
            // HP bar row
            parent
                .spawn(NodeBundle {
                    style: Style {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(9.0),
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    ..default()
                })
                .with_children(|row| {
                    row.spawn(NodeBundle {
                        style: Style {
                            width: Val::Px(210.0),
                            height: Val::Px(8.0),
                            ..default()
                        },
                        background_color: BackgroundColor(Color::srgba(
                            0.15, 0.03, 0.03, 0.5,
                        )),
                        ..default()
                    })
                    .with_children(|bar| {
                        bar.spawn((
                            NodeBundle {
                                style: Style {
                                    width: Val::Percent(100.0),
                                    height: Val::Percent(100.0),
                                    ..default()
                                },
                                background_color: BackgroundColor(Color::srgb(
                                    0.9, 0.18, 0.18,
                                )),
                                ..default()
                            },
                            HpBarFill,
                        ));
                    });
                    row.spawn((
                        TextBundle::from_section(
                            "100",
                            TextStyle {
                                font: font.clone(),
                                font_size: 14.0,
                                color: Color::srgba(0.85, 0.3, 0.3, 0.85),
                            },
                        ),
                        HpText,
                    ));
                });
        });

    // ── Top-center: Wave ──
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    position_type: PositionType::Absolute,
                    top: Val::Px(15.0),
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(3.0),
                    ..default()
                },
                ..default()
            },
            HudRoot,
        ))
        .with_children(|parent| {
            parent.spawn((
                TextBundle::from_section(
                    "WAVE 1",
                    TextStyle {
                        font: font.clone(),
                        font_size: 24.0,
                        color: Color::srgba(1.0, 0.85, 0.25, 0.85),
                    },
                ),
                WaveTitleText,
            ));
            parent.spawn((
                TextBundle::from_section(
                    "",
                    TextStyle {
                        font: font.clone(),
                        font_size: 12.0,
                        color: Color::srgba(0.7, 0.7, 0.7, 0.6),
                    },
                ),
                WaveStatusText,
            ));
        });

    // ── Top-right: Score ──
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    position_type: PositionType::Absolute,
                    top: Val::Px(15.0),
                    right: Val::Px(24.0),
                    ..default()
                },
                ..default()
            },
            HudRoot,
        ))
        .with_children(|parent| {
            parent.spawn((
                TextBundle::from_section(
                    "$0",
                    TextStyle {
                        font: font.clone(),
                        font_size: 20.0,
                        color: Color::srgba(0.55, 0.95, 0.4, 0.9),
                    },
                ),
                ScoreValueText,
            ));
        });

    // ── Segment unlock prompt (bottom-center, hidden until in range) ──
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(225.0),
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    ..default()
                },
                visibility: Visibility::Hidden,
                ..default()
            },
            HudRoot,
            SegmentPromptRoot,
        ))
        .with_children(|panel| {
            panel.spawn(NodeBundle {
                style: Style {
                    padding: UiRect::axes(Val::Px(27.0), Val::Px(12.0)),
                    border: UiRect::all(Val::Px(3.0)),
                    ..default()
                },
                background_color: BackgroundColor(Color::srgba(0.06, 0.07, 0.10, 0.86)),
                border_color: BorderColor(Color::srgb(0.55, 0.46, 0.18)),
                ..default()
            })
            .with_children(|node| {
                node.spawn((
                    TextBundle::from_section(
                        "",
                        TextStyle {
                            font: font.clone(),
                            font_size: 21.0,
                            color: Color::srgba(0.92, 0.82, 0.32, 1.0),
                        },
                    ),
                    SegmentPromptText,
                ));
            });
        });

    // ── Top-left: multiplayer player list (visible only in Host/Client) ──
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    position_type: PositionType::Absolute,
                    top: Val::Px(15.0),
                    left: Val::Px(24.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(6.0),
                    width: Val::Px(270.0),
                    ..default()
                },
                visibility: Visibility::Hidden,
                ..default()
            },
            HudRoot,
            PlayerListRoot,
        ))
        .with_children(|panel| {
            for slot in 0u8..4 {
                panel
                    .spawn((
                        NodeBundle {
                            style: Style {
                                width: Val::Percent(100.0),
                                height: Val::Px(33.0),
                                flex_direction: FlexDirection::Row,
                                align_items: AlignItems::Center,
                                column_gap: Val::Px(9.0),
                                padding: UiRect::axes(Val::Px(9.0), Val::Px(3.0)),
                                border: UiRect::all(Val::Px(1.0)),
                                ..default()
                            },
                            background_color: BackgroundColor(Color::srgba(
                                0.05, 0.06, 0.08, 0.78,
                            )),
                            border_color: BorderColor(Color::srgba(0.18, 0.22, 0.25, 0.85)),
                            visibility: Visibility::Hidden,
                            ..default()
                        },
                        PlayerListSlot { slot },
                    ))
                    .with_children(|row| {
                        // Color stripe
                        row.spawn(NodeBundle {
                            style: Style {
                                width: Val::Px(12.0),
                                height: Val::Px(21.0),
                                ..default()
                            },
                            background_color: BackgroundColor(player_palette_color(slot)),
                            ..default()
                        });
                        // Nickname text (fixed width so HP bar aligns)
                        row.spawn(NodeBundle {
                            style: Style {
                                width: Val::Px(120.0),
                                ..default()
                            },
                            ..default()
                        })
                        .with_children(|cell| {
                            cell.spawn((
                                TextBundle::from_section(
                                    "",
                                    TextStyle {
                                        font: font.clone(),
                                        font_size: 17.0,
                                        color: Color::srgba(0.85, 0.85, 0.88, 0.95),
                                    },
                                ),
                                PlayerListNickText { slot },
                            ));
                        });
                        // HP bar (track + fill)
                        row.spawn(NodeBundle {
                            style: Style {
                                width: Val::Px(90.0),
                                height: Val::Px(12.0),
                                border: UiRect::all(Val::Px(1.0)),
                                ..default()
                            },
                            background_color: BackgroundColor(Color::srgba(
                                0.04, 0.05, 0.06, 0.95,
                            )),
                            border_color: BorderColor(Color::srgba(0.22, 0.24, 0.26, 0.9)),
                            ..default()
                        })
                        .with_children(|track| {
                            track.spawn((
                                NodeBundle {
                                    style: Style {
                                        width: Val::Percent(100.0),
                                        height: Val::Percent(100.0),
                                        ..default()
                                    },
                                    background_color: BackgroundColor(Color::srgba(
                                        0.55, 0.85, 0.4, 0.9,
                                    )),
                                    ..default()
                                },
                                PlayerListHpFill { slot },
                            ));
                        });
                    });
            }
        });

    // Damage overlay (full-screen red layer) — alpha is driven each frame
    // by `update_damage_overlay`.  Composed from the hit-flash timer plus a
    // pulsing low-HP factor.
    commands.spawn((
        NodeBundle {
            style: Style {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(0.0),
                ..default()
            },
            background_color: BackgroundColor(Color::srgba(0.85, 0.05, 0.06, 0.0)),
            z_index: ZIndex::Global(50),
            ..default()
        },
        HudRoot,
        DamageOverlay,
    ));

    // Floor indicator (bottom-center) — shows when player is inside a
    // multi-floor building.  Hidden otherwise.
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(60.0),
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    ..default()
                },
                visibility: Visibility::Hidden,
                ..default()
            },
            HudRoot,
            FloorIndicatorRoot,
        ))
        .with_children(|node| {
            node.spawn(NodeBundle {
                style: Style {
                    padding: UiRect::axes(Val::Px(14.0), Val::Px(6.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    ..default()
                },
                background_color: BackgroundColor(Color::srgba(0.05, 0.06, 0.08, 0.78)),
                border_color: BorderColor(Color::srgba(0.95, 0.85, 0.4, 0.85)),
                ..default()
            })
            .with_children(|panel| {
                panel.spawn((
                    TextBundle::from_section(
                        "PIETRO 0/0",
                        TextStyle {
                            font: font.clone(),
                            font_size: 14.0,
                            color: Color::srgba(0.98, 0.92, 0.7, 0.95),
                        },
                    ),
                    FloorIndicatorText,
                ));
            });
        });

    // Kill-streak combo counter — top-right under the wave HUD.  Hidden
    // until the player chains kills.
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    position_type: PositionType::Absolute,
                    top: Val::Px(80.0),
                    right: Val::Px(28.0),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::FlexEnd,
                    ..default()
                },
                z_index: ZIndex::Global(38),
                visibility: Visibility::Hidden,
                ..default()
            },
            HudRoot,
            ComboCounterRoot,
        ))
        .with_children(|node| {
            node.spawn((
                TextBundle::from_section(
                    "x0 KILL",
                    TextStyle {
                        font: font.clone(),
                        font_size: 22.0,
                        color: Color::srgba(1.0, 0.95, 0.55, 1.0),
                    },
                ),
                ComboCounterText,
            ));
        });

    // Boss alert — red flashing border + "GIANT INCOMING" centre text,
    // fires when a Giant spawns.  Hidden by default.
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    border: UiRect::all(Val::Px(8.0)),
                    ..default()
                },
                border_color: BorderColor(Color::srgba(0.95, 0.10, 0.12, 0.0)),
                z_index: ZIndex::Global(45),
                visibility: Visibility::Hidden,
                ..default()
            },
            HudRoot,
            BossAlertRoot,
        ))
        .with_children(|node| {
            // Inner backing panel so the text reads against any background.
            node.spawn(NodeBundle {
                style: Style {
                    padding: UiRect::axes(Val::Px(28.0), Val::Px(14.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    margin: UiRect::top(Val::Px(80.0)),
                    ..default()
                },
                background_color: BackgroundColor(Color::srgba(0.10, 0.04, 0.05, 0.85)),
                border_color: BorderColor(Color::srgba(0.95, 0.10, 0.12, 0.95)),
                ..default()
            })
            .with_children(|panel| {
                panel.spawn((
                    TextBundle::from_section(
                        "GIANT INCOMING",
                        TextStyle {
                            font: font.clone(),
                            font_size: 30.0,
                            color: Color::srgba(1.0, 0.18, 0.18, 1.0),
                        },
                    ),
                    BossAlertText,
                ));
            });
        });

    // Wave intro splash — big text centred on screen, fades in/out when a
    // new wave starts.  Hidden by default; `update_wave_intro` flips it.
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                z_index: ZIndex::Global(40),
                visibility: Visibility::Hidden,
                ..default()
            },
            HudRoot,
            WaveIntroRoot,
        ))
        .with_children(|node| {
            node.spawn((
                TextBundle::from_section(
                    "WAVE 1",
                    TextStyle {
                        font: font.clone(),
                        font_size: 64.0,
                        color: Color::srgba(1.0, 0.92, 0.45, 1.0),
                    },
                ),
                WaveIntroText,
            ));
        });

    // Pickup prompt (centred, slightly above the floor indicator).  Stays
    // hidden until the player walks onto a weapon pickup with slot 2 full.
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(110.0),
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    ..default()
                },
                visibility: Visibility::Hidden,
                ..default()
            },
            HudRoot,
            PickupPromptRoot,
        ))
        .with_children(|node| {
            node.spawn(NodeBundle {
                style: Style {
                    padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    ..default()
                },
                background_color: BackgroundColor(Color::srgba(0.05, 0.07, 0.10, 0.82)),
                border_color: BorderColor(Color::srgba(0.95, 0.85, 0.4, 0.92)),
                ..default()
            })
            .with_children(|panel| {
                panel.spawn((
                    TextBundle::from_section(
                        "[E] PRESS TO PICK UP",
                        TextStyle {
                            font: font.clone(),
                            font_size: 16.0,
                            color: Color::srgba(0.98, 0.92, 0.7, 0.95),
                        },
                    ),
                    PickupPromptText,
                ));
            });
        });

    // FPS counter (bottom-right, hidden by default)
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(21.0),
                    right: Val::Px(21.0),
                    ..default()
                },
                visibility: Visibility::Hidden,
                ..default()
            },
            HudRoot,
            FpsCounterRoot,
        ))
        .with_children(|node| {
            node.spawn((
                TextBundle::from_section(
                    "",
                    TextStyle {
                        font,
                        font_size: 14.0,
                        color: Color::srgba(0.5, 0.5, 0.5, 0.6),
                    },
                ),
                FpsCounterText,
            ));
        });
}

fn despawn_hud(mut commands: Commands, q: Query<Entity, With<HudRoot>>) {
    for e in &q {
        commands.entity(e).despawn_recursive();
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_hud(
    ctx: Res<NetContext>,
    players: Query<&Player>,
    score: Res<Score>,
    wave: Res<WaveState>,
    zombies: Query<(), With<Zombie>>,
    mut bars: ParamSet<(
        Query<&mut Style, With<HpBarFill>>,
        Query<&mut Style, With<ArmorBarFill>>,
    )>,
    mut hp_text: Query<
        &mut Text,
        (
            With<HpText>,
            Without<WaveTitleText>,
            Without<WaveStatusText>,
            Without<ScoreValueText>,
            Without<WeaponText>,
            Without<AmmoText>,
            Without<ArmorText>,
        ),
    >,
    mut armor_text: Query<
        &mut Text,
        (
            With<ArmorText>,
            Without<HpText>,
            Without<WaveTitleText>,
            Without<WaveStatusText>,
            Without<ScoreValueText>,
            Without<WeaponText>,
            Without<AmmoText>,
        ),
    >,
    mut wave_title: Query<
        &mut Text,
        (
            With<WaveTitleText>,
            Without<HpText>,
            Without<WaveStatusText>,
            Without<ScoreValueText>,
            Without<WeaponText>,
            Without<AmmoText>,
            Without<ArmorText>,
        ),
    >,
    mut wave_status: Query<
        &mut Text,
        (
            With<WaveStatusText>,
            Without<HpText>,
            Without<WaveTitleText>,
            Without<ScoreValueText>,
            Without<WeaponText>,
            Without<AmmoText>,
            Without<ArmorText>,
        ),
    >,
    mut score_text: Query<
        &mut Text,
        (
            With<ScoreValueText>,
            Without<HpText>,
            Without<WaveTitleText>,
            Without<WaveStatusText>,
            Without<WeaponText>,
            Without<AmmoText>,
            Without<ArmorText>,
        ),
    >,
    mut weapon_text: Query<
        &mut Text,
        (
            With<WeaponText>,
            Without<HpText>,
            Without<WaveTitleText>,
            Without<WaveStatusText>,
            Without<ScoreValueText>,
            Without<AmmoText>,
            Without<ArmorText>,
        ),
    >,
    mut ammo_text: Query<
        &mut Text,
        (
            With<AmmoText>,
            Without<HpText>,
            Without<WaveTitleText>,
            Without<WaveStatusText>,
            Without<ScoreValueText>,
            Without<WeaponText>,
            Without<ArmorText>,
        ),
    >,
) {
    let Some(player) = players.iter().find(|p| p.id == ctx.my_id) else {
        return;
    };
    let hp_pct = (player.hp.max(0) as f32 / PLAYER_MAX_HP as f32 * 100.0).clamp(0.0, 100.0);
    let armor_pct = (player.armor.max(0) as f32 / PLAYER_ARMOR_MAX as f32 * 100.0)
        .clamp(0.0, 100.0);
    if let Ok(mut style) = bars.p0().get_single_mut() {
        style.width = Val::Percent(hp_pct);
    }
    if let Ok(mut style) = bars.p1().get_single_mut() {
        style.width = Val::Percent(armor_pct);
    }
    if let Ok(mut text) = hp_text.get_single_mut() {
        text.sections[0].value = format!("{}", player.hp.max(0));
    }
    if let Ok(mut text) = armor_text.get_single_mut() {
        text.sections[0].value = format!("{}", player.armor.max(0));
    }
    // Weapon / slot display
    if let Ok(mut text) = weapon_text.get_single_mut() {
        let slot = player.active_slot;
        if slot <= 1 {
            let label = player.active_weapon().label();
            text.sections[0].value = format!("[{}] {}", slot + 1, label);
        } else {
            text.sections[0].value = format!("[3] {}", player.throwable_kind.label());
        }
    }
    // Ammo display
    if let Ok(mut text) = ammo_text.get_single_mut() {
        let slot = player.active_slot as usize;
        if slot <= 1 {
            let weapon = player.active_weapon();
            if weapon.has_infinite_ammo() {
                text.sections[0].value = "INF".to_string();
            } else if player.reload_timer > 0.0 {
                text.sections[0].value = "RELOADING...".to_string();
            } else {
                text.sections[0].value = format!(
                    "{}/{}  [{}]",
                    player.ammo[slot],
                    weapon.magazine_size(),
                    player.reserve_ammo[slot],
                );
            }
        } else {
            text.sections[0].value = format!("x{}", player.throwable_count);
        }
    }
    if let Ok(mut text) = wave_title.get_single_mut() {
        text.sections[0].value = if wave.in_break && wave.current_wave == 0 {
            "GET READY".to_string()
        } else if wave.in_break {
            format!("WAVE {} IN...", wave.current_wave + 1)
        } else {
            format!("WAVE {}", wave.current_wave)
        };
    }
    if let Ok(mut text) = wave_status.get_single_mut() {
        text.sections[0].value = if wave.in_break {
            format!("{:.1}s", wave.break_timer.remaining_secs())
        } else {
            let alive = zombies.iter().count();
            let left = wave.zombies_to_spawn as usize + alive;
            format!("ZOMBIES: {left}")
        };
    }
    if let Ok(mut text) = score_text.get_single_mut() {
        text.sections[0].value = format!("${}", score.0);
    }
}

fn update_slot_icons(
    ctx: Res<NetContext>,
    players: Query<&Player>,
    weapon_assets: Res<WeaponAssets>,
    throwable_assets: Res<ThrowableAssets>,
    mut icons: Query<(&mut UiImage, &mut Visibility, &SlotIcon)>,
    mut borders: Query<(&mut BorderColor, &SlotBorder)>,
) {
    let Some(player) = players.iter().find(|p| p.id == ctx.my_id) else {
        return;
    };
    for (mut img, mut vis, slot) in &mut icons {
        match slot.0 {
            0 | 1 => {
                let idx = slot.0 as usize;
                if let Some(w) = player.slots[idx] {
                    img.texture = weapon_assets.images[w.as_u8() as usize].clone();
                    *vis = Visibility::Inherited;
                } else {
                    *vis = Visibility::Hidden;
                }
            }
            2 => {
                if player.throwable_count > 0 {
                    img.texture = match player.throwable_kind {
                        ThrowableKind::Grenade => throwable_assets.grenade.clone(),
                        ThrowableKind::Smoke => throwable_assets.smoke.clone(),
                        ThrowableKind::Molotov => throwable_assets.molotov.clone(),
                    };
                    *vis = Visibility::Inherited;
                } else {
                    *vis = Visibility::Hidden;
                }
            }
            _ => {}
        }
    }
    for (mut border, slot) in &mut borders {
        border.0 = if slot.0 == player.active_slot {
            Color::srgba(1.0, 0.85, 0.2, 0.95)
        } else {
            Color::srgba(0.4, 0.4, 0.45, 0.4)
        };
    }
}

fn reset_wave_intro(mut state: ResMut<WaveIntroState>) {
    *state = WaveIntroState::default();
}

fn reset_combo(mut state: ResMut<ComboState>) {
    *state = ComboState::default();
}

/// Fires the boss-alert UI flash when a Giant spawns.  The border + text
/// pulse for a few seconds then auto-hide.
#[allow(clippy::type_complexity)]
fn update_boss_alert(
    time: Res<Time>,
    mut events: EventReader<SpawnZombieEvent>,
    mut alert: ResMut<BossAlertTimer>,
    mut root: Query<
        (&mut Visibility, &mut BorderColor),
        (With<BossAlertRoot>, Without<BossAlertText>),
    >,
    mut text_q: Query<&mut Text, With<BossAlertText>>,
) {
    for ev in events.read() {
        if matches!(ev.kind, ZombieKind::Giant) {
            alert.remaining = BOSS_ALERT_DURATION;
        }
    }
    alert.remaining = (alert.remaining - time.delta_seconds()).max(0.0);
    let active = alert.remaining > 0.0;
    if let Ok((mut vis, mut border)) = root.get_single_mut() {
        *vis = if active {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if active {
            // Fast pulse for that classic alarm feel.
            let pulse = (time.elapsed_seconds() * 7.0).sin() * 0.5 + 0.5;
            border.0 = Color::srgba(1.0, 0.12, 0.18, 0.5 + pulse * 0.4);
        }
    }
    if active {
        if let Ok(mut text) = text_q.get_single_mut() {
            // Punch the text alpha at the same pulse so the warning reads
            // even if the player is looking away from the centre.
            let pulse = (time.elapsed_seconds() * 7.0).sin() * 0.5 + 0.5;
            for sec in &mut text.sections {
                sec.style.color.set_alpha(0.65 + pulse * 0.35);
            }
        }
    }
}

/// Tracks consecutive kills and pumps the on-screen combo counter.  Tiered
/// label + colour so longer chains read as escalating: "KILL" → "STREAK" →
/// "RAMPAGE" → "UNSTOPPABLE".  Punch animation scales the text briefly
/// every fresh kill.
#[allow(clippy::type_complexity)]
fn update_combo(
    time: Res<Time>,
    mut state: ResMut<ComboState>,
    mut events: EventReader<ZombieKilledEvent>,
    mut root: Query<&mut Visibility, (With<ComboCounterRoot>, Without<ComboCounterText>)>,
    mut text_q: Query<(&mut Text, &mut Transform), With<ComboCounterText>>,
) {
    let dt = time.delta_seconds();
    state.time_since_kill += dt;
    state.punch_time += dt;

    let mut bumped = false;
    for _ in events.read() {
        if state.time_since_kill > COMBO_TIMEOUT {
            state.count = 1;
        } else {
            state.count += 1;
        }
        state.time_since_kill = 0.0;
        state.punch_time = 0.0;
        bumped = true;
    }
    let _ = bumped;

    if state.time_since_kill > COMBO_TIMEOUT {
        state.count = 0;
    }

    let active = state.count >= 2;
    if let Ok(mut vis) = root.get_single_mut() {
        *vis = if active {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }

    if !active {
        return;
    }

    // Tiered label + colour.
    let (label, color) = match state.count {
        2..=4 => (
            format!("x{} KILL", state.count),
            Color::srgba(1.0, 0.96, 0.62, 1.0),
        ),
        5..=9 => (
            format!("x{} STREAK", state.count),
            Color::srgba(1.0, 0.78, 0.32, 1.0),
        ),
        10..=19 => (
            format!("x{} RAMPAGE", state.count),
            Color::srgba(1.0, 0.45, 0.20, 1.0),
        ),
        _ => (
            format!("x{} UNSTOPPABLE", state.count),
            Color::srgba(1.0, 0.18, 0.18, 1.0),
        ),
    };

    // Fade out as the streak window runs out.
    let fade = ((COMBO_TIMEOUT - state.time_since_kill) / COMBO_TIMEOUT).clamp(0.0, 1.0);
    // Scale punch — quick overshoot to 1.4× then settle to 1.0 over 0.25 s.
    let punch_phase = (state.punch_time / 0.25).clamp(0.0, 1.0);
    let scale = 1.0 + (1.0 - punch_phase) * 0.4;

    if let Ok((mut text, mut transform)) = text_q.get_single_mut() {
        for sec in &mut text.sections {
            sec.value = label.clone();
            let mut c = color;
            c.set_alpha(fade);
            sec.style.color = c;
        }
        transform.scale = Vec3::new(scale, scale, 1.0);
    }
}

fn setup_postprocess_assets(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let vignette = images.add(build_vignette_image());
    let grain_frames = [
        images.add(build_grain_image(0)),
        images.add(build_grain_image(1)),
        images.add(build_grain_image(2)),
        images.add(build_grain_image(3)),
    ];
    commands.insert_resource(PostprocessAssets {
        vignette,
        grain_frames,
    });
}

/// Animated film grain overlay — cycles through 4 noise textures every
/// ~80 ms.  Currently dormant; left in source so re-enabling is a single
/// system-registration line away.
#[allow(clippy::type_complexity, dead_code)]
fn update_film_grain(
    time: Res<Time>,
    assets: Res<PostprocessAssets>,
    mut q: Query<&mut UiImage, With<FilmGrain>>,
    mut frame_timer: Local<f32>,
    mut frame_idx: Local<usize>,
) {
    *frame_timer += time.delta_seconds();
    if *frame_timer >= 0.08 {
        *frame_timer = 0.0;
        *frame_idx = (*frame_idx + 1) % assets.grain_frames.len();
        if let Ok(mut img) = q.get_single_mut() {
            img.texture = assets.grain_frames[*frame_idx].clone();
        }
    }
}

fn build_vignette_image() -> Image {
    // Radial alpha gradient: transparent in the centre, dark only at the
    // very corners.  Inner radius starts later (0.62 instead of 0.45) and
    // the alpha cap is much lower so the vignette frames the action
    // instead of blanketing it.
    let size: i32 = 128;
    let mut c = Canvas::new(size, size);
    let cx = size as f32 * 0.5;
    let cy = size as f32 * 0.5;
    let max_d = ((cx * cx) + (cy * cy)).sqrt();
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt() / max_d;
            let t = ((d - 0.62) / 0.38).clamp(0.0, 1.0);
            let a = (t.powf(2.4) * 110.0) as u8;
            if a > 0 {
                c.put(x, y, [4, 5, 8, a]);
            }
        }
    }
    c.into_image()
}

fn build_grain_image(seed: u32) -> Image {
    // 64×64 noise tile — only a small fraction of pixels carry any alpha
    // so the grain reads as occasional speckles rather than a uniform
    // fog.  Each frame uses a different seed so cycling them looks like
    // animated grain.
    let size: i32 = 64;
    let mut c = Canvas::new(size, size);
    let mut s: u32 = seed.wrapping_mul(0x9E3779B9).wrapping_add(0x12345);
    for y in 0..size {
        for x in 0..size {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let n = (s & 0xFF) as u8;
            // Sparse grain: only the brightest ~20% of pixels show, the
            // rest stay fully transparent so most of the screen remains
            // untouched.
            if n < 200 {
                continue;
            }
            let v = (n as i32 - 200).max(0) as u8 * 4;
            c.put(x, y, [v, v, v, (v / 4).min(18)]);
        }
    }
    c.into_image()
}

/// Big "WAVE N" splash centred on screen, fired once per new wave start.
/// Triggers on `wave.in_break == false` AND `wave.current_wave > last`.
#[allow(clippy::type_complexity)]
fn update_wave_intro(
    time: Res<Time>,
    wave: Res<WaveState>,
    mut state: ResMut<WaveIntroState>,
    mut root: Query<&mut Visibility, (With<WaveIntroRoot>, Without<WaveIntroText>)>,
    mut text_q: Query<&mut Text, With<WaveIntroText>>,
) {
    if !wave.in_break && wave.current_wave > state.last_wave {
        state.last_wave = wave.current_wave;
        state.flash_remaining = WAVE_INTRO_DURATION;
        if let Ok(mut text) = text_q.get_single_mut() {
            for sec in &mut text.sections {
                sec.value = format!("WAVE {}", wave.current_wave);
            }
        }
    }
    state.flash_remaining = (state.flash_remaining - time.delta_seconds()).max(0.0);
    let active = state.flash_remaining > 0.0;
    if let Ok(mut vis) = root.get_single_mut() {
        *vis = if active {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    if active {
        let pct = (state.flash_remaining / WAVE_INTRO_DURATION).clamp(0.0, 1.0);
        // Smooth fade-in for first 30% of life then steady, fade-out at end.
        let alpha = if pct > 0.7 {
            ((1.0 - pct) / 0.3).clamp(0.0, 1.0)
        } else if pct < 0.3 {
            (pct / 0.3).clamp(0.0, 1.0)
        } else {
            1.0
        };
        if let Ok(mut text) = text_q.get_single_mut() {
            for sec in &mut text.sections {
                sec.style.color.set_alpha(alpha);
            }
        }
    }
}

/// Drives the full-screen red overlay alpha every frame.  Composed from:
///   - a hit-flash that spikes on `PlayerDamagedEvent` and decays
///   - a pulsing low-HP layer that grows as HP drops below 30%.
/// Both layers max-blend so neither cancels the other.
fn update_damage_overlay(
    time: Res<Time>,
    ctx: Res<NetContext>,
    players: Query<&Player>,
    mut events: EventReader<PlayerDamagedEvent>,
    mut flash_timer: ResMut<HitFlashTimer>,
    mut overlays: Query<&mut BackgroundColor, With<DamageOverlay>>,
) {
    // Bump the flash on every relevant damage event for the local player.
    for ev in events.read() {
        if ev.target_id == ctx.my_id {
            flash_timer.remaining = HIT_FLASH_DURATION;
        }
    }
    flash_timer.remaining = (flash_timer.remaining - time.delta_seconds()).max(0.0);
    // Subtle hit flash — peaks at 0.28 alpha then fades quickly so the
    // player feels the hit without the screen turning into a red wall.
    let flash_alpha = (flash_timer.remaining / HIT_FLASH_DURATION).clamp(0.0, 1.0) * 0.28;

    // Low-HP pulse — kicks in below 30% HP, gentle pulse so the warning
    // is felt rather than blasted at the player.
    let local_hp = players
        .iter()
        .find(|p| p.id == ctx.my_id)
        .map(|p| p.hp)
        .unwrap_or(PLAYER_MAX_HP);
    let hp_pct = (local_hp.max(0) as f32 / PLAYER_MAX_HP as f32).clamp(0.0, 1.0);
    let low_hp_factor = ((0.30 - hp_pct) / 0.30).max(0.0);
    let pulse = (time.elapsed_seconds() * 3.0).sin() * 0.5 + 0.5;
    let low_hp_alpha = low_hp_factor * (0.06 + pulse * 0.10);

    let alpha = flash_alpha.max(low_hp_alpha).min(0.34);
    if let Ok(mut bg) = overlays.get_single_mut() {
        bg.0 = Color::srgba(0.92, 0.10, 0.10, alpha);
    }
}

/// Shows the "[E] PICK UP" prompt whenever the local player is overlapping
/// a weapon pickup that would replace a held weapon.  Hidden when slot 2
/// is empty (auto-pickup case) or when there's no nearby pickup.
#[allow(clippy::type_complexity)]
fn update_pickup_prompt(
    hint: Res<PickupPromptHint>,
    mut root: Query<&mut Visibility, (With<PickupPromptRoot>, Without<PickupPromptText>)>,
    mut text_q: Query<&mut Text, With<PickupPromptText>>,
) {
    let Ok(mut vis) = root.get_single_mut() else {
        return;
    };
    match hint.weapon {
        Some(w) => {
            *vis = Visibility::Inherited;
            if let Ok(mut text) = text_q.get_single_mut() {
                text.sections[0].value = format!("[E] PICK UP - {}", w.label());
            }
        }
        None => {
            *vis = Visibility::Hidden;
        }
    }
}

/// Toggles the floor indicator visibility based on whether the local
/// player is inside a multi-floor building, and shows the current
/// `floor + 1 / total` count plus the localised hint.
#[allow(clippy::type_complexity)]
fn update_floor_indicator(
    floor_state: Res<PlayerFloorState>,
    mut root: Query<&mut Visibility, (With<FloorIndicatorRoot>, Without<FloorIndicatorText>)>,
    mut text_q: Query<&mut Text, With<FloorIndicatorText>>,
) {
    let Ok(mut vis) = root.get_single_mut() else {
        return;
    };
    match floor_state.building {
        Some(b_idx) => {
            *vis = Visibility::Inherited;
            let kind = BUILDINGS[b_idx].kind;
            let total = building_floor_count(kind);
            // Translate the numeric floor index into a human label so the
            // ground floor reads "PARTER" (typical Polish convention).
            let label = match floor_state.floor {
                0 => "PARTER".to_string(),
                f if f as u8 + 1 == total => "DACH".to_string(),
                f => format!("{} PIETRO", f),
            };
            if let Ok(mut text) = text_q.get_single_mut() {
                text.sections[0].value = format!(
                    "{}  ({}/{})  - E aby zmienic",
                    label,
                    floor_state.floor + 1,
                    total,
                );
            }
        }
        None => {
            *vis = Visibility::Hidden;
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_fps_counter(
    time: Res<Time>,
    settings: Res<GraphicsSettings>,
    mut root_vis: Query<&mut Visibility, With<FpsCounterRoot>>,
    mut fps_text: Query<
        &mut Text,
        (
            With<FpsCounterText>,
            Without<HpText>,
            Without<WaveTitleText>,
            Without<WaveStatusText>,
            Without<ScoreValueText>,
            Without<WeaponText>,
            Without<AmmoText>,
        ),
    >,
    mut history: Local<VecDeque<f32>>,
) {
    let dt = time.delta_seconds();
    if dt > 0.0 {
        history.push_back(dt);
    }
    while history.len() > 60 {
        history.pop_front();
    }

    if let Ok(mut vis) = root_vis.get_single_mut() {
        *vis = if settings.show_fps {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }

    if settings.show_fps {
        if let Ok(mut text) = fps_text.get_single_mut() {
            let avg: f32 = if history.is_empty() {
                0.0
            } else {
                history.iter().sum::<f32>() / history.len() as f32
            };
            let fps = if avg > 0.0 {
                (1.0 / avg).round() as u32
            } else {
                0
            };
            text.sections[0].value = format!("{fps} FPS");
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_player_list(
    net: Res<NetMode>,
    nicknames: Res<PlayerNicknames>,
    players: Query<&Player>,
    mut root: Query<
        &mut Visibility,
        (
            With<PlayerListRoot>,
            Without<PlayerListSlot>,
            Without<PlayerListNickText>,
            Without<PlayerListHpFill>,
        ),
    >,
    mut slots: Query<
        (&PlayerListSlot, &mut Visibility),
        (
            Without<PlayerListRoot>,
            Without<PlayerListNickText>,
            Without<PlayerListHpFill>,
        ),
    >,
    mut nick_texts: Query<(&PlayerListNickText, &mut Text)>,
    mut hp_fills: Query<(&PlayerListHpFill, &mut Style)>,
) {
    let multiplayer = matches!(*net, NetMode::Host | NetMode::Client);
    if let Ok(mut vis) = root.get_single_mut() {
        *vis = if multiplayer {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    if !multiplayer {
        return;
    }

    // Build a quick lookup: slot index → matching player id, looking by sort
    // order of player ids so layout is stable across frames.
    let mut ids: Vec<u8> = players.iter().map(|p| p.id).collect();
    ids.sort_unstable();

    for (slot_marker, mut vis) in slots.iter_mut() {
        let active = (slot_marker.slot as usize) < ids.len();
        *vis = if active {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    // Update text + HP fill per slot.
    for (n, mut text) in nick_texts.iter_mut() {
        if let Some(&id) = ids.get(n.slot as usize) {
            let nick = nicknames
                .0
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("P{}", id));
            text.sections[0].value = nick;
        } else {
            text.sections[0].value.clear();
        }
    }
    for (hp_fill, mut style) in hp_fills.iter_mut() {
        if let Some(&id) = ids.get(hp_fill.slot as usize) {
            let hp = players
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.hp.max(0))
                .unwrap_or(0);
            let frac = (hp as f32 / PLAYER_MAX_HP as f32).clamp(0.0, 1.0);
            style.width = Val::Percent(frac * 100.0);
        } else {
            style.width = Val::Percent(0.0);
        }
    }
}

fn update_segment_prompt(
    hint: Res<SegmentUnlockHint>,
    mut roots: Query<&mut Visibility, With<SegmentPromptRoot>>,
    mut texts: Query<&mut Text, With<SegmentPromptText>>,
) {
    let Ok(mut vis) = roots.get_single_mut() else {
        return;
    };
    let Ok(mut text) = texts.get_single_mut() else {
        return;
    };
    match hint.segment_idx {
        Some(idx) => {
            *vis = Visibility::Inherited;
            let label = segment_name(idx);
            let section = &mut text.sections[0];
            if hint.affordable {
                section.value = format!("[E]  ODBLOKUJ {label}  -  ${}", hint.cost);
                section.style.color = Color::srgba(0.55, 0.95, 0.4, 1.0);
            } else {
                section.value = format!("BRAK $$$  -  {label}  KOSZT  ${}", hint.cost);
                section.style.color = Color::srgba(0.92, 0.45, 0.32, 1.0);
            }
        }
        None => {
            *vis = Visibility::Hidden;
        }
    }
}

fn spawn_game_over(
    mut commands: Commands,
    score: Res<Score>,
    wave: Res<WaveState>,
    assets: Res<UiAssets>,
) {
    let font = assets.font.clone();
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(22.0),
                    ..default()
                },
                background_color: BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.78)),
                ..default()
            },
            GameOverUi,
        ))
        .with_children(|parent| {
            parent.spawn(TextBundle::from_section(
                "GAME OVER",
                TextStyle {
                    font: font.clone(),
                    font_size: 64.0,
                    color: Color::srgb(0.9, 0.18, 0.18),
                },
            ));
            parent.spawn(TextBundle::from_section(
                format!("WAVE {}    ${}", wave.current_wave, score.0),
                TextStyle {
                    font: font.clone(),
                    font_size: 22.0,
                    color: Color::WHITE,
                },
            ));
            parent.spawn(TextBundle::from_section(
                "SPACE - main menu",
                TextStyle {
                    font,
                    font_size: 16.0,
                    color: Color::srgb(0.75, 0.75, 0.75),
                },
            ));
        });
}

fn despawn_game_over(mut commands: Commands, q: Query<Entity, With<GameOverUi>>) {
    for e in &q {
        commands.entity(e).despawn_recursive();
    }
}

fn game_over_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut next_state: ResMut<NextState<GameState>>,
    mut ctx: ResMut<NetContext>,
    mut mode: ResMut<NetMode>,
) {
    if keys.just_pressed(KeyCode::Space) || keys.just_pressed(KeyCode::Enter) {
        ctx.disconnect();
        *mode = NetMode::SinglePlayer;
        next_state.set(GameState::Menu);
    }
}
