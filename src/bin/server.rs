use std::collections::HashMap;
use bevy::prelude::*;
use bevy::winit::WinitPlugin; // headless VPS では無効化する
use bevy::app::ScheduleRunnerPlugin; // Winit を無効化したらループ駆動を自前で
use std::time::Duration;
use bevy::time::Fixed;
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
        .insert_resource(ServerEntities::default())
        .insert_resource(MapReady(false))
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(SnapshotTimer(Timer::from_seconds(1.0/30.0, TimerMode::Repeating)))
        .insert_resource(ServerLogTimer(Timer::from_seconds(1.0, TimerMode::Repeating)))
        .add_systems(Startup, (setup_server, setup_map))
        .add_systems(Update, (accept_clients, add_mesh_colliders_for_map, log_clients_count))
        .add_systems(Update, sync_players_with_connections)
        .add_systems(FixedUpdate, recv_inputs)
        .add_systems(FixedUpdate, srv_kcc_move.before(PhysicsSet::StepSimulation))
        .add_systems(FixedUpdate, srv_kcc_post.after(PhysicsSet::Writeback))
        .add_systems(FixedUpdate, srv_shoot_and_respawn.after(srv_kcc_post))
        .add_systems(FixedUpdate, broadcast_snapshots.after(srv_shoot_and_respawn))
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
) {
    while let Some(event) = server.get_event() {
        match event {
            bevy_renet::renet::ServerEvent::ClientConnected { client_id } => {
                let id = client_id.raw();
                let spawn = Vec3::new(0.0, 10.0, 5.0);
                players.states.insert(id, PlayerState { pos: spawn, yaw: 0.0, hp: 100, alive: true, vy: 0.0, grounded: true });
                let ent = commands.spawn((
                    TransformBundle::from_transform(Transform::from_translation(spawn)),
                    Collider::capsule_y(0.6, 0.3),
                    KinematicCharacterController::default(),
                )).id();
                ents.0.insert(id, ent);
                info!("server: inserted player state for {} (total={})", id, players.states.len());
                // broadcast spawn
                let ev = ServerMessage::Event(EventMsg::Spawn { id, pos: [0.0, 10.0, 5.0] });
                let bytes = bincode::serialize(&ev).unwrap();
                for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                info!("client connected: {}", id);
            }
            bevy_renet::renet::ServerEvent::ClientDisconnected { client_id, reason } => {
                let id = client_id.raw();
                players.states.remove(&id);
                if let Some(e) = ents.0.remove(&id) { commands.entity(e).despawn_recursive(); }
                let ev = ServerMessage::Event(EventMsg::Despawn { id });
                let bytes = bincode::serialize(&ev).unwrap();
                for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
                info!("client disconnected: {} ({:?})", id, reason);
            }
        }
    }
}

const MAP_SCENE_PATH: &str = "maps/map.glb#Scene0";

fn setup_map(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(SceneBundle { scene: asset_server.load(MAP_SCENE_PATH), ..default() });
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
            if inp.jump && state.grounded { vy = 5.2; }
            let motion = horiz * speed * dt + Vec3::Y * vy * dt;
            kcc.translation = Some(motion);
            state.vy = vy;
            state.yaw = inp.yaw;
        }
    }
}

// Post-physics: update states from transforms/outputs
fn srv_kcc_post(
    mut players: ResMut<Players>,
    ents: Res<ServerEntities>,
    q: Query<(&GlobalTransform, Option<&KinematicCharacterControllerOutput>)>,
) {
    for (id, state) in players.states.iter_mut() {
        let Some(&entity) = ents.0.get(id) else { continue };
        if let Ok((gt, out)) = q.get(entity) {
            state.pos = gt.translation();
            if let Some(o) = out {
                state.grounded = o.grounded;
                if o.grounded && state.vy <= 0.0 { state.vy = 0.0; }
            }
        }
    }
}

fn srv_shoot_and_respawn(
    time_fixed: Res<Time<Fixed>>,
    mut players: ResMut<Players>,
    last: Res<LastInputs>,
    mut last_fire: ResMut<LastFireSeq>,
    mut respawns: ResMut<RespawnTimers>,
    mut server: ResMut<RenetServer>,
    rapier: Res<RapierContext>,
    ents: Res<ServerEntities>,
) {
    let dt = time_fixed.delta_seconds();
    // immutable snapshot of states for safe iteration
    let snap: Vec<(u64, Vec3, bool)> = players
        .states
        .iter()
        .map(|(id, s)| (*id, s.pos, s.alive))
        .collect();

    for (id, pos, alive) in snap.iter().copied() {
        let Some(inp) = last.0.get(&id) else { continue };
        let last_seq = last_fire.0.entry(id).or_insert(0);
        if inp.fire && inp.seq != *last_seq && alive {
            *last_seq = inp.seq;
            let yaw_rot = Quat::from_rotation_y(inp.yaw);
            let pitch_rot = Quat::from_rotation_x(inp.pitch);
            let forward = yaw_rot * pitch_rot * Vec3::NEG_Z;
            let origin = pos + Vec3::new(0.0, 0.7, 0.0);
            let range = 100.0f32;
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
                // 射線上の障害物チェック（自分自身のコライダーは除外）
                let mut filter = QueryFilter::default();
                if let Some(&self_ent) = ents.0.get(&id) { filter = filter.exclude_collider(self_ent); }
                if let Some((hit_ent, _toi)) = rapier.cast_ray(origin, forward, t_hit, true, filter) {
                    // もし最初に当たったのが狙っているプレイヤー本人なら遮蔽なしとみなす
                    let target_ent = ents.0.get(&hit_id).copied();
                    if Some(hit_ent) != target_ent { continue; }
                }
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
                        }
                    }
                }
            }
        }
    }
    // respawn countdown
    let mut to_spawn = Vec::new();
    for (pid, t) in respawns.0.iter_mut() {
        *t -= dt;
        if *t <= 0.0 { to_spawn.push(*pid); }
    }
    for pid in to_spawn {
        respawns.0.remove(&pid);
        if let Some(p) = players.states.get_mut(&pid) {
            p.alive = true;
            p.hp = 100;
            p.pos = Vec3::new(0.0, 10.0, 5.0);
            p.vy = 0.0;
            p.grounded = true;
            let ev = ServerMessage::Event(EventMsg::Spawn { id: pid, pos: [p.pos.x, p.pos.y, p.pos.z] });
            let bytes = bincode::serialize(&ev).unwrap();
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
) {
    use std::collections::HashSet;
    let current: HashSet<u64> = server.clients_id().iter().map(|c| c.raw()).collect();

    // Add missing players for newly connected clients
    for id in current.iter().copied() {
        if !players.states.contains_key(&id) {
            let spawn = Vec3::new(0.0, 10.0, 5.0);
            players.states.insert(id, PlayerState { pos: spawn, yaw: 0.0, hp: 100, alive: true, vy: 0.0, grounded: true });
            let ent = commands.spawn((
                TransformBundle::from_transform(Transform::from_translation(spawn)),
                Collider::capsule_y(0.6, 0.3),
                KinematicCharacterController::default(),
            )).id();
            ents.0.insert(id, ent);
            info!("server: sync add player {} (total={})", id, players.states.len());
            let ev = ServerMessage::Event(EventMsg::Spawn { id, pos: [spawn.x, spawn.y, spawn.z] });
            let bytes = bincode::serialize(&ev).unwrap();
            for cid in server.clients_id() { let _ = server.send_message(cid, CH_RELIABLE, bytes.clone()); }
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
) {
    timer.0.tick(time_fixed.delta());
    if !timer.0.finished() { return; }
    let snap = SnapshotMsg {
        tick: 0,
        players: players.states.iter().map(|(id, s)| PlayerStateMsg { id: *id, pos: [s.pos.x, s.pos.y, s.pos.z], yaw: s.yaw, alive: s.alive, hp: s.hp }).collect(),
    };
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
