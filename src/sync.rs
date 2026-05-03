use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::{HashSet, VecDeque};

use crate::bullet::{
    spawn_bullet_entity, spawn_explosion_entity, Bullet, BulletAssets, Explosion,
    EXPLOSION_LIFETIME,
};
use crate::map::MapSegmentUnlockState;
use crate::net::{
    broadcast, is_host, is_net_client, ClientInEvent, ClientMsg, LocalInput, NetBulletState,
    NetContext, NetEntities, NetExplosionState, NetMode, NetPickupState, NetPlayerState,
    NetSnapshot, NetZombieState, PlayerNicknames, RemoteInputs, ServerEvent, ServerMsg,
};
use crate::player::{spawn_player_entity, LogicalPos, Player, PlayerAssets};
use crate::wave::WaveState;
use crate::weapon::{
    spawn_armor_entity, spawn_health_entity, spawn_money_entity, spawn_pickup_entity, ArmorPickup,
    ExtraPickupAssets, HealthPickup, MoneyMultPickup, Weapon, WeaponAssets, WeaponPickup,
    ARMOR_PICKUP_KIND, HEALTH_PICKUP_KIND, MONEY2X_PICKUP_KIND, MONEY3X_PICKUP_KIND,
};
use crate::zombie::{spawn_zombie_entity, Zombie, ZombieAssets, ZombieKind};
use crate::{GameState, Score};

// ─── Snapshot rate / interpolation tuning ───────────────────────────────
//
// Server broadcasts every Nth `FixedUpdate` tick (60 Hz), so at N=2 we
// stream snapshots at ~30 Hz.  Inputs still flow at 60 Hz (they're cheap
// and latency-sensitive), so this only affects bandwidth in the host→client
// direction and the temporal granularity of remote-entity poses.
const SNAPSHOT_INTERVAL_TICKS: u64 = 2;
/// Approximate gap between snapshots in seconds; used to size the interp
/// buffer and the render-time delay.
const SNAPSHOT_INTERVAL_SECS: f32 = SNAPSHOT_INTERVAL_TICKS as f32 / 60.0;
/// Render remote entities this far behind the newest snapshot — a touch
/// over one snapshot interval gives enough buffer to ride out jitter
/// without making remote players feel laggy.
const INTERP_DELAY_SECS: f32 = SNAPSHOT_INTERVAL_SECS * 1.6;
/// How long we keep snapshots in memory for interpolation lookups.
const HISTORY_RETAIN_SECS: f32 = 0.5;

/// Buffered snapshots for interpolation.  Latest is the back; oldest is the
/// front.  `last_applied_tick` is bumped each time we run lifecycle apply.
#[derive(Resource, Default)]
pub struct SnapshotHistory {
    pub entries: VecDeque<BufferedSnapshot>,
    pub last_applied_tick: u64,
}

pub struct BufferedSnapshot {
    pub received: f64,
    pub snap: Box<NetSnapshot>,
}

impl SnapshotHistory {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.last_applied_tick = 0;
    }
}

pub struct NetSyncPlugin;

impl Plugin for NetSyncPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SnapshotHistory>()
            .add_systems(
                FixedUpdate,
                (server_receive_inputs, server_broadcast_snapshot)
                    .run_if(in_state(GameState::Playing))
                    .run_if(is_host),
            )
            .add_systems(
                FixedUpdate,
                client_send_input
                    .run_if(in_state(GameState::Playing))
                    .run_if(is_net_client),
            )
            .add_systems(
                Update,
                client_apply_snapshots
                    .run_if(in_state(GameState::Playing))
                    .run_if(is_net_client),
            )
            .add_systems(OnExit(GameState::Playing), reset_snapshot_history);
    }
}

fn reset_snapshot_history(mut history: ResMut<SnapshotHistory>) {
    history.clear();
}

fn server_receive_inputs(
    ctx: Res<NetContext>,
    mut remote: ResMut<RemoteInputs>,
    mut nicknames: ResMut<PlayerNicknames>,
) {
    let Some(host) = ctx.host.as_ref() else {
        return;
    };
    let events_arc = host.events.clone();
    let Ok(rx) = events_arc.lock() else {
        return;
    };
    while let Ok(e) = rx.try_recv() {
        match e {
            ServerEvent::Input { id, input } => {
                // Merge one-shot switch_slot to prevent lost inputs.
                let mut merged = input;
                if merged.switch_slot == 0 {
                    if let Some(prev) = remote.0.get(&id) {
                        merged.switch_slot = prev.switch_slot;
                    }
                }
                remote.0.insert(id, merged);
            }
            ServerEvent::Connected { id } => {
                info!("Client {} connected mid-game (not spawning)", id);
            }
            ServerEvent::Hello { id, nickname } => {
                nicknames.0.insert(id, nickname);
            }
            ServerEvent::Disconnected { id } => {
                remote.0.remove(&id);
                nicknames.0.remove(&id);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn server_broadcast_snapshot(
    ctx: Res<NetContext>,
    players: Query<(&Transform, &Player)>,
    zombies: Query<(&Transform, &crate::net::NetId, &Zombie)>,
    bullets: Query<(&Transform, &crate::net::NetId, &Bullet)>,
    pickups: Query<(&Transform, &crate::net::NetId, &WeaponPickup)>,
    health_pickups: Query<(&Transform, &crate::net::NetId), With<HealthPickup>>,
    armor_pickups: Query<(&Transform, &crate::net::NetId), With<ArmorPickup>>,
    money_pickups: Query<(&Transform, &crate::net::NetId, &MoneyMultPickup)>,
    explosions: Query<(&Transform, &crate::net::NetId, &Explosion)>,
    score: Res<Score>,
    wave: Res<WaveState>,
    segments: Res<MapSegmentUnlockState>,
    nicknames: Res<PlayerNicknames>,
    game_state: Res<State<GameState>>,
    mut tick: Local<u64>,
) {
    let Some(host) = ctx.host.as_ref() else {
        return;
    };
    *tick += 1;
    // Rate-limit snapshot broadcast to ~30 Hz so we don't flood the link
    // with redundant state.  Inputs still flow at the full FixedUpdate rate.
    // We always send the very first tick so the client sees the world
    // state immediately on join.
    if *tick > 1 && !(*tick).is_multiple_of(SNAPSHOT_INTERVAL_TICKS) {
        return;
    }

    let player_states: Vec<NetPlayerState> = players
        .iter()
        .map(|(t, p)| NetPlayerState {
            id: p.id,
            x: t.translation.x,
            y: t.translation.y,
            rot: t.rotation.to_euler(EulerRot::ZYX).0,
            hp: p.hp,
            armor: p.armor,
            active_slot: p.active_slot,
            slot1_weapon: p.slots[1].map(|w| w.as_u8()).unwrap_or(255),
        })
        .collect();

    let zombie_states: Vec<NetZombieState> = zombies
        .iter()
        .map(|(t, id, z)| NetZombieState {
            id: id.0,
            x: t.translation.x,
            y: t.translation.y,
            rot: t.rotation.to_euler(EulerRot::ZYX).0,
            kind: z.kind.as_u8(),
        })
        .collect();

    let bullet_states: Vec<NetBulletState> = bullets
        .iter()
        .map(|(t, id, b)| NetBulletState {
            id: id.0,
            x: t.translation.x,
            y: t.translation.y,
            rot: t.rotation.to_euler(EulerRot::ZYX).0,
            is_rocket: b.is_rocket,
        })
        .collect();

    let mut pickup_states: Vec<NetPickupState> = pickups
        .iter()
        .map(|(t, id, pk)| NetPickupState {
            id: id.0,
            x: t.translation.x,
            y: t.translation.y,
            kind: pk.kind.as_u8(),
        })
        .collect();
    for (t, id) in &health_pickups {
        pickup_states.push(NetPickupState {
            id: id.0,
            x: t.translation.x,
            y: t.translation.y,
            kind: HEALTH_PICKUP_KIND,
        });
    }
    for (t, id) in &armor_pickups {
        pickup_states.push(NetPickupState {
            id: id.0,
            x: t.translation.x,
            y: t.translation.y,
            kind: ARMOR_PICKUP_KIND,
        });
    }
    for (t, id, mp) in &money_pickups {
        pickup_states.push(NetPickupState {
            id: id.0,
            x: t.translation.x,
            y: t.translation.y,
            kind: if mp.factor >= 3 {
                MONEY3X_PICKUP_KIND
            } else {
                MONEY2X_PICKUP_KIND
            },
        });
    }

    let explosion_states: Vec<NetExplosionState> = explosions
        .iter()
        .map(|(t, id, exp)| NetExplosionState {
            id: id.0,
            x: t.translation.x,
            y: t.translation.y,
            radius: exp.radius,
            remaining: exp.lifetime,
        })
        .collect();

    let snap = NetSnapshot {
        tick: *tick,
        players: player_states,
        zombies: zombie_states,
        bullets: bullet_states,
        pickups: pickup_states,
        explosions: explosion_states,
        score: score.0,
        wave: wave.current_wave,
        in_break: wave.in_break,
        break_secs: wave.break_timer.remaining_secs(),
        zombies_to_spawn: wave.zombies_to_spawn,
        game_over: *game_state.get() == GameState::GameOver,
        unlocked_segments_mask: segments.as_mask(),
        player_nicknames: nicknames
            .0
            .iter()
            .map(|(&id, n)| (id, n.clone()))
            .collect(),
    };

    broadcast(host, &ServerMsg::Snapshot(Box::new(snap)));
}

fn client_send_input(ctx: Res<NetContext>, local: Res<LocalInput>) {
    if let Some(client) = ctx.client.as_ref() {
        let _ = client.sender.send(ClientMsg::Input(local.0));
    }
}

/// Bundled asset handles — the snapshot apply system already pushes Bevy's
/// 16-param-per-system limit, so we group these into one `SystemParam`.
#[derive(SystemParam)]
struct SnapshotAssets<'w> {
    player: Res<'w, PlayerAssets>,
    zombie: Res<'w, ZombieAssets>,
    bullet: Res<'w, BulletAssets>,
    weapon: Res<'w, WeaponAssets>,
    extra: Res<'w, ExtraPickupAssets>,
}

/// Find the (older, newer) snapshot pair whose received-times bracket
/// `render_time`, plus the [0, 1] alpha for lerping between them.
///
/// Edge cases:
/// - Single snapshot in buffer → `(Some(it), Some(it), 0.0)` (no interp).
/// - render_time before everything → use the two oldest, alpha 0.
/// - render_time after newest → use the two newest, alpha 1 (frozen at
///   newest; we deliberately don't extrapolate).
fn find_interp_pair(
    history: &VecDeque<BufferedSnapshot>,
    render_time: f64,
) -> Option<(usize, usize, f32)> {
    if history.is_empty() {
        return None;
    }
    if history.len() == 1 {
        return Some((0, 0, 0.0));
    }
    // Walk forward looking for the first entry with received >= render_time.
    let newer_idx = history
        .iter()
        .position(|e| e.received >= render_time)
        .unwrap_or(history.len() - 1);
    let older_idx = newer_idx.saturating_sub(1);
    if older_idx == newer_idx {
        return Some((older_idx, newer_idx, 0.0));
    }
    let older = &history[older_idx];
    let newer = &history[newer_idx];
    let span = (newer.received - older.received).max(1e-6);
    let alpha = ((render_time - older.received) / span).clamp(0.0, 1.0) as f32;
    Some((older_idx, newer_idx, alpha))
}

#[inline]
fn lerp_pos(o: (f32, f32), n: (f32, f32), a: f32) -> Vec2 {
    Vec2::new(o.0 + (n.0 - o.0) * a, o.1 + (n.1 - o.1) * a)
}

/// Picks the interpolated position from `(older, newer)` snapshot lookups,
/// falling back to whichever side has the entity (or `fallback` if neither
/// does — usually a freshly-spawned entity present only in `lifecycle`).
#[inline]
fn interp_or_fallback(
    older: Option<&(f32, f32)>,
    newer: Option<&(f32, f32)>,
    alpha: f32,
    fallback: (f32, f32),
) -> Vec2 {
    match (older, newer) {
        (Some(&o), Some(&n)) => lerp_pos(o, n, alpha),
        (_, Some(&n)) => Vec2::new(n.0, n.1),
        (Some(&o), None) => Vec2::new(o.0, o.1),
        (None, None) => Vec2::new(fallback.0, fallback.1),
    }
}

/// Despawns every entity whose key is not in `seen`, leaving the surviving
/// keys in `map`.  Used to drop replicated entities that the latest
/// snapshot no longer references.
fn despawn_stale<K>(
    commands: &mut Commands,
    map: &mut std::collections::HashMap<K, Entity>,
    seen: &HashSet<K>,
) where
    K: Eq + std::hash::Hash + Copy,
{
    let stale: Vec<K> = map.keys().filter(|k| !seen.contains(k)).copied().collect();
    for k in stale {
        if let Some(ent) = map.remove(&k) {
            commands.entity(ent).despawn_recursive();
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn client_apply_snapshots(
    time: Res<Time>,
    mut history: ResMut<SnapshotHistory>,
    mut commands: Commands,
    ctx: ResMut<NetContext>,
    mut mode: ResMut<NetMode>,
    mut net_entities: ResMut<NetEntities>,
    assets: SnapshotAssets,
    mut players: Query<
        (&mut Transform, &mut Player, Option<&mut LogicalPos>),
        (Without<Zombie>, Without<Bullet>, Without<Explosion>),
    >,
    mut zombies: Query<
        &mut Transform,
        (With<Zombie>, Without<Player>, Without<Bullet>, Without<Explosion>),
    >,
    mut bullets: Query<
        &mut Transform,
        (With<Bullet>, Without<Player>, Without<Zombie>, Without<Explosion>),
    >,
    mut explosions: Query<
        (&mut Explosion, &mut Sprite),
        (Without<Player>, Without<Zombie>, Without<Bullet>),
    >,
    mut score: ResMut<Score>,
    mut wave: ResMut<WaveState>,
    mut segments: ResMut<MapSegmentUnlockState>,
    mut nicknames: ResMut<PlayerNicknames>,
    mut next_state: ResMut<NextState<GameState>>,
) {
    let Some(client) = ctx.client.as_ref() else {
        return;
    };
    let events_arc = client.events.clone();

    // ── 1. Drain incoming events ──────────────────────────────────────────
    let mut new_snaps: Vec<Box<NetSnapshot>> = Vec::new();
    let mut disconnect = false;
    {
        let Ok(rx) = events_arc.lock() else {
            return;
        };
        while let Ok(e) = rx.try_recv() {
            match e {
                ClientInEvent::Snapshot(s) => {
                    // Drop stale snapshots — TCP guarantees order, but be
                    // defensive in case we ever swap the transport.
                    if s.tick > history.last_applied_tick {
                        new_snaps.push(s);
                    }
                }
                ClientInEvent::Disconnected | ClientInEvent::FullLobby => {
                    disconnect = true;
                }
                _ => {}
            }
        }
    }

    if disconnect {
        let mut ctx = ctx;
        ctx.disconnect();
        *mode = NetMode::SinglePlayer;
        net_entities.clear();
        history.clear();
        next_state.set(GameState::Menu);
        return;
    }

    // ── 2. Push new snapshots into the history buffer ─────────────────────
    let now = time.elapsed_seconds_f64();
    for s in new_snaps {
        history.entries.push_back(BufferedSnapshot {
            received: now,
            snap: s,
        });
    }

    // Trim history that's outside our retain window — but always keep at
    // least two so interpolation can still find a pair.
    let cutoff = now - HISTORY_RETAIN_SECS as f64;
    while history.entries.len() > 2 {
        let drop = history
            .entries
            .front()
            .map(|e| e.received < cutoff)
            .unwrap_or(false);
        if !drop {
            break;
        }
        history.entries.pop_front();
    }

    // Need at least one snapshot before we can do anything.
    let Some(latest_entry) = history.entries.back() else {
        return;
    };
    let latest_tick = latest_entry.snap.tick;

    // ── 3. Apply scalar wave/score state from latest ──────────────────────
    let lifecycle = latest_entry.snap.as_ref();
    score.0 = lifecycle.score;
    wave.current_wave = lifecycle.wave;
    wave.in_break = lifecycle.in_break;
    wave.zombies_to_spawn = lifecycle.zombies_to_spawn;
    wave.break_timer = Timer::from_seconds(lifecycle.break_secs.max(0.01), TimerMode::Once);
    segments.apply_mask(lifecycle.unlocked_segments_mask);
    nicknames.0.clear();
    for (id, n) in &lifecycle.player_nicknames {
        nicknames.0.insert(*id, n.clone());
    }

    // ── 4. Find interpolation pair for remote-entity poses ────────────────
    let render_time = now - INTERP_DELAY_SECS as f64;
    let pair = find_interp_pair(&history.entries, render_time);
    let (older_snap, newer_snap, alpha) = match pair {
        Some((oi, ni, a)) => (
            history.entries.get(oi).map(|e| e.snap.as_ref()),
            history.entries.get(ni).map(|e| e.snap.as_ref()),
            a,
        ),
        None => (None, None, 0.0),
    };

    // Pre-build id→pos lookups so per-entity interpolation is O(1).
    use std::collections::HashMap;
    let mut older_players: HashMap<u8, (f32, f32)> = HashMap::new();
    let mut newer_players: HashMap<u8, (f32, f32)> = HashMap::new();
    let mut older_zombies: HashMap<u32, (f32, f32)> = HashMap::new();
    let mut newer_zombies: HashMap<u32, (f32, f32)> = HashMap::new();
    let mut older_bullets: HashMap<u32, (f32, f32)> = HashMap::new();
    let mut newer_bullets: HashMap<u32, (f32, f32)> = HashMap::new();
    if let Some(o) = older_snap {
        for p in &o.players {
            older_players.insert(p.id, (p.x, p.y));
        }
        for z in &o.zombies {
            older_zombies.insert(z.id, (z.x, z.y));
        }
        for b in &o.bullets {
            older_bullets.insert(b.id, (b.x, b.y));
        }
    }
    if let Some(n) = newer_snap {
        for p in &n.players {
            newer_players.insert(p.id, (p.x, p.y));
        }
        for z in &n.zombies {
            newer_zombies.insert(z.id, (z.x, z.y));
        }
        for b in &n.bullets {
            newer_bullets.insert(b.id, (b.x, b.y));
        }
    }

    // ── 5. Players: lifecycle + interpolated remote positions ─────────────
    let my_id = ctx.my_id;
    let mut seen_players: HashSet<u8> = HashSet::new();
    for np in &lifecycle.players {
        seen_players.insert(np.id);
        match net_entities.players.get(&np.id).copied() {
            Some(ent) => {
                if let Ok((mut t, mut p, lp)) = players.get_mut(ent) {
                    p.hp = np.hp;
                    p.armor = np.armor;
                    p.active_slot = np.active_slot;
                    if np.slot1_weapon == 255 {
                        p.slots[1] = None;
                    } else {
                        p.slots[1] = Some(Weapon::from_u8(np.slot1_weapon));
                    }

                    if np.id == my_id {
                        // Local player: client-side prediction reconciles
                        // toward the server's authoritative position with
                        // a soft lerp.  We modify `LogicalPos.curr` (not
                        // Transform) so the reconciliation persists into
                        // the next FixedUpdate's sim — `interpolate_logical_pos`
                        // will produce the smooth Transform from prev→curr.
                        if let Some(mut lp) = lp {
                            lp.curr.x += (np.x - lp.curr.x) * 0.3;
                            lp.curr.y += (np.y - lp.curr.y) * 0.3;
                        } else {
                            // Fallback: no LogicalPos (shouldn't happen for
                            // the local player, but be safe).
                            t.translation.x += (np.x - t.translation.x) * 0.3;
                            t.translation.y += (np.y - t.translation.y) * 0.3;
                        }
                    } else {
                        // Remote player: render-time interpolation between
                        // the two history snapshots bracketing render_time.
                        // Remote players don't carry LogicalPos.
                        let target = interp_or_fallback(
                            older_players.get(&np.id),
                            newer_players.get(&np.id),
                            alpha,
                            (np.x, np.y),
                        );
                        t.translation.x = target.x;
                        t.translation.y = target.y;
                        t.rotation = Quat::from_rotation_z(np.rot);
                    }
                }
            }
            None => {
                let pos = Vec2::new(np.x, np.y);
                let ent = spawn_player_entity(&mut commands, &assets.player, np.id, pos);
                // Only the local player gets a LogicalPos — remote players
                // are interpolated solely via the snapshot history buffer.
                if np.id == my_id {
                    commands.entity(ent).insert(LogicalPos::at(pos));
                }
                net_entities.players.insert(np.id, ent);
            }
        }
    }
    despawn_stale(&mut commands, &mut net_entities.players, &seen_players);

    // ── 6. Zombies: interpolated positions, snapped rotation ──────────────
    let mut seen_zombies: HashSet<u32> = HashSet::new();
    for nz in &lifecycle.zombies {
        seen_zombies.insert(nz.id);
        let kind = ZombieKind::from_u8(nz.kind);
        match net_entities.zombies.get(&nz.id).copied() {
            Some(ent) => {
                if let Ok(mut t) = zombies.get_mut(ent) {
                    let target = interp_or_fallback(
                        older_zombies.get(&nz.id),
                        newer_zombies.get(&nz.id),
                        alpha,
                        (nz.x, nz.y),
                    );
                    t.translation.x = target.x;
                    t.translation.y = target.y;
                    t.rotation = Quat::from_rotation_z(nz.rot);
                }
            }
            None => {
                let ent = spawn_zombie_entity(
                    &mut commands,
                    &assets.zombie,
                    Vec2::new(nz.x, nz.y),
                    nz.id,
                    kind.base_hp(),
                    kind.base_speed(),
                    kind,
                );
                net_entities.zombies.insert(nz.id, ent);
            }
        }
    }
    despawn_stale(&mut commands, &mut net_entities.zombies, &seen_zombies);

    // ── 7. Bullets: interpolated positions ────────────────────────────────
    let mut seen_bullets: HashSet<u32> = HashSet::new();
    for nb in &lifecycle.bullets {
        seen_bullets.insert(nb.id);
        match net_entities.bullets.get(&nb.id).copied() {
            Some(ent) => {
                if let Ok(mut t) = bullets.get_mut(ent) {
                    let target = interp_or_fallback(
                        older_bullets.get(&nb.id),
                        newer_bullets.get(&nb.id),
                        alpha,
                        (nb.x, nb.y),
                    );
                    t.translation.x = target.x;
                    t.translation.y = target.y;
                    t.rotation = Quat::from_rotation_z(nb.rot);
                }
            }
            None => {
                let ent = spawn_bullet_entity(
                    &mut commands,
                    &assets.bullet,
                    Vec2::new(nb.x, nb.y),
                    Vec2::new(nb.rot.cos(), nb.rot.sin()),
                    0.0,
                    0,
                    nb.id,
                    nb.is_rocket,
                    false,
                    None,
                    false,
                );
                net_entities.bullets.insert(nb.id, ent);
            }
        }
    }
    despawn_stale(&mut commands, &mut net_entities.bullets, &seen_bullets);

    // ── 8. Pickups: spawn-only (static positions) ─────────────────────────
    let mut seen_pickups: HashSet<u32> = HashSet::new();
    for np in &lifecycle.pickups {
        seen_pickups.insert(np.id);
        use std::collections::hash_map::Entry;
        if let Entry::Vacant(entry) = net_entities.pickups.entry(np.id) {
            let ent = match np.kind {
                HEALTH_PICKUP_KIND => spawn_health_entity(
                    &mut commands,
                    &assets.extra,
                    Vec2::new(np.x, np.y),
                    np.id,
                ),
                ARMOR_PICKUP_KIND => spawn_armor_entity(
                    &mut commands,
                    &assets.extra,
                    Vec2::new(np.x, np.y),
                    np.id,
                ),
                MONEY2X_PICKUP_KIND => spawn_money_entity(
                    &mut commands,
                    &assets.extra,
                    Vec2::new(np.x, np.y),
                    2,
                    np.id,
                ),
                MONEY3X_PICKUP_KIND => spawn_money_entity(
                    &mut commands,
                    &assets.extra,
                    Vec2::new(np.x, np.y),
                    3,
                    np.id,
                ),
                _ => {
                    let kind = Weapon::from_u8(np.kind);
                    spawn_pickup_entity(
                        &mut commands,
                        &assets.weapon,
                        Vec2::new(np.x, np.y),
                        kind,
                        np.id,
                    )
                }
            };
            entry.insert(ent);
        }
    }
    despawn_stale(&mut commands, &mut net_entities.pickups, &seen_pickups);

    // ── 9. Explosions: anim state from latest ─────────────────────────────
    let mut seen_explosions: HashSet<u32> = HashSet::new();
    for ne in &lifecycle.explosions {
        seen_explosions.insert(ne.id);
        match net_entities.explosions.get(&ne.id).copied() {
            Some(ent) => {
                if let Ok((mut exp, mut sprite)) = explosions.get_mut(ent) {
                    exp.lifetime = ne.remaining;
                    exp.radius = ne.radius;
                    let t = (ne.remaining / EXPLOSION_LIFETIME).clamp(0.0, 1.0);
                    let phase = 1.0 - t;
                    let scale = 1.1 + phase * 1.0;
                    sprite.custom_size = Some(Vec2::splat(ne.radius * scale));
                }
            }
            None => {
                let ent = spawn_explosion_entity(
                    &mut commands,
                    &assets.bullet,
                    Vec2::new(ne.x, ne.y),
                    ne.radius,
                    ne.id,
                );
                net_entities.explosions.insert(ne.id, ent);
            }
        }
    }
    despawn_stale(&mut commands, &mut net_entities.explosions, &seen_explosions);

    if lifecycle.game_over {
        next_state.set(GameState::GameOver);
    }

    history.last_applied_tick = latest_tick;
}
