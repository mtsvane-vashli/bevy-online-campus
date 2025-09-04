// #![windows_subsystem = "windows"]

use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;
use bevy::window::CursorGrabMode;
use bevy_rapier3d::prelude::*;
use bevy_renet::RenetClientPlugin;
use bevy_renet::transport::NetcodeClientPlugin;
use bevy_renet::renet::RenetClient;

#[path = "net.rs"]
mod net;
use net::*;

// ===== Config =====
const MAP_SCENE_PATH: &str = "maps/map.glb#Scene0"; // assets 配下に maps/map.glb を置いてください
const PLAYER_START: Vec3 = Vec3::new(0.0, 10.0, 5.0);
const MOVE_SPEED: f32 = 6.0; // m/s
const RUN_MULTIPLIER: f32 = 1.7;
const MOUSE_SENSITIVITY: f32 = 0.0018; // rad/pixel
const BULLET_SPEED: f32 = 40.0; // m/s
const BULLET_LIFETIME: f32 = 2.0; // sec
const GRAVITY: f32 = 9.81; // m/s^2
const JUMP_SPEED: f32 = 5.2; // m/s (必要なら調整)
const KEY_LOOK_SPEED: f32 = 2.2; // rad/s for arrow-key look

#[derive(Component)]
struct Player;

#[derive(Component)]
struct PlayerCamera {
    yaw: f32,
    pitch: f32,
}

#[derive(Component)]
struct Bullet {
    dir: Vec3,
    speed: f32,
    life: Timer,
}

#[derive(Resource, Default)]
struct CursorLocked(pub bool);

#[derive(Component, Default)]
struct Controller {
    vy: f32,
    on_ground: bool,
}

#[derive(Resource, Default)]
struct MapReady(pub bool);

fn main() {
    App::new()
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.03)))
        .insert_resource(AmbientLight { color: Color::WHITE, brightness: 300.0 })
        .insert_resource(CursorLocked(true))
        .insert_resource(MapReady(false))
        .insert_resource(ConnStatePrev::default())
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bevy FPS".into(),
                present_mode: bevy::window::PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }))
        .add_plugins((RenetClientPlugin, NetcodeClientPlugin))
        .add_plugins((
            RapierPhysicsPlugin::<NoUserData>::default(),
            // デバッグ表示が欲しい場合は下を有効化
            // RapierDebugRenderPlugin::default(),
        ))
        .add_systems(Startup, (setup_world, setup_ui, setup_physics, setup_net_client, setup_player))
        .add_systems(Update, (
            cursor_lock_controls,
            mouse_look_system,
            keyboard_look_system,
            kcc_move_system,
            kcc_post_step_system,
            reconcile_self,
            shoot_system,
            bullet_move_and_despawn,
            add_mesh_colliders_for_map,
            net_log_connection,
            net_send_input,
            net_recv_snapshot,
            net_recv_events,
        ))
        .run();
}

fn setup_world(mut commands: Commands, asset_server: Res<AssetServer>) {
    // マップのGLBシーンをロード
    commands.spawn(SceneBundle {
        scene: asset_server.load(MAP_SCENE_PATH),
        transform: Transform::from_xyz(0.0, 0.0, 0.0),
        ..default()
    });

    // 環境光は Resource で設定済み。補助の方向ライトを追加
    commands.spawn((
        DirectionalLightBundle {
            directional_light: DirectionalLight {
                shadows_enabled: true,
                illuminance: 30_000.0,
                ..default()
            },
            transform: Transform::from_xyz(10.0, 12.0, 8.0)
                .looking_at(Vec3::ZERO, Vec3::Y),
            ..default()
        },
    ));
}

fn setup_physics(mut conf: ResMut<RapierConfiguration>) {
    conf.gravity = Vec3::new(0.0, -GRAVITY, 0.0);
}

fn setup_player(mut commands: Commands, mut windows: Query<&mut Window>) {
    // プレイヤー本体（位置と yaw を持つ）
    let player = commands
        .spawn((
            Player,
            SpatialBundle {
                transform: Transform::from_translation(PLAYER_START),
                ..default()
            },
        ))
        // キャラクターコントローラと当たり判定（カプセル）
        .insert((
            Collider::capsule_y(0.6, 0.3), // 全高 ~1.8m（0.6*2 + 0.3*2）
            KinematicCharacterController::default(),
            Controller::default(),
        ))
        .id();

    // カメラはプレイヤーの子: pitch はカメラにのみ反映
    // 目線の高さ（プレイヤー中心から +0.7m）
    let cam = Camera3dBundle {
        transform: Transform::from_xyz(0.0, 0.7, 0.0),
        camera: Camera { hdr: true, ..default() },
        ..default()
    };

    commands.entity(player).with_children(|p| {
        p.spawn((cam, PlayerCamera { yaw: 0.0, pitch: 0.0 }));
    });

    // カーソルをロック
    if let Ok(mut win) = windows.get_single_mut() {
        win.cursor.visible = false;
        win.cursor.grab_mode = CursorGrabMode::Locked;
    }
}

fn setup_ui(mut commands: Commands) {
    // 画面中央に簡易クロスヘア
    commands.spawn(NodeBundle {
        style: Style {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        background_color: BackgroundColor(Color::NONE),
        ..default()
    }).with_children(|parent| {
        parent.spawn(TextBundle::from_section(
            "＋",
            TextStyle {
                font_size: 18.0,
                color: Color::srgb(1.0, 1.0, 1.0),
                ..default()
            },
        ));
    });
}

fn cursor_lock_controls(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut win_q: Query<&mut Window>,
    mut locked: ResMut<CursorLocked>,
) {
    let mut win = if let Ok(w) = win_q.get_single_mut() { w } else { return };

    // Esc で解除、左クリックで再ロック
    if keys.just_pressed(KeyCode::Escape) {
        locked.0 = false;
    }
    if buttons.just_pressed(MouseButton::Left) {
        locked.0 = true;
    }

    match locked.0 {
        true => {
            win.cursor.visible = false;
            win.cursor.grab_mode = CursorGrabMode::Locked;
        }
        false => {
            win.cursor.visible = true;
            win.cursor.grab_mode = CursorGrabMode::None;
        }
    }
}

fn mouse_look_system(
    mut mouse_evr: EventReader<MouseMotion>,
    locked: Res<CursorLocked>,
    mut q: ParamSet<(
        Query<&mut Transform, (With<Player>, Without<Camera3d>)>,
        Query<(&mut Transform, &mut PlayerCamera), With<Camera3d>>,
    )>,
) {
    if !locked.0 {
        mouse_evr.clear();
        return;
    }

    let mut delta = Vec2::ZERO;
    for ev in mouse_evr.read() {
        delta += ev.delta;
    }
    if delta == Vec2::ZERO {
        return;
    }

    // まずカメラ側で yaw/pitch を更新し、必要値をローカルに保持
    let mut new_yaw: f32 = 0.0;
    let mut new_pitch: f32 = 0.0;
    {
        let mut cam_query = q.p1();
        let Ok((mut cam_tf, mut pcam)) = cam_query.get_single_mut() else { return };
        pcam.yaw -= delta.x * MOUSE_SENSITIVITY;
        pcam.pitch = (pcam.pitch - delta.y * MOUSE_SENSITIVITY).clamp(-1.54, 1.54);
        new_yaw = pcam.yaw;
        new_pitch = pcam.pitch;
        cam_tf.rotation = Quat::from_rotation_x(pcam.pitch);
    }

    // 次にプレイヤーの yaw 回転を反映（別スコープで別クエリを借用）
    let mut player_query = q.p0();
    if let Ok(mut player_tf) = player_query.get_single_mut() {
        player_tf.rotation = Quat::from_rotation_y(new_yaw);
    }
}

fn keyboard_look_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut q: ParamSet<(
        Query<&mut Transform, (With<Player>, Without<Camera3d>)>,
        Query<(&mut Transform, &mut PlayerCamera), With<Camera3d>>,
    )>,
) {
    let dt = time.delta_seconds();
    let horiz = (keys.pressed(KeyCode::ArrowRight) as i32 - keys.pressed(KeyCode::ArrowLeft) as i32) as f32;
    let vert = (keys.pressed(KeyCode::ArrowDown) as i32 - keys.pressed(KeyCode::ArrowUp) as i32) as f32;
    if horiz == 0.0 && vert == 0.0 { return; }

    let mut new_yaw = 0.0f32;
    let mut new_pitch = 0.0f32;
    {
        let mut cam_query = q.p1();
        let Ok((mut cam_tf, mut pcam)) = cam_query.get_single_mut() else { return };
        // 矢印キー: 右=右回転（マウスの正方向と同じく yaw を減算）、上=上向き（pitch 減算）
        pcam.yaw -= horiz * KEY_LOOK_SPEED * dt;
        pcam.pitch = (pcam.pitch - vert * KEY_LOOK_SPEED * dt).clamp(-1.54, 1.54);
        new_yaw = pcam.yaw;
        new_pitch = pcam.pitch;
        cam_tf.rotation = Quat::from_rotation_x(new_pitch);
    }
    let mut player_query = q.p0();
    if let Ok(mut player_tf) = player_query.get_single_mut() {
        player_tf.rotation = Quat::from_rotation_y(new_yaw);
    }
}

fn kcc_move_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    cam_q: Query<&PlayerCamera, With<Camera3d>>,
    mut q: Query<(&mut KinematicCharacterController, &mut Controller), With<Player>>,
    ready: Res<MapReady>,
) {
    if !ready.0 { return; }
    let (mut kcc, mut ctrl) = if let Ok(v) = q.get_single_mut() { v } else { return };
    let cam = if let Ok(v) = cam_q.get_single() { v } else { return };

    // 入力（水平面）
    let mut input = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) { input += Vec3::NEG_Z; }
    if keys.pressed(KeyCode::KeyS) { input += Vec3::Z; }
    if keys.pressed(KeyCode::KeyA) { input += Vec3::NEG_X; }
    if keys.pressed(KeyCode::KeyD) { input += Vec3::X; }

    let mut horiz = Vec3::ZERO;
    if input.length_squared() > 1e-6 {
        let yaw_rot = Quat::from_rotation_y(cam.yaw);
        horiz = (yaw_rot * input).normalize();
    }

    // スピード調整
    let mut speed = MOVE_SPEED;
    if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        speed *= RUN_MULTIPLIER;
    }

    // 重力・ジャンプ
    let dt = time.delta_seconds();
    ctrl.vy -= GRAVITY * dt;
    if keys.just_pressed(KeyCode::Space) && ctrl.on_ground {
        ctrl.vy = JUMP_SPEED;
        ctrl.on_ground = false;
    }

    let motion = horiz * speed * dt + Vec3::Y * ctrl.vy * dt;
    kcc.translation = Some(motion);
}

fn kcc_post_step_system(
    mut q: Query<(&mut Controller, Option<&KinematicCharacterControllerOutput>), With<Player>>,
) {
    let (mut ctrl, output) = if let Ok(v) = q.get_single_mut() { v } else { return };
    if let Some(out) = output {
        ctrl.on_ground = out.grounded;
        if out.grounded && ctrl.vy <= 0.0 {
            ctrl.vy = 0.0;
        }
    }
}

fn shoot_system(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    cam_global_q: Query<&GlobalTransform, With<Camera3d>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !(buttons.just_pressed(MouseButton::Left) || keys.just_pressed(KeyCode::KeyF)) {
        return;
    }
    let cam_g = if let Ok(v) = cam_global_q.get_single() { v } else { return };

    let forward: Vec3 = cam_g.forward().into();
    let start = cam_g.translation();

    // 小さな弾体（可視化用）
    let mesh = meshes.add(Sphere::new(0.04).mesh().ico(4).unwrap());
    let mat = materials.add(Color::srgb(1.0, 0.9, 0.2));

    commands.spawn((
        PbrBundle {
            mesh,
            material: mat,
            transform: Transform::from_translation(start),
            ..default()
        },
        Bullet {
            dir: forward,
            speed: BULLET_SPEED,
            life: Timer::from_seconds(BULLET_LIFETIME, TimerMode::Once),
        },
    ));

    // 砲口フラッシュっぽいライト（短命）
    commands.spawn((
        PointLightBundle {
            point_light: PointLight { intensity: 500.0, range: 4.0, ..default() },
            transform: Transform::from_translation(start + forward * 0.1),
            ..default()
        },
        Bullet { dir: Vec3::ZERO, speed: 0.0, life: Timer::from_seconds(0.06, TimerMode::Once) },
    ));
}

fn bullet_move_and_despawn(
    time: Res<Time>,
    mut commands: Commands,
    mut bullets: Query<(Entity, &mut Transform, &mut Bullet)>,
) {
    for (e, mut tf, mut b) in &mut bullets {
        b.life.tick(time.delta());
        if b.life.finished() {
            commands.entity(e).despawn_recursive();
            continue;
        }
        if b.speed > 0.0 {
            tf.translation += b.dir * b.speed * time.delta_seconds();
        }
    }
}

// GLBのメッシュに静的コライダーを自動付与
fn add_mesh_colliders_for_map(
    mut commands: Commands,
    meshes: Res<Assets<Mesh>>,
    mut ready: ResMut<MapReady>,
    q: Query<(Entity, &Handle<Mesh>), (Added<Handle<Mesh>>, Without<Collider>, Without<Bullet>, Without<Player>)>,
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
        info!("Map colliders ready (client)");
    }
}

#[derive(Resource, Default)]
struct ConnStatePrev { connected: bool }

fn net_log_connection(mut prev: ResMut<ConnStatePrev>, mut client: ResMut<RenetClient>) {
    let is_conn = client.is_connected();
    if is_conn != prev.connected {
        if is_conn {
            info!("Client connected to server");
        } else {
            info!("Client not connected; attempting handshake to 127.0.0.1:5000");
        }
        prev.connected = is_conn;
    }
}

// --- Networking (client) ---

#[derive(Component)]
struct RemoteAvatar { id: u64 }

#[derive(Resource, Default)]
struct RemoteMap(std::collections::HashMap<u64, Entity>);

#[derive(Resource)]
struct LocalNetInfo { id: u64 }

#[derive(Resource, Default)]
struct InputSeq(u32);

#[derive(Resource, Default)]
struct AuthoritativeSelf { pos: Option<Vec3>, yaw: Option<f32> }

#[derive(Resource)]
struct LocalHealth { hp: u16 }

fn setup_net_client(mut commands: Commands) {
    let (client, transport, client_id) = new_client(None);
    commands.insert_resource(client);
    commands.insert_resource(transport);
    commands.insert_resource(LocalNetInfo { id: client_id.raw() });
    commands.insert_resource(RemoteMap::default());
    commands.insert_resource(InputSeq::default());
    commands.insert_resource(AuthoritativeSelf::default());
    commands.insert_resource(LocalHealth { hp: 100 });
}

fn net_send_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    cam_q: Query<&PlayerCamera, With<Camera3d>>,
    mut client: ResMut<RenetClient>,
    mut seq: ResMut<InputSeq>,
) {
    let cam = if let Ok(c) = cam_q.get_single() { c } else { return };
    let mut mv = [0.0f32, 0.0f32];
    if keys.pressed(KeyCode::KeyW) { mv[1] -= 1.0; }
    if keys.pressed(KeyCode::KeyS) { mv[1] += 1.0; }
    if keys.pressed(KeyCode::KeyA) { mv[0] -= 1.0; }
    if keys.pressed(KeyCode::KeyD) { mv[0] += 1.0; }
    let run = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let jump = keys.just_pressed(KeyCode::Space);
    let fire = buttons.just_pressed(MouseButton::Left) || keys.just_pressed(KeyCode::KeyF);
    seq.0 = seq.0.wrapping_add(1);
    let frame = InputFrame { seq: seq.0, mv, run, jump, fire, yaw: cam.yaw, pitch: cam.pitch };
    if let Ok(bytes) = bincode::serialize(&ClientMessage::Input(frame)) {
        let _ = client.send_message(CH_INPUT, bytes);
    }
}

fn net_recv_snapshot(
    mut commands: Commands,
    mut client: ResMut<RenetClient>,
    mut remap: ResMut<RemoteMap>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    local: Res<LocalNetInfo>,
    mut self_auth: ResMut<AuthoritativeSelf>,
) {
    while let Some(raw) = client.receive_message(CH_SNAPSHOT) {
        if let Ok(ServerMessage::Snapshot(snap)) = bincode::deserialize::<ServerMessage>(&raw) {
            if snap.players.len() > 0 {
                info!("client: snapshot players={}", snap.players.len());
            }
            for p in snap.players {
                if p.id == local.id {
                    self_auth.pos = Some(Vec3::new(p.pos[0], p.pos[1], p.pos[2]));
                    self_auth.yaw = Some(p.yaw);
                    continue;
                }
                let pos = Vec3::new(p.pos[0], p.pos[1], p.pos[2]);
                if p.alive {
                    if let Some(&ent) = remap.0.get(&p.id) {
                        if let Some(mut ec) = commands.get_entity(ent) {
                            ec.insert(Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(p.yaw)));
                        }
                    } else {
                        let mesh = meshes.add(Cuboid::new(0.4, 1.8, 0.4));
                    let mat = materials.add(Color::srgb(0.2, 0.9, 0.3));
                        let ent = commands.spawn((
                            PbrBundle { mesh, material: mat, transform: Transform::from_translation(pos), ..default() },
                            RemoteAvatar { id: p.id },
                        )).id();
                        remap.0.insert(p.id, ent);
                    }
                } else {
                    if let Some(ent) = remap.0.remove(&p.id) {
                        commands.entity(ent).despawn_recursive();
                    }
                }
            }
        }
    }
}

fn net_recv_events(
    mut commands: Commands,
    mut client: ResMut<RenetClient>,
    mut remap: ResMut<RemoteMap>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    local: Res<LocalNetInfo>,
    mut self_auth: ResMut<AuthoritativeSelf>,
    mut my_hp: ResMut<LocalHealth>,
) {
    while let Some(raw) = client.receive_message(CH_RELIABLE) {
        if let Ok(ServerMessage::Event(ev)) = bincode::deserialize::<ServerMessage>(&raw) {
            match ev {
                EventMsg::Spawn { id, pos } => {
                    let p = Vec3::new(pos[0], pos[1], pos[2]);
                    if id == local.id {
                        self_auth.pos = Some(p);
                        my_hp.hp = 100;
                    } else {
                        if let Some(&ent) = remap.0.get(&id) {
                            if let Some(mut ec) = commands.get_entity(ent) {
                                ec.insert(Transform::from_translation(p));
                            }
                        } else {
                            let mesh = meshes.add(Cuboid::new(0.4, 1.8, 0.4));
                            let mat = materials.add(Color::srgb(0.2, 0.9, 0.3));
                            let ent = commands.spawn((PbrBundle { mesh, material: mat, transform: Transform::from_translation(p), ..default() }, RemoteAvatar { id })).id();
                            remap.0.insert(id, ent);
                        }
                    }
                }
                EventMsg::Despawn { id } => {
                    if let Some(ent) = remap.0.remove(&id) { commands.entity(ent).despawn_recursive(); }
                }
                EventMsg::Hit { target_id, new_hp, by: _ } => {
                    if target_id == local.id { my_hp.hp = new_hp; }
                }
                EventMsg::Death { target_id, by: _ } => {
                    if target_id == local.id { my_hp.hp = 0; }
                    if let Some(ent) = remap.0.remove(&target_id) { commands.entity(ent).despawn_recursive(); }
                }
            }
        }
    }
}

// 自分プレイヤーの補正（簡易リコンシリエーション）
fn reconcile_self(
    time: Res<Time>,
    mut q: Query<&mut Transform, With<Player>>,
    self_auth: Res<AuthoritativeSelf>,
) {
    let mut tf = if let Ok(t) = q.get_single_mut() { t } else { return };
    if let Some(target) = self_auth.pos {
        let diff = target - tf.translation;
        let d = diff.length();
        if d > 0.001 {
            let rate = 10.0; // per second
            let step = (rate * time.delta_seconds()).min(1.0);
            tf.translation += diff * step;
        }
    }
    if let Some(yaw) = self_auth.yaw {
        // 軽い追従のみ（強いワープは避ける）
        let current_yaw = tf.rotation.to_euler(EulerRot::YXZ).0;
        let delta = (yaw - current_yaw).atan2((yaw - current_yaw).cos());
        let step = (6.0 * time.delta_seconds()).min(1.0);
        tf.rotation = Quat::from_rotation_y(current_yaw + delta * step);
    }
}
