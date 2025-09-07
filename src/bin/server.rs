use std::collections::HashMap;
use bevy::prelude::*;
use bevy::winit::WinitPlugin; // headless VPS では無効化する
use bevy::app::ScheduleRunnerPlugin; // Winit を無効化したらループ駆動を自前で
use std::time::Duration;
use std::env;
use bevy::time::Fixed;
use bevy::ecs::system::SystemParam;
use bevy::prelude::Name;
use bevy_rapier3d::prelude::*;
use bevy_renet::renet::{ClientId, RenetServer};
use bevy_renet::transport::NetcodeServerPlugin;
use bevy_renet::RenetServerPlugin;

#[path = "../net.rs"]
mod net;
use crate::net::*;

#[derive(Resource, Default)]
struct Players {
    states: HashMap<u64, PlayerState>,
}

#[derive(Default, Clone, Copy)]
struct PlayerState {
    pos: Vec3,
    yaw: f32,
    hp: u16,
    alive: bool,
    vy: f32,
    grounded: bool,
}

// --- Bots ---
#[derive(Default, Clone, Copy)]
struct BotState {
    pos: Vec3,
    yaw: f32,
    hp: u16,
    alive: bool,
    vy: f32,
    grounded: bool,
}

#[derive(Resource, Default)]
struct Bots { states: HashMap<u64, BotState> }

#[derive(Resource, Default)]
struct BotEntities(HashMap<u64, Entity>);

#[derive(Resource, Default)]
struct BotRespawnTimers(HashMap<u64, f32>);

#[derive(Resource)]
struct NextBotId(u64);

#[derive(Resource, Default)]
struct ProtectTimers(HashMap<u64, f32>);

#[derive(Resource, Default)]
struct BotFocus(HashMap<u64, (Option<u64>, f32)>); // (target_id, lock_time)

#[derive(SystemParam)]
struct WpnProt<'w> {
    weapons: ResMut<'w, Weapons>,
    protect: ResMut<'w, ProtectTimers>,
}

#[derive(Resource, Default)]
struct LastInputs(HashMap<u64, InputFrame>);

#[derive(Resource, Default)]
struct LastFireSeq(HashMap<u64, u32>);

#[derive(Resource, Default)]
struct RespawnTimers(HashMap<u64, f32>);

#[derive(Resource, Default)]
struct ServerEntities(HashMap<u64, Entity>);

#[derive(Resource, Default)]
struct MapReady(pub bool);

#[derive(Resource, Default)]
struct Scores(HashMap<u64, (u32, u32)>); // id -> (kills, deaths)

#[derive(Resource, Default)]
struct SpawnPoints(pub Vec<Vec3>);
#[derive(Resource, Default)]
struct JumpCounts(HashMap<u64, u8>);

#[derive(Clone, Copy, PartialEq, Eq)]
enum RoundPhase { Active, Ending }

#[derive(Resource)]
struct RoundState {
    phase: RoundPhase,
    time_left: f32,
    end_timer: f32,
}

const WIN_KILLS: u32 = 10;
const ROUND_TIME_SEC: f32 = 300.0; // 5 min
const ROUND_END_DELAY_SEC: f32 = 5.0;

// Weapon constants
const MAG_SIZE: u16 = 12;
const RELOAD_TIME: f32 = 1.6; // sec
const FIRE_COOLDOWN: f32 = 1.0 / 7.5; // ~450 RPM

#[derive(Default, Clone, Copy)]
struct WeaponStatus { ammo: u16, cooldown: f32, reload: f32 }

#[derive(Resource, Default)]
struct Weapons(HashMap<u64, WeaponStatus>);

const DESIRED_BOTS: usize = 5;
const BOT_SPAWN_COOLDOWN: f32 = 2.0;
const BOT_ID_START: u64 = 1_000_000_000_000; // 衝突低確率な帯を使用
const BOT_MOVE_SPEED: f32 = 5.5;
const BOT_FIRE_RANGE: f32 = 60.0;
const BOT_FOV_COS: f32 = 0.5; // 約60度（厳しめに）
const BOT_TURN_RATE: f32 = 6.0; // rad/s: 向き直り速度
const BOT_REACT_SEC: f32 = 0.25; // 目標を捉えてから撃つまでの反応時間
const BOT_FIRE_COOLDOWN: f32 = 0.18; // 連射間隔
const BOT_DMG: u16 = 20; // botの与ダメージ
const BOT_SPREAD_BASE: f32 = 0.015; // 基本拡散（ラジアン）
const BOT_SPREAD_DIST_K: f32 = 0.01; // 距離による拡散増加
const BOT_AIRBORNE_SPREAD_MUL: f32 = 1.5; // 空中ターゲット拡散倍率

const SPAWN_JITTER_RADIUS: f32 = 6.0; // スポーン分散半径
const PROTECT_SEC: f32 = 2.0; // リスポーン保護（無敵・発砲不可）

fn main() {
    App::new()
        // ヘッドレス運用: WinitPlugin（X/Wayland依存のイベントループ）を無効化
        // WindowPlugin は primary_window=None で維持（Asset や Render 依存を壊さない）
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    close_when_requested: false,
                    ..default()
                })
                .disable::<WinitPlugin>()
        )
        // ヘッドレスでスケジュールを駆動するランナー（60Hz）
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(1.0/60.0)))
        .add_plugins((RenetServerPlugin, NetcodeServerPlugin))
        .add_plugins(RapierPhysicsPlugin::<NoUserData>::default())
        .insert_resource(Players::default())
        .insert_resource(LastInputs::default())
        .insert_resource(LastFireSeq::default())
        .insert_resource(RespawnTimers::default())
        .insert_resource(Scores::default())
        .insert_resource(RoundState { phase: RoundPhase::Active, time_left: ROUND_TIME_SEC, end_timer: 0.0 })
        .insert_resource(SpawnPoints::default())
        .insert_resource(JumpCounts::default())
        .insert_resource(Weapons::default())
        .insert_resource(ServerEntities::default())
        .insert_resource(Bots::default())
        .insert_resource(BotEntities::default())
        .insert_resource(BotRespawnTimers::default())
        .insert_resource(NextBotId(BOT_ID_START))
        .insert_resource(ProtectTimers::default())
        .insert_resource(BotFocus::default())
        .insert_resource(MapReady(false))
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(SnapshotTimer(Timer::from_seconds(1.0/30.0, TimerMode::Repeating)))
        .insert_resource(ServerLogTimer(Timer::from_seconds(1.0, TimerMode::Repeating)))
        .add_systems(Startup, (setup_server, setup_map))
        .add_systems(Update, (accept_clients, add_mesh_colliders_for_map, collect_spawn_points_from_map, ensure_bots, log_clients_count))
        .add_systems(Update, sync_players_with_connections)
        .add_systems(FixedUpdate, recv_inputs)
        .add_systems(FixedUpdate, srv_kcc_move.before(PhysicsSet::StepSimulation))
        .add_systems(FixedUpdate, bot_kcc_move.before(PhysicsSet::StepSimulation))
        .add_systems(FixedUpdate, srv_kcc_post.after(PhysicsSet::Writeback))
        .add_systems(FixedUpdate, bot_kcc_post.after(PhysicsSet::Writeback))
        .add_systems(FixedUpdate, srv_shoot_and_respawn.after(srv_kcc_post))
        .add_systems(FixedUpdate, bot_ai_shoot_and_respawn.after(srv_shoot_and_respawn))
        .add_systems(FixedUpdate, broadcast_snapshots.after(bot_ai_shoot_and_respawn))
        .add_systems(FixedUpdate, round_update.after(broadcast_snapshots))
        .run();
}

fn setup_server(mut commands: Commands) {
    let (server, transport) = new_server();
    commands.insert_resource(server);
    commands.insert_resource(transport);
    info!("Server listening on 0.0.0.0:{}", SERVER_PORT);
}

fn accept_clients(
    mut commands: Commands,
    mut server: ResMut<RenetServer>,
    mut players: ResMut<Players>,
    mut ents: ResMut<ServerEntities>,
    mut scores: ResMut<Scores>,
    round: Res<RoundState>,
    spawns: Res<SpawnPoints>,
    mut weapons: ResMut<Weapons>,
    mut protect: ResMut<ProtectTimers>,
) {
    while let Some(event) = server.get_event() {
        match event {
            bevy_renet::renet::ServerEvent::ClientConnected { client_id } => {
                let id = client_id.raw();
                let mut spawn = choose_spawn_point(&spawns, &players);
                // スポーン分散ジッター
                let jitter = Vec3::new((rand::random::<f32>()-0.5)*2.0*SPAWN_JITTER_RADIUS, 0.0, (rand::random::<f32>()-0.5)*2.0*SPAWN_JITTER_RADIUS);
                spawn += jitter;
                players.states.insert(id, PlayerState { pos: spawn, yaw: 0.0, hp: 100, alive: true, vy: 0.0, grounded: true });
                let ent = commands.spawn((
                    TransformBundle::from_transform(Transform::from_translation(spawn)),
                    Collider::capsule_y(0.6, 0.3),
                    KinematicCharacterController::default(),
                )).id();
                ents.0.insert(id, ent);
                info!("server: inserted player state for {} (total={})", id, players.states.len());
                // broadcast spawn
                let ev = ServerMessage::Event(EventMsg::Spawn { id, pos: [spawn.x, spawn.y, spawn.z], kind: ActorKind::Human });
                let bytes = bincode::serialize(&ev).unwrap();
                for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                scores.0.entry(id).or_insert((0,0));
                weapons.0.insert(id, WeaponStatus { ammo: MAG_SIZE, cooldown: 0.0, reload: 0.0 });
                if let Ok(bytes) = bincode::serialize(&ServerMessage::Event(EventMsg::Ammo { id, ammo: MAG_SIZE, reloading: false })) { let _ = server.send_message(client_id, CH_RELIABLE, bytes); }
                // スポーン保護
                protect.0.insert(id, PROTECT_SEC);
                info!("client connected: {} (protect {:.1}s)", id, PROTECT_SEC);
                // 現在のラウンド残り時間を通知
                let ev = ServerMessage::Event(EventMsg::RoundStart { time_left_sec: round.time_left.max(0.0) as u32 });
                if let Ok(bytes) = bincode::serialize(&ev) { let _ = server.send_message(client_id, CH_RELIABLE, bytes); }
            }
            bevy_renet::renet::ServerEvent::ClientDisconnected { client_id, reason } => {
                let id = client_id.raw();
                players.states.remove(&id);
                if let Some(e) = ents.0.remove(&id) { commands.entity(e).despawn_recursive(); }
                let ev = ServerMessage::Event(EventMsg::Despawn { id });
                let bytes = bincode::serialize(&ev).unwrap();
                for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                info!("client disconnected: {} ({:?})", id, reason);
                scores.0.remove(&id);
                weapons.0.remove(&id);
            }
        }
    }
}

const MAP_SCENE_PATH: &str = "maps/map.glb#Scene0";

fn setup_map(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(SceneBundle { scene: asset_server.load(MAP_SCENE_PATH), ..default() });
}

fn is_human_id(players: &Players, id: u64) -> bool { players.states.contains_key(&id) }
fn is_bot_id(bots: &Bots, id: u64) -> bool { bots.states.contains_key(&id) }

fn ensure_bots(
    mut commands: Commands,
    mut bots: ResMut<Bots>,
    mut bot_ents: ResMut<BotEntities>,
    mut ents: ResMut<ServerEntities>,
    mut next_id: ResMut<NextBotId>,
    mut weapons: ResMut<Weapons>,
    spawns: Res<SpawnPoints>,
    _players: Res<Players>,
    mut server: ResMut<RenetServer>,
    mut protect: ResMut<ProtectTimers>,
) {
    // 既に規定数いれば何もしない
    if bots.states.len() >= DESIRED_BOTS { return; }
    // スポーン位置
    let base_pos = if !spawns.0.is_empty() { spawns.0[rand::random::<usize>() % spawns.0.len()] } else { Vec3::new(0.0, 10.0, 5.0) };
    while bots.states.len() < DESIRED_BOTS {
        let id = { let cur = next_id.0; next_id.0 += 1; cur };
        let mut pos = base_pos;
        // 少し散らす
        let jitter = Vec3::new((rand::random::<f32>()-0.5)*2.0*SPAWN_JITTER_RADIUS, 0.0, (rand::random::<f32>()-0.5)*2.0*SPAWN_JITTER_RADIUS);
        pos += jitter;
        bots.states.insert(id, BotState { pos, yaw: 0.0, hp: 100, alive: true, vy: 0.0, grounded: true });
        let ent = commands.spawn((
            TransformBundle::from_transform(Transform::from_translation(pos)),
            Collider::capsule_y(0.6, 0.3),
            KinematicCharacterController::default(),
        )).id();
        bot_ents.0.insert(id, ent);
        ents.0.insert(id, ent); // レイ判定用に共通Mapにも入れておく
        weapons.0.insert(id, WeaponStatus { ammo: MAG_SIZE, cooldown: 0.0, reload: 0.0 });
        // 保護
        protect.0.insert(id, PROTECT_SEC);
        // Spawnイベント（Bot）
        let ev = ServerMessage::Event(EventMsg::Spawn { id, pos: [pos.x, pos.y, pos.z], kind: ActorKind::Bot });
        if let Ok(bytes) = bincode::serialize(&ev) {
            for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
        }
        info!("server: spawned bot id={} at ({:.2},{:.2},{:.2})", id, pos.x, pos.y, pos.z);
    }
}

fn recv_inputs(mut server: ResMut<RenetServer>, mut last: ResMut<LastInputs>) {
    for client_id in server.clients_id().iter().copied().collect::<Vec<ClientId>>() {
        while let Some(raw) = server.receive_message(client_id, CH_INPUT) {
            if let Ok(msg) = bincode::deserialize::<ClientMessage>(&raw) {
                let ClientMessage::Input(frame) = msg;
                last.0.insert(client_id.raw(), frame);
            }
        }
    }
}

// Pre-physics movement using KCC
fn srv_kcc_move(
    time_fixed: Res<Time<Fixed>>,
    mut players: ResMut<Players>,
    ents: Res<ServerEntities>,
    last: Res<LastInputs>,
    mut q: Query<&mut KinematicCharacterController>,
    ready: Res<MapReady>,
    mut jumps: ResMut<JumpCounts>,
) {
    if !ready.0 { return; }
    let dt = time_fixed.delta_seconds();
    for (id, state) in players.states.iter_mut() {
        if !state.alive { continue; }
        let Some(inp) = last.0.get(id) else { continue };
        let Some(&entity) = ents.0.get(id) else { continue };
        if let Ok(mut kcc) = q.get_mut(entity) {
            let input = Vec3::new(inp.mv[0], 0.0, inp.mv[1]);
            let mut horiz = Vec3::ZERO;
            if input.length_squared() > 1e-6 {
                let yaw_rot = Quat::from_rotation_y(inp.yaw);
                horiz = (yaw_rot * input).normalize();
            }
            let mut speed = 6.0;
            if inp.run { speed *= 1.7; }
            let mut vy = state.vy - 9.81 * dt;
            if inp.jump {
                let used = jumps.0.entry(*id).or_insert(0);
                if state.grounded || *used < 1 {
                    vy = 5.2;
                    if !state.grounded { *used = used.saturating_add(1); }
                }
            }
            let motion = horiz * speed * dt + Vec3::Y * vy * dt;
            kcc.translation = Some(motion);
            state.vy = vy;
            state.yaw = inp.yaw;
        }
    }
}

fn bot_kcc_move(
    time_fixed: Res<Time<Fixed>>,
    mut bots: ResMut<Bots>,
    bot_ents: Res<BotEntities>,
    players: Res<Players>,
    mut q: Query<&mut KinematicCharacterController>,
    ready: Res<MapReady>,
) {
    if !ready.0 { return; }
    let dt = time_fixed.delta_seconds();
    for (id, state) in bots.states.iter_mut() {
        if !state.alive { continue; }
        let Some(&entity) = bot_ents.0.get(id) else { continue };
        // find nearest player
        let mut target_dir = Vec3::ZERO;
        let mut best_d2 = f32::INFINITY;
        for (_pid, p) in players.states.iter() {
            if !p.alive { continue; }
            let d2 = p.pos.distance_squared(state.pos);
            if d2 < best_d2 { best_d2 = d2; target_dir = (p.pos - state.pos).with_y(0.0); }
        }
        if target_dir.length_squared() > 1e-6 {
            let dir = target_dir.normalize();
            let desired_yaw = dir.z.atan2(dir.x) + std::f32::consts::FRAC_PI_2;
            // 正しい角度差でスムーズに向き直る
            let mut delta = (desired_yaw - state.yaw + std::f32::consts::PI).rem_euclid(2.0*std::f32::consts::PI) - std::f32::consts::PI;
            delta = delta.clamp(-BOT_TURN_RATE*dt, BOT_TURN_RATE*dt);
            state.yaw += delta;
            if let Ok(mut kcc) = q.get_mut(entity) {
                let vy = state.vy - 9.81 * dt;
                kcc.translation = Some(dir * BOT_MOVE_SPEED * dt + Vec3::Y * vy * dt);
                state.vy = vy;
            }
        } else if let Ok(mut kcc) = q.get_mut(entity) {
            let vy = state.vy - 9.81 * dt;
            kcc.translation = Some(Vec3::Y * vy * dt);
            state.vy = vy;
        }
    }
}

// Post-physics: update states from transforms/outputs
fn srv_kcc_post(
    mut players: ResMut<Players>,
    ents: Res<ServerEntities>,
    q: Query<(&GlobalTransform, Option<&KinematicCharacterControllerOutput>)>,
    mut jumps: ResMut<JumpCounts>,
) {
    for (id, state) in players.states.iter_mut() {
        let Some(&entity) = ents.0.get(id) else { continue };
        if let Ok((gt, out)) = q.get(entity) {
            state.pos = gt.translation();
            if let Some(o) = out {
                state.grounded = o.grounded;
                if o.grounded && state.vy <= 0.0 {
                    state.vy = 0.0;
                    if let Some(j) = jumps.0.get_mut(id) { *j = 0; }
                }
            }
        }
    }
}

fn bot_kcc_post(
    mut bots: ResMut<Bots>,
    bot_ents: Res<BotEntities>,
    q: Query<(&GlobalTransform, Option<&KinematicCharacterControllerOutput>)>,
) {
    for (id, state) in bots.states.iter_mut() {
        let Some(&entity) = bot_ents.0.get(id) else { continue };
        if let Ok((gt, out)) = q.get(entity) {
            state.pos = gt.translation();
            if let Some(o) = out {
                state.grounded = o.grounded;
                if o.grounded && state.vy <= 0.0 { state.vy = 0.0; }
            }
        }
    }
}

fn bot_ai_shoot_and_respawn(
    mut commands: Commands,
    time_fixed: Res<Time<Fixed>>,
    mut players: ResMut<Players>,
    mut bots: ResMut<Bots>,
    mut weapons: ResMut<Weapons>,
    mut server: ResMut<RenetServer>,
    rapier: Res<RapierContext>,
    ents: Res<ServerEntities>,
    mut scores: ResMut<Scores>,
    mut respawns_players: ResMut<RespawnTimers>,
    mut respawns_bots: ResMut<BotRespawnTimers>,
    spawns: Res<SpawnPoints>,
    bot_ents: Res<BotEntities>,
    mut protect: ResMut<ProtectTimers>,
    mut focus: ResMut<BotFocus>,
) {
    let dt = time_fixed.delta_seconds();
    // 射撃（Bot→人間のみ、FFなし）
    for (id, b) in bots.states.iter() {
        if !b.alive { continue; }
        let w = weapons.0.entry(*id).or_insert(WeaponStatus { ammo: MAG_SIZE, cooldown: 0.0, reload: 0.0 });
        if w.reload > 0.0 || w.cooldown > 0.0 { continue; }
        // ボット自身が保護中は発砲不可
        if protect.0.get(id).copied().unwrap_or(0.0) > 0.0 { continue; }
        if w.ammo == 0 { w.reload = RELOAD_TIME; continue; }
        // 索敵
        let origin = b.pos + Vec3::new(0.0, 0.7, 0.0);
        let forward = Quat::from_rotation_y(b.yaw) * Vec3::NEG_Z;
        let range = BOT_FIRE_RANGE;
        let mut best: Option<(u64, f32)> = None;
        for (pid, p) in players.states.iter() {
            if !p.alive { continue; }
            let to = (p.pos + Vec3::new(0.0,0.7,0.0)) - origin;
            let dist = to.length();
            if dist > range { continue; }
            let cos = forward.normalize().dot(to.normalize());
            if cos < BOT_FOV_COS { continue; }
            // 視野内の前方で最も近いターゲットを選択（横ずれ判定は行わず、遮蔽は後段のレイ判定で）
            let t = to.dot(forward).clamp(0.0, range);
            if best.map_or(true, |(_,bt)| t < bt) { best = Some((*pid, t)); }
        }
        if let Some((hit_id, t_hit)) = best {
            // 目標方向へ直接狙う（yawに依存しない）
            let target_eye = if let Some(p) = players.states.get(&hit_id) { p.pos + Vec3::new(0.0,0.7,0.0) } else { continue };
            let to = target_eye - origin;
            let dist = to.length().max(0.001);
            let aim_dir = (to / dist).normalize();
            // 反応時間: 同じターゲットに一定時間フォーカスしてから射撃
            let entry = focus.0.entry(*id).or_insert((None, 0.0));
            if entry.0 == Some(hit_id) { entry.1 += dt; } else { *entry = (Some(hit_id), 0.0); }
            if entry.1 < BOT_REACT_SEC { continue; }
            // 保護中の対象は無効
            if protect.0.get(&hit_id).copied().unwrap_or(0.0) > 0.0 { continue; }
            // Fire event（Bot）: 衝突点をレイで取得
            let mut filter_fire = QueryFilter::default();
            if let Some(&self_ent) = ents.0.get(id) { filter_fire = filter_fire.exclude_collider(self_ent); }
            let hit_opt = if let Some((_hit_ent, toi)) = rapier.cast_ray(origin, aim_dir, dist, true, filter_fire) {
                Some([origin.x + aim_dir.x * toi, origin.y + aim_dir.y * toi, origin.z + aim_dir.z * toi])
            } else { None };
            if let Ok(bytes) = bincode::serialize(&ServerMessage::Event(EventMsg::Fire { id: *id, origin: [origin.x, origin.y, origin.z], dir: [aim_dir.x, aim_dir.y, aim_dir.z], hit: hit_opt })) {
                for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
            }
            // 遮蔽レイ判定（自身は除外）
            let mut filter = QueryFilter::default();
            if let Some(&self_ent) = ents.0.get(id) { filter = filter.exclude_collider(self_ent); }
            if let Some((hit_ent, _)) = rapier.cast_ray(origin, aim_dir, dist, true, filter) {
                let target_ent_h = ents.0.get(&hit_id).copied();
                let target_ent_b = bot_ents.0.get(&hit_id).copied();
                if Some(hit_ent) != target_ent_h && Some(hit_ent) != target_ent_b { continue; }
            }
            if let Some(hit) = players.states.get(&hit_id) {
                // ダメージ適用（読み取り→書き込みのためクローンIDで再参照）
                drop(hit);
                if let Some(hitm) = players.states.get_mut(&hit_id) {
                    let dmg = BOT_DMG;
                    if hitm.alive {
                        hitm.hp = hitm.hp.saturating_sub(dmg);
                        let ev = ServerMessage::Event(EventMsg::Hit { target_id: hit_id, new_hp: hitm.hp, by: *id });
                        if let Ok(bytes) = bincode::serialize(&ev) { for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); } }
                        if hitm.hp == 0 {
                            let mut_dead = players.states.get_mut(&hit_id).unwrap();
                            mut_dead.alive = false;
                            let ev = ServerMessage::Event(EventMsg::Death { target_id: hit_id, by: *id });
                            if let Ok(bytes) = bincode::serialize(&ev) { for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); } }
                            respawns_players.0.insert(hit_id, 2.0);
                            // スコアは人間のみ集計（Botのキルは加算しないがデスは加算）
                            let e2 = scores.0.entry(hit_id).or_insert((0,0)); e2.1 = e2.1.saturating_add(1);
                        }
                    }
                }
                // 射撃消費
                w.ammo = w.ammo.saturating_sub(1);
                w.cooldown = FIRE_COOLDOWN;
            }
            // 弾消費とクールダウン（Bot用）
            w.ammo = w.ammo.saturating_sub(1);
            w.cooldown = BOT_FIRE_COOLDOWN;
        }
    }
    // Botリスポーン
    let mut to_spawn = Vec::new();
    for (bid, t) in respawns_bots.0.iter_mut() { *t -= dt; if *t <= 0.0 { to_spawn.push(*bid); } }
    for bid in to_spawn {
        respawns_bots.0.remove(&bid);
        if let Some(b) = bots.states.get_mut(&bid) {
            let mut spawn = if !spawns.0.is_empty() { spawns.0[rand::random::<usize>() % spawns.0.len()] } else { Vec3::new(0.0, 10.0, 5.0) };
            // ジッターで分散
            let jitter = Vec3::new((rand::random::<f32>()-0.5)*2.0*SPAWN_JITTER_RADIUS, 0.0, (rand::random::<f32>()-0.5)*2.0*SPAWN_JITTER_RADIUS);
            spawn += jitter;
            b.alive = true; b.hp = 100; b.pos = spawn; b.vy = 0.0; b.grounded = true;
            if let Some(&e) = bot_ents.0.get(&bid) {
                commands.entity(e).insert(TransformBundle::from_transform(Transform::from_translation(spawn)));
            }
            let ev = ServerMessage::Event(EventMsg::Spawn { id: bid, pos: [spawn.x, spawn.y, spawn.z], kind: ActorKind::Bot });
            if let Ok(bytes) = bincode::serialize(&ev) { for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); } }
            // 武器リセット
            let w = weapons.0.entry(bid).or_insert(WeaponStatus::default());
            *w = WeaponStatus { ammo: MAG_SIZE, cooldown: 0.0, reload: 0.0 };
            // 保護付与
            protect.0.insert(bid, PROTECT_SEC);
        }
    }
}

fn srv_shoot_and_respawn(
    mut commands: Commands,
    time_fixed: Res<Time<Fixed>>,
    mut players: ResMut<Players>,
    mut bots: ResMut<Bots>,
    last: Res<LastInputs>,
    mut last_fire: ResMut<LastFireSeq>,
    mut respawns: ResMut<RespawnTimers>,
    mut bot_respawns: ResMut<BotRespawnTimers>,
    mut server: ResMut<RenetServer>,
    rapier: Res<RapierContext>,
    ents: Res<ServerEntities>,
    bot_ents: Res<BotEntities>,
    mut scores: ResMut<Scores>,
    round: Res<RoundState>,
    spawns: Res<SpawnPoints>,
    mut wpnprot: WpnProt,
) {
    if round.phase != RoundPhase::Active { return; }
    let dt = time_fixed.delta_seconds();
    // tick weapon timers
    for (id, w) in wpnprot.weapons.0.iter_mut() {
        if w.cooldown > 0.0 { w.cooldown = (w.cooldown - dt).max(0.0); }
        if w.reload > 0.0 {
            w.reload = (w.reload - dt).max(0.0);
            if w.reload == 0.0 {
                w.ammo = MAG_SIZE;
                // notify reload complete
                if let Ok(bytes) = bincode::serialize(&ServerMessage::Event(EventMsg::Ammo { id: *id, ammo: w.ammo, reloading: false })) {
                    for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                }
            }
        }
    }
    // immutable snapshot of states for safe iteration (humans + bots)
    let mut snap: Vec<(u64, Vec3, bool)> = players
        .states
        .iter()
        .map(|(id, s)| (*id, s.pos, s.alive))
        .collect();
    snap.extend(bots.states.iter().map(|(id, s)| (*id, s.pos, s.alive)));

    for (id, pos, alive) in snap.iter().copied() {
        let Some(inp) = last.0.get(&id) else { continue };
        let w = wpnprot.weapons.0.entry(id).or_insert(WeaponStatus { ammo: MAG_SIZE, cooldown: 0.0, reload: 0.0 });
        let last_seq = last_fire.0.entry(id).or_insert(0);
        if inp.fire && inp.seq != *last_seq && alive {
            *last_seq = inp.seq;
            // Can fire?
            if w.reload > 0.0 || w.cooldown > 0.0 { continue; }
            // 保護中は発砲不可
            if wpnprot.protect.0.get(&id).copied().unwrap_or(0.0) > 0.0 { continue; }
            if w.ammo == 0 {
                // start reload
                if w.reload <= 0.0 { w.reload = RELOAD_TIME; }
                if let Ok(bytes) = bincode::serialize(&ServerMessage::Event(EventMsg::Ammo { id, ammo: w.ammo, reloading: true })) {
                    for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                }
                continue;
            }
            // consume ammo and set cooldown
            w.ammo = w.ammo.saturating_sub(1);
            w.cooldown = FIRE_COOLDOWN;
            if let Ok(bytes) = bincode::serialize(&ServerMessage::Event(EventMsg::Ammo { id, ammo: w.ammo, reloading: false })) {
                for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
            }
            let yaw_rot = Quat::from_rotation_y(inp.yaw);
            let pitch_rot = Quat::from_rotation_x(inp.pitch);
            let forward = yaw_rot * pitch_rot * Vec3::NEG_Z;
            let origin = pos + Vec3::new(0.0, 0.7, 0.0);
            let range = 100.0f32;
            // Send Fire event (with first hit point if any)
            let mut filter_fire = QueryFilter::default();
            if let Some(&self_ent) = ents.0.get(&id) { filter_fire = filter_fire.exclude_collider(self_ent); }
            let hit_opt = if let Some((_hit_ent, toi)) = rapier.cast_ray(origin, forward, range, true, filter_fire) {
                Some([origin.x + forward.x * toi, origin.y + forward.y * toi, origin.z + forward.z * toi])
            } else { None };
            if let Ok(bytes) = bincode::serialize(&ServerMessage::Event(EventMsg::Fire { id, origin: [origin.x, origin.y, origin.z], dir: [forward.x, forward.y, forward.z], hit: hit_opt })) {
                for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
            }
            let mut best: Option<(u64, f32)> = None;
            for (oid, opos, oalive) in snap.iter().copied() {
                if oid == id || !oalive { continue; }
                let target = opos + Vec3::new(0.0, 0.7, 0.0);
                let t = (target - origin).dot(forward).clamp(0.0, range);
                let closest = origin + forward * t;
                let dist2 = (target - closest).length_squared();
                if dist2 <= 0.35f32 * 0.35f32 {
                    if best.map_or(true, |(_, bt)| t < bt) { best = Some((oid, t)); }
                }
            }
            if let Some((hit_id, t_hit)) = best {
                // 保護中の対象は無効
                if wpnprot.protect.0.get(&hit_id).copied().unwrap_or(0.0) > 0.0 { continue; }
                // 射線上の障害物チェック（自分自身のコライダーは除外）
                let mut filter = QueryFilter::default();
                if let Some(&self_ent) = ents.0.get(&id) { filter = filter.exclude_collider(self_ent); }
                if players.states.contains_key(&hit_id) { if let Some((hit_ent, _toi)) = rapier.cast_ray(origin, forward, t_hit, true, filter) {
                    // もし最初に当たったのが狙っているプレイヤー本人なら遮蔽なしとみなす
                    let target_ent = ents.0.get(&hit_id).copied();
                    let target_ent_bot = bot_ents.0.get(&hit_id).copied();
                    if Some(hit_ent) != target_ent && Some(hit_ent) != target_ent_bot { continue; }
                } }
                if let Some(hit) = players.states.get_mut(&hit_id) {
                    if hit.alive {
                        let dmg = 35u16;
                        hit.hp = hit.hp.saturating_sub(dmg);
                        let ev = ServerMessage::Event(EventMsg::Hit { target_id: hit_id, new_hp: hit.hp, by: id });
                        let bytes = bincode::serialize(&ev).unwrap();
                        for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                        if hit.hp == 0 {
                            hit.alive = false;
                            let ev = ServerMessage::Event(EventMsg::Death { target_id: hit_id, by: id });
                            let bytes = bincode::serialize(&ev).unwrap();
                            for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                            respawns.0.insert(hit_id, 2.0);
                            // update scores and broadcast（人間のみスコア集計）
                            if players.states.contains_key(&id) { let e = scores.0.entry(id).or_insert((0,0)); e.0 = e.0.saturating_add(1); }
                            if players.states.contains_key(&hit_id) { let e2 = scores.0.entry(hit_id).or_insert((0,0)); e2.1 = e2.1.saturating_add(1); }
                            let table: Vec<ScoreEntry> = scores.0.iter().map(|(id,(k,d))| ScoreEntry{ id:*id, kills:*k as u32, deaths:*d as u32}).collect();
                            if let Ok(bytes) = bincode::serialize(&ServerMessage::Score(table)) {
                                for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                            }
                            // auto reload on kill if empty and not already reloading
                            let ww = wpnprot.weapons.0.entry(id).or_insert(WeaponStatus { ammo: MAG_SIZE, cooldown: 0.0, reload: 0.0 });
                            if ww.ammo == 0 && ww.reload <= 0.0 {
                                ww.reload = RELOAD_TIME;
                                if let Ok(bytes) = bincode::serialize(&ServerMessage::Event(EventMsg::Ammo { id, ammo: ww.ammo, reloading: true })) {
                                    for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                                }
                            }
                        }
                    }
                } else if let Some(hit) = bots.states.get_mut(&hit_id) {
                    if hit.alive {
                        let dmg = 35u16;
                        hit.hp = hit.hp.saturating_sub(dmg);
                        let ev = ServerMessage::Event(EventMsg::Hit { target_id: hit_id, new_hp: hit.hp, by: id });
                        let bytes = bincode::serialize(&ev).unwrap();
                        for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                        if hit.hp == 0 {
                            hit.alive = false;
                            let ev = ServerMessage::Event(EventMsg::Death { target_id: hit_id, by: id });
                            let bytes = bincode::serialize(&ev).unwrap();
                            for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                            bot_respawns.0.insert(hit_id, 2.0);
                        }
                    }
                }
            }
        }
    }
    // 保護タイマー更新
    for t in wpnprot.protect.0.values_mut() { if *t > 0.0 { *t = (*t - dt).max(0.0); } }
    // respawn countdown
    let mut to_spawn = Vec::new();
    for (pid, t) in respawns.0.iter_mut() {
        *t -= dt;
        if *t <= 0.0 { to_spawn.push(*pid); }
    }
    for pid in to_spawn {
        respawns.0.remove(&pid);
        let spawn = choose_spawn_point(&spawns, &players);
        if let Some(p) = players.states.get_mut(&pid) {
            p.alive = true;
            p.hp = 100;
            p.pos = spawn;
            p.vy = 0.0;
            p.grounded = true;
            if let Some(&e) = ents.0.get(&pid) {
                commands.entity(e).insert(TransformBundle::from_transform(Transform::from_translation(spawn)));
            }
            // リスポーン保護
            wpnprot.protect.0.insert(pid, PROTECT_SEC);
            let ev = ServerMessage::Event(EventMsg::Spawn { id: pid, pos: [p.pos.x, p.pos.y, p.pos.z], kind: ActorKind::Human });
            let bytes = bincode::serialize(&ev).unwrap();
            for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
        }
        // reset weapon
        let w = wpnprot.weapons.0.entry(pid).or_insert(WeaponStatus::default());
        *w = WeaponStatus { ammo: MAG_SIZE, cooldown: 0.0, reload: 0.0 };
        if let Ok(bytes) = bincode::serialize(&ServerMessage::Event(EventMsg::Ammo { id: pid, ammo: MAG_SIZE, reloading: false })) {
            for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
        }
    }
}

// Build static colliders for any meshes loaded from the map scene
fn add_mesh_colliders_for_map(
    mut commands: Commands,
    meshes: Res<Assets<Mesh>>,
    mut ready: ResMut<MapReady>,
    q: Query<(Entity, &Handle<Mesh>), (Added<Handle<Mesh>>, Without<Collider>)>,
) {
    let mut any_inserted = false;
    for (e, h) in &q {
        if let Some(mesh) = meshes.get(h) {
            if let Some(collider) = Collider::from_bevy_mesh(mesh, &ComputedColliderShape::TriMesh) {
                commands.entity(e).insert((collider, RigidBody::Fixed));
                any_inserted = true;
            }
        }
    }
    if any_inserted && !ready.0 {
        ready.0 = true;
        info!("Map colliders ready (server)");
    }
}

// Fallback: ensure Players map stays in sync with current connections.
// This covers cases where ServerEvent is not observed in this schedule ordering.
fn sync_players_with_connections(
    mut commands: Commands,
    mut server: ResMut<RenetServer>,
    mut players: ResMut<Players>,
    mut ents: ResMut<ServerEntities>,
    mut scores: ResMut<Scores>,
    spawns: Res<SpawnPoints>,
    mut weapons: ResMut<Weapons>,
) {
    use std::collections::HashSet;
    let current: HashSet<u64> = server.clients_id().iter().map(|c| c.raw()).collect();

    // Add missing players for newly connected clients
    for id in current.iter().copied() {
        if !players.states.contains_key(&id) {
            let spawn = choose_spawn_point(&spawns, &players);
            players.states.insert(id, PlayerState { pos: spawn, yaw: 0.0, hp: 100, alive: true, vy: 0.0, grounded: true });
            let ent = commands.spawn((
                TransformBundle::from_transform(Transform::from_translation(spawn)),
                Collider::capsule_y(0.6, 0.3),
                KinematicCharacterController::default(),
            )).id();
            ents.0.insert(id, ent);
            info!("server: sync add player {} (total={})", id, players.states.len());
            let ev = ServerMessage::Event(EventMsg::Spawn { id, pos: [spawn.x, spawn.y, spawn.z], kind: ActorKind::Human });
            let bytes = bincode::serialize(&ev).unwrap();
            for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
            scores.0.entry(id).or_insert((0,0));
            // init weapon and notify
            let w = WeaponStatus { ammo: MAG_SIZE, cooldown: 0.0, reload: 0.0 };
            weapons.0.insert(id, w);
            if let Ok(bytes) = bincode::serialize(&ServerMessage::Event(EventMsg::Ammo { id, ammo: MAG_SIZE, reloading: false })) {
                for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
            }
            let table: Vec<ScoreEntry> = scores.0.iter().map(|(id,(k,d))| ScoreEntry{ id:*id, kills:*k as u32, deaths:*d as u32}).collect();
            if let Ok(bytes) = bincode::serialize(&ServerMessage::Score(table)) {
                for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
            }
        }
    }

    // Remove players for disconnected clients
    let known: Vec<u64> = players.states.keys().copied().collect();
    for id in known {
        if !current.contains(&id) {
            players.states.remove(&id);
            if let Some(e) = ents.0.remove(&id) { commands.entity(e).despawn_recursive(); }
            info!("server: sync remove player {} (total={})", id, players.states.len());
            let ev = ServerMessage::Event(EventMsg::Despawn { id });
            let bytes = bincode::serialize(&ev).unwrap();
            for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
            scores.0.remove(&id);
        }
    }
}

fn collect_spawn_points_from_map(
    mut spawns: ResMut<SpawnPoints>,
    q: Query<(&GlobalTransform, Option<&Name>), Added<GlobalTransform>>,
) {
    let mut added = 0;
    for (gt, name) in &q {
        if let Some(n) = name {
            let s = n.as_str();
            if s.starts_with("spawn") || s.starts_with("Spawn") || s.starts_with("SPAWN") || s.starts_with("spawn_") {
                let p = gt.translation();
                spawns.0.push(p);
                added += 1;
            }
        }
    }
    if added > 0 {
        info!("Map spawn points collected: +{} (total={})", added, spawns.0.len());
    }
}

fn choose_spawn_point(spawns: &SpawnPoints, players: &Players) -> Vec3 {
    // 環境変数でスポーン点機能を一時無効化（デバッグ用）
    if matches!(env::var("USE_SPAWN_POINTS").ok().as_deref(), Some("0" | "false" | "False")) {
        return Vec3::new(0.0, 10.0, 5.0);
    }
    if spawns.0.is_empty() {
        return Vec3::new(0.0, 10.0, 5.0);
    }
    let mut best_pos = spawns.0[0];
    let mut best_score = f32::MIN;
    for &p in &spawns.0 {
        let mut mind = f32::INFINITY;
        for (_id, s) in players.states.iter() {
            if s.alive {
                let d = s.pos.distance(p);
                if d < mind { mind = d; }
            }
        }
        if mind > best_score {
            best_score = mind;
            best_pos = p;
        }
    }
    best_pos
}

fn round_update(
    time_fixed: Res<Time<Fixed>>,
    mut round: ResMut<RoundState>,
    mut scores: ResMut<Scores>,
    mut players: ResMut<Players>,
    mut server: ResMut<RenetServer>,
    mut respawns: ResMut<RespawnTimers>,
    spawns: Res<SpawnPoints>,
    mut jumps: ResMut<JumpCounts>,
) {
    let dt = time_fixed.delta_seconds();
    match round.phase {
        RoundPhase::Active => {
            round.time_left -= dt;
            // 勝利条件チェック
            let mut winner: Option<u64> = None;
            for (id, (k, _d)) in scores.0.iter() {
                if *k >= WIN_KILLS { winner = Some(*id); break; }
            }
            if winner.is_some() || round.time_left <= 0.0 {
                // 終了を通知
                let ev = ServerMessage::Event(EventMsg::RoundEnd { winner_id: winner, next_in_sec: ROUND_END_DELAY_SEC as u32 });
                if let Ok(bytes) = bincode::serialize(&ev) {
                    for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                }
                round.phase = RoundPhase::Ending;
                round.end_timer = ROUND_END_DELAY_SEC;
            }
        }
        RoundPhase::Ending => {
            round.end_timer -= dt;
            if round.end_timer <= 0.0 {
                // リセット: スコア、プレイヤー状態、リスポーン
                // scores は accept/sync で再周知されるため、ここでクリア
                // ただし現状の実装ではスコア配布のために空配列を通知
                // プレイヤーを全員リスポーン
                let ids: Vec<u64> = players.states.keys().copied().collect();
                for id in ids {
                    let spawn = choose_spawn_point(&spawns, &players);
                    if let Some(state) = players.states.get_mut(&id) {
                        state.alive = true;
                        state.hp = 100;
                        state.pos = spawn;
                        state.vy = 0.0;
                        state.grounded = true;
                        if let Some(j) = jumps.0.get_mut(&id) { *j = 0; }
                        // 送信
                        let ev = ServerMessage::Event(EventMsg::Spawn { id, pos: [state.pos.x, state.pos.y, state.pos.z], kind: ActorKind::Human });
                        if let Ok(bytes) = bincode::serialize(&ev) {
                            for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                        }
                    }
                }
                respawns.0.clear();
                // スコアをゼロクリア
                // 既存のキーを維持して0にする
                for (_id, kd) in scores.0.iter_mut() { *kd = (0,0); }
                let table: Vec<ScoreEntry> = scores.0.iter().map(|(id,(k,d))| ScoreEntry{ id:*id, kills:*k as u32, deaths:*d as u32}).collect();
                if let Ok(bytes) = bincode::serialize(&ServerMessage::Score(table)) {
                    for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                }
                // ラウンド開始通知
                round.phase = RoundPhase::Active;
                round.time_left = ROUND_TIME_SEC;
                let ev = ServerMessage::Event(EventMsg::RoundStart { time_left_sec: round.time_left as u32 });
                if let Ok(bytes) = bincode::serialize(&ev) {
                    for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                }
            }
        }
    }
}

#[derive(Resource)]
struct SnapshotTimer(Timer);

#[derive(Resource)]
struct ServerLogTimer(Timer);

fn broadcast_snapshots(
    time_fixed: Res<Time<Fixed>>,
    mut timer: ResMut<SnapshotTimer>,
    mut server: ResMut<RenetServer>,
    players: Res<Players>,
    bots: Res<Bots>,
) {
    timer.0.tick(time_fixed.delta());
    if !timer.0.finished() { return; }
    let mut players_vec: Vec<PlayerStateMsg> = players
        .states
        .iter()
        .map(|(id, s)| PlayerStateMsg { id: *id, pos: [s.pos.x, s.pos.y, s.pos.z], yaw: s.yaw, alive: s.alive, hp: s.hp, kind: ActorKind::Human })
        .collect();
    players_vec.extend(bots.states.iter().map(|(id, s)| PlayerStateMsg { id: *id, pos: [s.pos.x, s.pos.y, s.pos.z], yaw: s.yaw, alive: s.alive, hp: s.hp, kind: ActorKind::Bot }));
    if matches!(std::env::var("NET_SNAPSHOT_LOG").ok(), Some(_)) {
        info!("server: snapshot actors={}", players_vec.len());
    }
    let snap = SnapshotMsg { tick: 0, players: players_vec };
    let bytes = bincode::serialize(&ServerMessage::Snapshot(snap)).unwrap();
    for client_id in server.clients_id() {
        let _ = server.send_message(client_id, CH_SNAPSHOT, bytes.clone());
    }
}

fn log_clients_count(
    time: Res<Time>,
    mut timer: ResMut<ServerLogTimer>,
    server: Res<RenetServer>,
) {
    timer.0.tick(time.delta());
    if timer.0.finished() {
        let n = server.clients_id().len();
        info!("server: clients={} ", n);
    }
}
