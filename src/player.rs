use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use rand::Rng;

use std::collections::HashSet;

use crate::audio::SfxEvent;
use crate::bullet::{spawn_walking_dust, BulletAssets, ShootEvent, ThrowEvent};
use crate::map::{MapObstacles, MAP_HEIGHT, MAP_WIDTH, PLAYER_SPAWN_X, PLAYER_SPAWN_Y};
use crate::net::{
    is_authoritative, is_net_client, LocalInput, NetContext, NetEntities, NetMode, RemoteInputs,
};
use crate::pixelart::{Canvas, Rgba};
use crate::weapon::{ThrowableKind, Weapon};
use crate::{gameplay_active, GameState, Score};

const PLAYER_SPRITE_SIZE: Vec2 = Vec2::new(30.0, 25.0);

pub const PLAYER_RADIUS: f32 = 10.0;
pub const PLAYER_SPEED: f32 = 260.0;
pub const PLAYER_MAX_HP: i32 = 100;
pub const PLAYER_INVULN: f32 = 0.5;

/// Render-time interpolation buffer for entities whose canonical position
/// is updated at the (60 Hz) FixedUpdate rate.  Holds the position at the
/// start (`prev`) and end (`curr`) of the most recent FixedUpdate batch;
/// `interpolate_logical_pos` lerps `Transform.translation` between them
/// using `Time<Fixed>::overstep_fraction()` so movement looks smooth at
/// any render FPS (165, 240, etc.) instead of stepping at 60 Hz.
///
/// Added per spawn site:
/// - `spawn_players` (host/SP) and `wave_system` respawn — every player
/// - `client_apply_snapshots` snapshot-driven spawn — only the local player
///   (remote players already get smooth render-time interp from the
///   snapshot history buffer; double-interpolating would fight that).
#[derive(Component, Debug, Clone, Copy)]
pub struct LogicalPos {
    pub prev: Vec2,
    pub curr: Vec2,
}

impl LogicalPos {
    pub fn at(p: Vec2) -> Self {
        Self { prev: p, curr: p }
    }
}

#[derive(Component)]
pub struct Player {
    pub id: u8,
    pub hp: i32,
    /// Soft-armor pool that absorbs incoming damage before HP — full armor
    /// effectively doubles the player's effective health.  Capped at
    /// `PLAYER_ARMOR_MAX`; armor pickups refill to full.
    pub armor: i32,
    pub fire_cooldown: f32,
    pub invuln_timer: f32,
    pub aim: Vec2,
    // Inventory: 2 weapon slots + throwable
    pub slots: [Option<Weapon>; 2],
    pub active_slot: u8, // 0 or 1 = weapon, 2 = throwable
    pub ammo: [u32; 2],
    pub reserve_ammo: [u32; 2],
    pub reload_timer: f32,
    pub throwable_kind: ThrowableKind,
    pub throwable_count: u32,
    pub throw_cooldown: f32,
    // Money multiplier (1x by default; 2x/3x while multiplier_timer > 0)
    pub money_mult: u8,
    pub money_mult_timer: f32,
}

/// Max armor pool is the same as max HP, so a fully-armored player has
/// twice the effective health.  Armor pickups refill the pool to full.
pub const PLAYER_ARMOR_MAX: i32 = PLAYER_MAX_HP;

impl Player {
    pub fn active_weapon(&self) -> Weapon {
        if self.active_slot <= 1 {
            self.slots[self.active_slot as usize].unwrap_or(Weapon::Pistol)
        } else {
            Weapon::Pistol
        }
    }

    #[allow(dead_code)]
    pub fn weapon_in_slot(&self, slot: usize) -> Option<Weapon> {
        self.slots.get(slot).copied().flatten()
    }
}

#[derive(Event)]
pub struct PlayerDamagedEvent {
    pub target_id: u8,
    pub amount: i32,
}

/// Fired the moment a player runs out of HP — listeners drop blood pools,
/// flash the screen, etc.  Position is the death location in world coords.
#[derive(Event)]
pub struct PlayerDiedEvent {
    #[allow(dead_code)] // Will gate per-player effects in future MP work.
    pub player_id: u8,
    pub pos: Vec2,
}

#[derive(Resource, Default)]
pub struct DeadPlayers(pub Vec<u8>);

/// World-space reload progress bar — always stays horizontal above the
/// local player.  Two child sprites (background + fill) so the fill width
/// scales with progress.
#[derive(Component)]
pub struct ReloadBarRoot;

#[derive(Component)]
pub struct ReloadBarFill;

#[derive(Resource)]
pub struct PlayerAssets {
    pub images: [Handle<Image>; 4],
}

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<PlayerDamagedEvent>()
            .add_event::<PlayerDiedEvent>()
            .init_resource::<DeadPlayers>()
            .add_systems(Startup, setup_player_assets)
            .add_systems(OnEnter(GameState::Playing), spawn_players)
            .add_systems(OnExit(GameState::Playing), despawn_players)
            .add_systems(
                Update,
                (gather_local_input, emit_walking_dust, update_reload_bar)
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(OnEnter(GameState::Playing), spawn_reload_bar)
            // Render-time interpolation: restore canonical Transform before
            // the FixedUpdate sim (so it doesn't read a lerped value),
            // snapshot the post-sim position after, and lerp in Update.
            .add_systems(
                FixedFirst,
                restore_logical_pos.run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                FixedLast,
                snapshot_logical_pos.run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                Update,
                interpolate_logical_pos.run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                FixedUpdate,
                (server_player_tick, player_damage_handler)
                    .chain()
                    .run_if(gameplay_active)
                    .run_if(is_authoritative),
            )
            .add_systems(
                FixedUpdate,
                client_local_predict
                    .run_if(in_state(GameState::Playing))
                    .run_if(is_net_client),
            );
    }
}

fn restore_logical_pos(mut q: Query<(&mut Transform, &mut LogicalPos)>) {
    for (mut t, mut lp) in q.iter_mut() {
        // Snapshot the pre-sim canonical state and restore Transform to it
        // so any FixedUpdate sim runs from the deterministic position
        // (not from whatever lerp `interpolate_logical_pos` left behind).
        lp.prev = lp.curr;
        t.translation.x = lp.curr.x;
        t.translation.y = lp.curr.y;
    }
}

fn snapshot_logical_pos(mut q: Query<(&Transform, &mut LogicalPos)>) {
    for (t, mut lp) in q.iter_mut() {
        lp.curr.x = t.translation.x;
        lp.curr.y = t.translation.y;
    }
}

/// Renders Transform at `lerp(prev, curr, overstep_fraction)` so movement
/// is smooth at any FPS even when sim updates only at 60 Hz.
pub fn interpolate_logical_pos(
    fixed_time: Res<Time<Fixed>>,
    mut q: Query<(&mut Transform, &LogicalPos)>,
) {
    let alpha = fixed_time.overstep_fraction();
    for (mut t, lp) in q.iter_mut() {
        t.translation.x = lp.prev.x + (lp.curr.x - lp.prev.x) * alpha;
        t.translation.y = lp.prev.y + (lp.curr.y - lp.prev.y) * alpha;
    }
}

fn setup_player_assets(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let imgs: [Handle<Image>; 4] = [
        images.add(build_player_image(0)),
        images.add(build_player_image(1)),
        images.add(build_player_image(2)),
        images.add(build_player_image(3)),
    ];
    commands.insert_resource(PlayerAssets { images: imgs });
}

/// Public accessor for the per-player colour triplet so the HUD can colour
/// the player-list rows to match the in-world sprite.
pub fn player_palette_color(id: u8) -> Color {
    let (mid, _, _) = player_palette(id);
    Color::srgba(
        mid[0] as f32 / 255.0,
        mid[1] as f32 / 255.0,
        mid[2] as f32 / 255.0,
        1.0,
    )
}

fn player_palette(id: u8) -> (Rgba, Rgba, Rgba) {
    match id % 4 {
        0 => (
            [100, 145, 230, 255],
            [60, 95, 175, 255],
            [32, 55, 110, 255],
        ),
        1 => (
            [225, 90, 75, 255],
            [170, 55, 50, 255],
            [95, 24, 24, 255],
        ),
        2 => (
            [110, 190, 90, 255],
            [65, 135, 60, 255],
            [35, 80, 32, 255],
        ),
        _ => (
            [235, 195, 60, 255],
            [180, 140, 35, 255],
            [105, 78, 18, 255],
        ),
    }
}

fn build_player_image(id: u8) -> Image {
    let (body_light, body_main, body_dark) = player_palette(id);
    let outline: Rgba = [14, 12, 8, 255];
    let skin: Rgba = [225, 185, 140, 255];
    let skin_shadow: Rgba = [188, 148, 108, 255];
    let eye: Rgba = [32, 22, 14, 255];
    let vest: Rgba = [44, 48, 40, 255];
    let vest_dark: Rgba = [28, 32, 24, 255];
    let vest_hi: Rgba = [62, 66, 54, 255];
    let belt: Rgba = [34, 28, 16, 255];
    let pouch: Rgba = [50, 42, 26, 255];
    let boot: Rgba = [22, 18, 12, 255];
    let gun_body: Rgba = [42, 42, 50, 255];
    let gun_hi: Rgba = [78, 78, 88, 255];
    let gun_dark: Rgba = [22, 22, 28, 255];
    let stock: Rgba = [72, 46, 20, 255];
    let stock_dark: Rgba = [48, 30, 12, 255];
    let muzzle: Rgba = [56, 56, 62, 255];

    let mut c = Canvas::new(25, 21);

    // Body
    c.fill_circle(9, 10, 7, outline);
    c.fill_circle(9, 10, 6, body_dark);
    c.fill_circle(9, 10, 5, body_main);
    c.fill_circle(7, 8, 2, body_light);

    // Boots
    c.put(5, 15, boot);
    c.put(6, 15, boot);
    c.put(12, 15, boot);
    c.put(13, 15, boot);

    // Tactical vest
    c.fill_rect(5, 6, 8, 8, vest_dark);
    c.fill_rect(6, 7, 6, 6, vest);
    c.put(6, 7, vest_hi);
    c.put(7, 7, vest_hi);
    c.put(6, 8, vest_hi);
    // Vest pockets
    c.fill_rect(7, 10, 2, 2, vest_dark);
    c.fill_rect(10, 10, 2, 2, vest_dark);

    // Belt with pouches
    c.fill_rect(4, 13, 10, 1, belt);
    c.fill_rect(5, 13, 2, 1, pouch);
    c.fill_rect(10, 13, 2, 1, pouch);

    // Weapon stock
    c.fill_rect(12, 9, 3, 3, stock_dark);
    c.fill_rect(12, 9, 3, 1, stock);
    c.put(12, 10, stock);

    // Arm reaching to weapon
    c.fill_rect(13, 8, 4, 3, outline);
    c.fill_rect(13, 9, 3, 1, skin);
    c.put(14, 8, skin_shadow);
    c.put(15, 8, skin);

    // Gun barrel
    c.fill_rect(16, 9, 8, 3, gun_dark);
    c.fill_rect(16, 9, 7, 2, gun_body);
    c.fill_rect(16, 9, 4, 1, gun_hi);
    c.put(23, 9, muzzle);
    c.put(23, 10, muzzle);
    c.put(24, 9, outline);
    c.put(24, 10, outline);

    // Head
    c.fill_circle(15, 10, 3, outline);
    c.fill_circle(15, 10, 2, skin);
    c.put(14, 9, skin_shadow);

    // Eye
    c.put(17, 10, eye);

    // Cap / helmet in player color
    c.fill_rect(13, 7, 5, 2, outline);
    c.fill_rect(13, 7, 4, 1, body_dark);
    c.put(14, 7, body_main);
    c.fill_rect(16, 6, 2, 2, body_main);
    c.put(17, 6, body_light);

    c.into_image()
}

pub fn spawn_player_entity(
    commands: &mut Commands,
    assets: &PlayerAssets,
    id: u8,
    pos: Vec2,
) -> Entity {
    commands
        .spawn((
            SpriteBundle {
                texture: assets.images[(id as usize) % 4].clone(),
                sprite: Sprite {
                    custom_size: Some(PLAYER_SPRITE_SIZE),
                    ..default()
                },
                transform: Transform::from_xyz(pos.x, pos.y, 10.0),
                ..default()
            },
            Player {
                id,
                hp: PLAYER_MAX_HP,
                armor: 0,
                fire_cooldown: 0.0,
                invuln_timer: 0.0,
                aim: Vec2::X,
                slots: [Some(Weapon::Pistol), None],
                active_slot: 0,
                ammo: [Weapon::Pistol.magazine_size(), 0],
                reserve_ammo: [Weapon::Pistol.reserve_ammo(), 0],
                reload_timer: 0.0,
                throwable_kind: ThrowableKind::Grenade,
                throwable_count: 3,
                throw_cooldown: 0.0,
                money_mult: 1,
                money_mult_timer: 0.0,
            },
        ))
        .id()
}

fn spawn_players(
    mut commands: Commands,
    assets: Res<PlayerAssets>,
    mut score: ResMut<Score>,
    net: Res<NetMode>,
    ctx: Res<NetContext>,
    mut net_entities: ResMut<NetEntities>,
    mut dead: ResMut<DeadPlayers>,
) {
    score.0 = 0;
    net_entities.clear();
    dead.0.clear();

    let ids: Vec<u8> = match *net {
        NetMode::SinglePlayer => vec![0],
        NetMode::Host => {
            if ctx.lobby_players.is_empty() {
                vec![0]
            } else {
                ctx.lobby_players.clone()
            }
        }
        NetMode::Client => return,
    };

    for (idx, id) in ids.iter().enumerate() {
        // Spawn south of the atrium fountain in a short row
        let col = idx % 4;
        let row = idx / 4;
        let pos = Vec2::new(
            PLAYER_SPAWN_X + col as f32 * 64.0,
            PLAYER_SPAWN_Y - row as f32 * 64.0,
        );
        let ent = spawn_player_entity(&mut commands, &assets, *id, pos);
        // SP/Host: every player is simulated locally at 60 Hz, so they all
        // need the render-time interpolation buffer.
        commands.entity(ent).insert(LogicalPos::at(pos));
        net_entities.players.insert(*id, ent);
    }
}

fn despawn_players(
    mut commands: Commands,
    q: Query<Entity, With<Player>>,
    bars: Query<Entity, With<ReloadBarRoot>>,
    mut net_entities: ResMut<NetEntities>,
) {
    for e in &q {
        commands.entity(e).despawn_recursive();
    }
    for e in &bars {
        commands.entity(e).despawn_recursive();
    }
    net_entities.clear();
}

const RELOAD_BAR_WIDTH: f32 = 40.0;
const RELOAD_BAR_HEIGHT: f32 = 4.0;
const RELOAD_BAR_OFFSET_Y: f32 = 24.0;

/// Spawns the singleton reload-bar entities (background + fill).  Hidden
/// until the player starts reloading.
fn spawn_reload_bar(mut commands: Commands) {
    let bg = commands
        .spawn((
            SpriteBundle {
                sprite: Sprite {
                    color: Color::srgba(0.05, 0.05, 0.07, 0.85),
                    custom_size: Some(Vec2::new(RELOAD_BAR_WIDTH, RELOAD_BAR_HEIGHT)),
                    ..default()
                },
                transform: Transform::from_xyz(0.0, 0.0, 11.0),
                visibility: Visibility::Hidden,
                ..default()
            },
            ReloadBarRoot,
        ))
        .id();
    let fill = commands
        .spawn((
            SpriteBundle {
                sprite: Sprite {
                    color: Color::srgba(0.95, 0.85, 0.30, 0.95),
                    custom_size: Some(Vec2::new(0.0, RELOAD_BAR_HEIGHT - 1.0)),
                    ..default()
                },
                // Sprite anchor is centre — we'll offset the fill so it
                // grows from the LEFT edge in `update_reload_bar`.
                transform: Transform::from_xyz(0.0, 0.0, 11.5),
                visibility: Visibility::Hidden,
                ..default()
            },
            ReloadBarFill,
        ))
        .id();
    let _ = (bg, fill); // suppress unused variable warnings
}

#[allow(clippy::type_complexity)]
fn update_reload_bar(
    ctx: Res<NetContext>,
    players: Query<(&Transform, &Player)>,
    mut bg_q: Query<
        (&mut Transform, &mut Visibility),
        (With<ReloadBarRoot>, Without<ReloadBarFill>, Without<Player>),
    >,
    mut fill_q: Query<
        (&mut Transform, &mut Sprite, &mut Visibility),
        (With<ReloadBarFill>, Without<ReloadBarRoot>, Without<Player>),
    >,
) {
    let local = players.iter().find(|(_, p)| p.id == ctx.my_id);
    let active = local
        .map(|(_, p)| p.reload_timer > 0.0 && p.hp > 0)
        .unwrap_or(false);

    let (Ok((mut bg_t, mut bg_vis)), Ok((mut fill_t, mut fill_sprite, mut fill_vis))) =
        (bg_q.get_single_mut(), fill_q.get_single_mut())
    else {
        return;
    };

    if !active {
        *bg_vis = Visibility::Hidden;
        *fill_vis = Visibility::Hidden;
        return;
    }
    let Some((player_t, player)) = local else {
        return;
    };
    let Some(weapon) = player.slots[player.active_slot.min(1) as usize] else {
        *bg_vis = Visibility::Hidden;
        *fill_vis = Visibility::Hidden;
        return;
    };
    let total = weapon.reload_time().max(0.001);
    let progress = ((total - player.reload_timer) / total).clamp(0.0, 1.0);

    let pp = player_t.translation.truncate();
    let bar_y = pp.y + RELOAD_BAR_OFFSET_Y;
    bg_t.translation.x = pp.x;
    bg_t.translation.y = bar_y;
    *bg_vis = Visibility::Inherited;

    let fill_w = (RELOAD_BAR_WIDTH - 2.0) * progress;
    fill_sprite.custom_size = Some(Vec2::new(fill_w.max(0.0), RELOAD_BAR_HEIGHT - 1.0));
    // Anchor fill so it grows from the LEFT edge of the bar.
    fill_t.translation.x = pp.x - (RELOAD_BAR_WIDTH - 2.0 - fill_w) * 0.5;
    fill_t.translation.y = bar_y;
    *fill_vis = Visibility::Inherited;
}

/// Spawns small dust puffs at the local player's feet whenever they're
/// actually moving — gives weight to the walk animation without any actual
/// frames.  Throttled so the puffs don't pile up.
fn emit_walking_dust(
    mut commands: Commands,
    time: Res<Time>,
    bullet_assets: Res<BulletAssets>,
    local: Res<LocalInput>,
    ctx: Res<NetContext>,
    players: Query<(&Transform, &Player)>,
    mut timer: Local<f32>,
) {
    *timer -= time.delta_seconds();
    if *timer > 0.0 {
        return;
    }
    *timer = 0.16; // 6 puffs/sec while moving
    let local_pos = players
        .iter()
        .find(|(_, p)| p.id == ctx.my_id)
        .filter(|(_, p)| p.hp > 0)
        .map(|(t, _)| t.translation.truncate());
    let Some(p) = local_pos else {
        return;
    };
    let mv = Vec2::new(local.0.move_x, local.0.move_y);
    if mv.length_squared() < 0.05 {
        return;
    }
    // Spawn slightly behind the player relative to their movement.
    let offset = mv.normalize_or_zero() * -8.0;
    spawn_walking_dust(&mut commands, &bullet_assets, p + offset);
}

fn gather_local_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    players: Query<(&Transform, &Player)>,
    ctx: Res<NetContext>,
    mut local: ResMut<LocalInput>,
) {
    let mut mv = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        mv.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        mv.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        mv.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        mv.x += 1.0;
    }
    mv = mv.normalize_or_zero();
    local.0.move_x = mv.x;
    local.0.move_y = mv.y;
    local.0.shoot = mouse.pressed(MouseButton::Left);
    local.0.throw = mouse.just_pressed(MouseButton::Right);
    local.0.reload = keys.just_pressed(KeyCode::KeyR);

    // Interact (E) — segment unlock confirmation.  Latch on press so the
    // value reaches FixedUpdate even if input arrived between ticks; cleared
    // when the unlock system consumes it.
    if keys.just_pressed(KeyCode::KeyE) {
        local.0.interact = true;
    }

    // Slot switching (sticky: only set, never clear — FixedUpdate may run less often than Update)
    if keys.just_pressed(KeyCode::Digit1) {
        local.0.switch_slot = 1;
    } else if keys.just_pressed(KeyCode::Digit2) {
        local.0.switch_slot = 2;
    } else if keys.just_pressed(KeyCode::Digit3) {
        local.0.switch_slot = 3;
    }

    let Ok(window) = windows.get_single() else {
        return;
    };
    let Ok((camera, cam_transform)) = cameras.get_single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Some(world) = camera.viewport_to_world_2d(cam_transform, cursor) else {
        return;
    };

    let my_pos = players
        .iter()
        .find(|(_, p)| p.id == ctx.my_id)
        .map(|(t, _)| t.translation.truncate());

    if let Some(pos) = my_pos {
        let dir = (world - pos).normalize_or_zero();
        if dir != Vec2::ZERO {
            local.0.aim_x = dir.x;
            local.0.aim_y = dir.y;
        }
    }
}

fn client_local_predict(
    time: Res<Time>,
    local: Res<LocalInput>,
    ctx: Res<NetContext>,
    obstacles: Res<MapObstacles>,
    mut players: Query<(&mut Transform, &mut Player)>,
) {
    let dt = time.delta_seconds();
    for (mut transform, mut player) in &mut players {
        if player.id != ctx.my_id {
            continue;
        }
        // Movement prediction
        let mv = Vec2::new(local.0.move_x, local.0.move_y).normalize_or_zero();
        if mv != Vec2::ZERO {
            transform.translation += (mv * PLAYER_SPEED * dt).extend(0.0);
        }
        let half_w = MAP_WIDTH / 2.0 - PLAYER_RADIUS;
        let half_h = MAP_HEIGHT / 2.0 - PLAYER_RADIUS;
        transform.translation.x = transform.translation.x.clamp(-half_w, half_w);
        transform.translation.y = transform.translation.y.clamp(-half_h, half_h);

        let mut pos = transform.translation.truncate();
        obstacles.resolve(&mut pos, PLAYER_RADIUS);
        transform.translation.x = pos.x.clamp(-half_w, half_w);
        transform.translation.y = pos.y.clamp(-half_h, half_h);

        // Aim rotation
        let aim = Vec2::new(local.0.aim_x, local.0.aim_y);
        if aim.length_squared() > 0.0001 {
            let aim = aim.normalize();
            player.aim = aim;
            transform.rotation = Quat::from_rotation_z(aim.y.atan2(aim.x));
        }

        // Slot switching (local instant feedback)
        match local.0.switch_slot {
            1 => {
                if player.active_slot != 0 {
                    player.active_slot = 0;
                }
            }
            2 => {
                if player.slots[1].is_some() && player.active_slot != 1 {
                    player.active_slot = 1;
                }
            }
            3 => {
                if player.throwable_count > 0 && player.active_slot != 2 {
                    player.active_slot = 2;
                }
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn server_player_tick(
    time: Res<Time>,
    local: Res<LocalInput>,
    remote: Res<RemoteInputs>,
    ctx: Res<NetContext>,
    obstacles: Res<MapObstacles>,
    mut players: Query<(&mut Transform, &mut Player)>,
    mut shoot_events: EventWriter<ShootEvent>,
    mut throw_events: EventWriter<ThrowEvent>,
    mut sfx: EventWriter<SfxEvent>,
) {
    let dt = time.delta_seconds();
    for (mut transform, mut player) in &mut players {
        let input = if player.id == ctx.my_id {
            local.0
        } else {
            remote.0.get(&player.id).copied().unwrap_or_default()
        };

        let mv = Vec2::new(input.move_x, input.move_y);
        let mv = if mv.length_squared() > 1.0 {
            mv.normalize()
        } else {
            mv
        };
        if mv != Vec2::ZERO {
            transform.translation += (mv * PLAYER_SPEED * dt).extend(0.0);
        }
        let half_w = MAP_WIDTH / 2.0 - PLAYER_RADIUS;
        let half_h = MAP_HEIGHT / 2.0 - PLAYER_RADIUS;
        transform.translation.x = transform.translation.x.clamp(-half_w, half_w);
        transform.translation.y = transform.translation.y.clamp(-half_h, half_h);

        let mut pos = transform.translation.truncate();
        obstacles.resolve(&mut pos, PLAYER_RADIUS);
        transform.translation.x = pos.x.clamp(-half_w, half_w);
        transform.translation.y = pos.y.clamp(-half_h, half_h);

        let aim = Vec2::new(input.aim_x, input.aim_y);
        let aim = if aim.length_squared() > 0.0001 {
            aim.normalize()
        } else {
            player.aim
        };
        player.aim = aim;
        transform.rotation = Quat::from_rotation_z(aim.y.atan2(aim.x));

        if player.fire_cooldown > 0.0 {
            player.fire_cooldown -= dt;
        }
        if player.invuln_timer > 0.0 {
            player.invuln_timer -= dt;
        }
        if player.throw_cooldown > 0.0 {
            player.throw_cooldown -= dt;
        }
        if player.money_mult_timer > 0.0 {
            player.money_mult_timer -= dt;
            if player.money_mult_timer <= 0.0 {
                player.money_mult_timer = 0.0;
                player.money_mult = 1;
            }
        }

        // Slot switching (1/2/3)
        match input.switch_slot {
            1 => {
                if player.active_slot != 0 {
                    player.active_slot = 0;
                    player.reload_timer = 0.0;
                    player.fire_cooldown = 0.15;
                }
            }
            2 => {
                if player.slots[1].is_some() && player.active_slot != 1 {
                    player.active_slot = 1;
                    player.reload_timer = 0.0;
                    player.fire_cooldown = 0.15;
                }
            }
            3 => {
                if player.throwable_count > 0 && player.active_slot != 2 {
                    player.active_slot = 2;
                    player.reload_timer = 0.0;
                }
            }
            _ => {}
        }

        // Reload logic (auto-reload when magazine empty, or manual with R)
        let slot = player.active_slot as usize;
        if slot <= 1 {
            if let Some(weapon) = player.slots[slot] {
                if !weapon.has_infinite_ammo() {
                    // Start reload: manual (R) or auto when magazine empty
                    if player.reload_timer <= 0.0
                        && player.ammo[slot] < weapon.magazine_size()
                        && player.reserve_ammo[slot] > 0
                        && (input.reload || player.ammo[slot] == 0)
                    {
                        player.reload_timer = weapon.reload_time();
                        player.fire_cooldown = weapon.reload_time();
                    }
                    // Complete reload
                    if player.reload_timer > 0.0 {
                        player.reload_timer -= dt;
                        if player.reload_timer <= 0.0 {
                            let need = weapon.magazine_size() - player.ammo[slot];
                            let fill = need.min(player.reserve_ammo[slot]);
                            player.ammo[slot] += fill;
                            player.reserve_ammo[slot] -= fill;
                            player.reload_timer = 0.0;
                        }
                    }
                }
            }
        }

        if player.hp <= 0 {
            continue;
        }

        // Throw (right click or left click when slot 3)
        let wants_throw = input.throw || (input.shoot && player.active_slot == 2);
        if wants_throw && player.throw_cooldown <= 0.0 && player.throwable_count > 0 {
            let origin = transform.translation.truncate() + player.aim * (PLAYER_RADIUS + 6.0);
            throw_events.send(ThrowEvent {
                origin,
                direction: player.aim,
                kind: player.throwable_kind,
            });
            player.throwable_count -= 1;
            player.throw_cooldown = 0.6;
            sfx.send(SfxEvent::Shot);
            // Switch back to weapon if throwables ran out
            if player.throwable_count == 0 && player.active_slot == 2 {
                player.active_slot = 0;
            }
            continue;
        }

        // Shooting (only from weapon slots)
        if player.active_slot > 1 {
            continue;
        }
        let weapon = player.active_weapon();
        let has_ammo = weapon.has_infinite_ammo() || player.ammo[slot] > 0;
        if input.shoot && player.fire_cooldown <= 0.0 && has_ammo && player.reload_timer <= 0.0 {
            player.fire_cooldown = weapon.fire_cooldown();
            let origin = transform.translation.truncate() + player.aim * (PLAYER_RADIUS + 8.0);
            let count = weapon.bullet_count();
            let spread = weapon.spread();
            let damage = weapon.bullet_damage();
            let speed = weapon.bullet_speed();
            let is_rocket = weapon.is_rocket();
            let homing = weapon.is_homing();
            let is_flame = weapon.is_flame();
            let mut rng = rand::thread_rng();
            for _ in 0..count {
                let angle = if spread > 0.0 {
                    rng.gen_range(-spread..spread)
                } else {
                    0.0
                };
                let (sin, cos) = angle.sin_cos();
                let dir = Vec2::new(
                    player.aim.x * cos - player.aim.y * sin,
                    player.aim.x * sin + player.aim.y * cos,
                );
                // Tiny per-puff speed jitter on flames so the cone breaks
                // up nicely — straight-line tracers look fake here.
                let puff_speed = if is_flame {
                    speed * rng.gen_range(0.85..1.15)
                } else {
                    speed
                };
                shoot_events.send(ShootEvent {
                    origin,
                    direction: dir,
                    damage,
                    speed: puff_speed,
                    is_rocket,
                    homing,
                    is_flame,
                });
            }
            // Consume ammo
            if !weapon.has_infinite_ammo() {
                player.ammo[slot] = player.ammo[slot].saturating_sub(1);
            }
            sfx.send(SfxEvent::Shot);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn player_damage_handler(
    mut commands: Commands,
    mut events: EventReader<PlayerDamagedEvent>,
    mut died_evw: EventWriter<PlayerDiedEvent>,
    mut players: Query<(Entity, &Transform, &mut Player)>,
    mut net_entities: ResMut<NetEntities>,
    mut sfx: EventWriter<SfxEvent>,
    mut next_state: ResMut<NextState<GameState>>,
    mut dead_players: ResMut<DeadPlayers>,
) {
    if events.is_empty() {
        return;
    }
    let mut newly_dead: HashSet<u8> = HashSet::new();
    for ev in events.read() {
        for (_, _t, mut player) in &mut players {
            if player.id != ev.target_id {
                continue;
            }
            if player.invuln_timer > 0.0 || player.hp <= 0 {
                continue;
            }
            // Armor absorbs damage first.  Anything past the remaining
            // armor pool spills over into HP — keeps small hits cheap and
            // big rocket / giant hits painful even with full armor.
            let mut remaining = ev.amount;
            if player.armor > 0 {
                let absorbed = remaining.min(player.armor);
                player.armor -= absorbed;
                remaining -= absorbed;
            }
            if remaining > 0 {
                player.hp -= remaining;
            }
            player.invuln_timer = PLAYER_INVULN;
            sfx.send(SfxEvent::PlayerHit);
            if player.hp <= 0 {
                newly_dead.insert(player.id);
            }
        }
    }
    if newly_dead.is_empty() {
        return;
    }

    for (entity, t, player) in &players {
        if newly_dead.contains(&player.id) {
            died_evw.send(PlayerDiedEvent {
                player_id: player.id,
                pos: t.translation.truncate(),
            });
            commands.entity(entity).despawn_recursive();
            net_entities.players.remove(&player.id);
            if !dead_players.0.contains(&player.id) {
                dead_players.0.push(player.id);
            }
        }
    }

    let survivors = players
        .iter()
        .filter(|(_, _, p)| p.hp > 0 && !newly_dead.contains(&p.id))
        .count();
    if survivors == 0 {
        next_state.set(GameState::GameOver);
    }
}
