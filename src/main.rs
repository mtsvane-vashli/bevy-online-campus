// #![windows_subsystem = "windows"]

use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::window::CursorGrabMode;
use bevy::window::WindowFocused;
use bevy_rapier3d::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy_renet::RenetClientPlugin;
use bevy_renet::transport::NetcodeClientPlugin;
use bevy_renet::renet::RenetClient;
use std::f32::consts::PI;
use std::time::Duration;

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
// Position reconciliation thresholds (client-authoritative bias)
const POS_DEADBAND: f32 = 0.03; // meters: ignore tiny diffs (stop jitter)
const POS_SNAP: f32 = 0.8; // meters: snap when far out-of-bounds

#[inline]
fn wrap_pi(a: f32) -> f32 {
    (a + PI).rem_euclid(2.0 * PI) - PI
}

// ===== HUD Components =====
#[derive(Component)]
struct UiHp;

#[derive(Component)]
struct UiFps;

#[derive(Resource)]
struct FpsTextTimer(Timer);

#[derive(Component)]
struct UiHitMarker { timer: Timer }

#[derive(Component)]
struct UiKillLog;

#[derive(Component)]
struct UiKillEntry { timer: Timer }

#[derive(Component)]
struct UiScoreboard;

#[derive(Resource, Default)]
struct ScoreData(Vec<(u64, u32, u32)>); // (id, kills, deaths)

#[derive(Resource, Default)]
struct ScoreVisible(bool);

#[derive(Resource, Default)]
struct RoundUi { phase_end: Option<Timer>, time_left: f32, winner: Option<u64> }

#[derive(Resource, Default)]
struct LocalAmmo { ammo: u16, reloading: bool }

// VFX components
#[derive(Component)]
struct MuzzleFx { timer: Timer }

#[derive(Component)]
struct TracerFx { timer: Timer }

#[derive(Component)]
struct ImpactFx { timer: Timer }

#[derive(Resource, Default)]
struct ActorKindsMap(std::collections::HashMap<u64, ActorKind>);

#[derive(Resource, Default)]
struct ActorPositions(std::collections::HashMap<u64, Vec3>);

#[derive(Component)]
struct UiDamageVignette { timer: Timer }

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
    jumps: u8, // 空中で使ったジャンプ回数（2段ジャンプ用）
}

#[derive(Resource, Default)]
struct MapReady(pub bool);

// ===== Scaffold (temporary platform) =====
const SCAFFOLD_SIZE: Vec3 = Vec3::new(2.0, 0.5, 2.0); // WxHxD (meters)
const SCAFFOLD_RANGE: f32 = 5.0; // meters
const SCAFFOLD_HP: i32 = 150;
const SCAFFOLD_LIFETIME: f32 = 10.0; // seconds
const SCAFFOLD_PER_PLAYER_LIMIT: usize = 3;

#[derive(Component)]
struct Scaffold { hp: i32, life: Timer, owner: u64 }

#[derive(Resource, Default)]
struct LocalScaffolds(Vec<Entity>); // FIFO 管理（ローカルプレイヤー用）

// ネット同期された足場（サーバ権威）
#[derive(Component)]
struct NetScaffold { sid: u64 }

#[derive(Resource, Default)]
struct NetScaffoldMap(std::collections::HashMap<u64, Entity>);

#[derive(SystemParam)]
struct NetScaffoldAssets<'w, 's> {
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    map: ResMut<'w, NetScaffoldMap>,
    _marker: std::marker::PhantomData<&'s ()>,
}

fn main() {
    App::new()
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.03)))
        .insert_resource(AmbientLight { color: Color::WHITE, brightness: 300.0 })
        .insert_resource(CursorLocked(true))
        .insert_resource(MapReady(false))
        .insert_resource(ConnStatePrev::default())
        .insert_resource(FpsTextTimer(Timer::from_seconds(0.5, TimerMode::Repeating)))
        .insert_resource(ActorKindsMap::default())
        .insert_resource(ActorPositions::default())
        .insert_resource(LocalScaffolds::default())
        .insert_resource(NetScaffoldMap::default())
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bevy FPS".into(),
                present_mode: if matches!(std::env::var("NO_VSYNC").ok().as_deref(), Some("1" | "true" | "TRUE")) { bevy::window::PresentMode::AutoNoVsync } else { bevy::window::PresentMode::AutoVsync },
                ..default()
            }),
            ..default()
        }))
        .add_plugins((RenetClientPlugin, NetcodeClientPlugin))
        .add_plugins(FrameTimeDiagnosticsPlugin)
        .add_plugins((
            RapierPhysicsPlugin::<NoUserData>::default(),
            // デバッグ表示が欲しい場合は下を有効化
            // RapierDebugRenderPlugin::default(),
        ))
        .add_systems(Startup, (setup_world, setup_ui, setup_physics, setup_net_client, setup_player, setup_hud))
        .add_systems(Update, handle_focus_events)
        .add_systems(Update, cursor_lock_controls)
        .add_systems(Update, mouse_look_system)
        .add_systems(Update, keyboard_look_system)
        .add_systems(Update, kcc_move_system)
        .add_systems(Update, kcc_post_step_system)
        .add_systems(Update, shoot_system)
        .add_systems(Update, bullet_move_and_despawn)
        .add_systems(Update, add_mesh_colliders_for_map)
        .add_systems(Update, net_log_connection)
        .add_systems(Update, net_send_input
            .after(mouse_look_system)
            .after(keyboard_look_system)
            .after(cursor_lock_controls)
            .after(handle_focus_events)
        )
        .add_systems(Update, net_recv_snapshot)
        .add_systems(Update, net_recv_events)
        .add_systems(Update, reconcile_self)
        .add_systems(Update, hud_update_hp)
        .add_systems(Update, hud_tick_hit_marker)
        .add_systems(Update, hud_tick_killlog)
        .add_systems(Update, hud_update_ammo)
        .add_systems(Update, fps_update_system)
        .add_systems(Update, scaffold_input_system)
        .add_systems(Update, (vfx_tick_and_cleanup, scaffold_tick_and_cleanup))
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
                shadows_enabled: !matches!(std::env::var("LOW_GFX").ok().as_deref(), Some("1" | "true" | "TRUE")),
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
        camera: Camera { hdr: !matches!(std::env::var("LOW_GFX").ok().as_deref(), Some("1" | "true" | "TRUE")), ..default() },
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
            "+",
            TextStyle {
                font_size: 28.0,
                color: Color::BLACK,
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
    // 微小ノイズ（トラックパッド等）を無視するデッドゾーン
    if delta.length_squared() < 0.04 { // ~0.2px 相当
        return;
    }

    // まずカメラ側で yaw/pitch を更新し、必要値をローカルに保持
    let mut new_yaw: f32 = 0.0;
    let mut new_pitch: f32 = 0.0;
    {
        let mut cam_query = q.p1();
        let Ok((mut cam_tf, mut pcam)) = cam_query.get_single_mut() else { return };
        pcam.yaw = wrap_pi(pcam.yaw - delta.x * MOUSE_SENSITIVITY);
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

fn setup_hud(mut commands: Commands) {
    // HP表示（左下）
    commands.spawn((
        TextBundle::from_section(
            "HP: 100",
            TextStyle { font_size: 36.0, color: Color::BLACK, ..default() },
        )
        .with_style(Style { position_type: PositionType::Absolute, left: Val::Px(10.0), bottom: Val::Px(10.0), ..default() }),
        UiHp,
    ));

    // FPS表示（左上）
    commands.spawn((
        TextBundle::from_section(
            "FPS: --",
            TextStyle { font_size: 18.0, color: Color::BLACK, ..default() },
        )
        .with_style(Style { position_type: PositionType::Absolute, left: Val::Px(10.0), top: Val::Px(10.0), ..default() }),
        UiFps,
    ));

    // ヒットマーカー（中心に薄いX、初期は透過）
    commands.spawn((
        TextBundle {
            text: Text::from_section(
                "x",
                TextStyle { font_size: 40.0, color: Color::srgba(0.0, 0.0, 0.0, 0.0), ..default() },
            ),
            style: Style { position_type: PositionType::Absolute, left: Val::Percent(50.0), top: Val::Percent(50.0), ..default() },
            ..default()
        },
        UiHitMarker { timer: Timer::from_seconds(0.0, TimerMode::Once) },
    ));

    // キルログ（右上、縦積み）
    commands.spawn((
        NodeBundle {
            style: Style {
                position_type: PositionType::Absolute,
                right: Val::Px(10.0),
                top: Val::Px(10.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                ..default()
            },
            background_color: BackgroundColor(Color::NONE),
            ..default()
        },
        UiKillLog,
    ));

    // スコアボード（中央上、非表示）
    commands.spawn((
        NodeBundle {
            style: Style {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Px(40.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            background_color: BackgroundColor(Color::NONE),
            visibility: Visibility::Hidden,
            ..default()
        },
        UiScoreboard,
    ));

    // ラウンド表示（中央上部、常時）
    commands.spawn((
        TextBundle::from_section(
            "",
            TextStyle { font_size: 32.0, color: Color::BLACK, ..default() },
        )
        .with_style(Style { position_type: PositionType::Absolute, left: Val::Percent(50.0), top: Val::Px(12.0), ..default() }),
        UiRoundText,
    ));

    // 弾数（右下）
    commands.spawn((
        TextBundle::from_section(
            "Ammo: 0",
            TextStyle { font_size: 28.0, color: Color::BLACK, ..default() },
        )
        .with_style(Style { position_type: PositionType::Absolute, right: Val::Px(10.0), bottom: Val::Px(10.0), ..default() }),
        UiAmmo,
    ));
}

#[derive(Component)]
struct UiRoundText;

#[derive(Component)]
struct UiAmmo;

fn round_ui_tick(
    time: Res<Time>,
    mut ui: ResMut<RoundUi>,
    mut q: Query<&mut Text, With<UiRoundText>>,
) {
    if let Ok(mut t) = q.get_single_mut() {
        if let Some(timer) = ui.phase_end.as_mut() {
            timer.tick(time.delta());
            let remain = (timer.duration().as_secs_f32() - timer.elapsed_secs()).max(0.0);
            t.sections[0].value = format!("Round End{}  Next: {:.0}s", ui.winner.map(|w| format!("  Winner {}", w)).unwrap_or_default(), remain);
        } else {
            ui.time_left = (ui.time_left - time.delta_seconds()).max(0.0);
            let m = (ui.time_left as i32 / 60).max(0);
            let s = (ui.time_left as i32 % 60).max(0);
            t.sections[0].value = format!("Time {:02}:{:02}", m, s);
        }
    }
}

// フォーカス喪失時に入力をクリアして「押しっぱなし」状態を解消
fn handle_focus_events(
    mut focused_events: EventReader<WindowFocused>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut buttons: ResMut<ButtonInput<MouseButton>>,
    mut mouse_evr: EventReader<MouseMotion>,
    mut locked: ResMut<CursorLocked>,
    mut win_q: Query<&mut Window>,
) {
    for ev in focused_events.read() {
        if !ev.focused {
            // 入力状態をリセット
            keys.clear();
            buttons.clear();
            mouse_evr.clear();
            // カーソルを解放
            locked.0 = false;
            if let Ok(mut w) = win_q.get_single_mut() {
                w.cursor.visible = true;
                w.cursor.grab_mode = CursorGrabMode::None;
            }
        }
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
        pcam.yaw = wrap_pi(pcam.yaw - horiz * KEY_LOOK_SPEED * dt);
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
    if keys.just_pressed(KeyCode::Space) {
        // 地上なら通常ジャンプ、空中なら1回だけ追加ジャンプを許可
        if ctrl.on_ground || ctrl.jumps < 1 {
            ctrl.vy = JUMP_SPEED;
            if !ctrl.on_ground { ctrl.jumps = ctrl.jumps.saturating_add(1); }
            ctrl.on_ground = false;
        }
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
            ctrl.jumps = 0; // 地上に戻ったら空中ジャンプ回数をリセット
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
    ammo: Res<LocalAmmo>,
) {
    if !(buttons.just_pressed(MouseButton::Left) || keys.just_pressed(KeyCode::KeyF)) {
        return;
    }
    // リロード中や弾0のときは見た目の弾は出さない（サーバー権威の判定は継続）
    if ammo.reloading || ammo.ammo == 0 {
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

// ===== HUD Systems =====
fn hud_update_hp(mut q: Query<&mut Text, With<UiHp>>, hp: Res<LocalHealth>) {
    if let Ok(mut t) = q.get_single_mut() {
        t.sections[0].value = format!("HP: {}", hp.hp);
        t.sections[0].style.color = Color::BLACK;
    }
}

fn hud_tick_hit_marker(time: Res<Time>, mut q: Query<(&mut UiHitMarker, &mut Text)>) {
    if let Ok((mut hm, mut text)) = q.get_single_mut() {
        hm.timer.tick(time.delta());
        let d = hm.timer.duration().as_secs_f32();
        let alpha = if d <= 0.0 { 0.0 } else { (d - hm.timer.elapsed_secs()).max(0.0) / d };
        text.sections[0].style.color = Color::srgba(0.0, 0.0, 0.0, alpha.clamp(0.0, 1.0));
    }
}

fn hud_tick_killlog(
    time: Res<Time>,
    mut commands: Commands,
    mut q: Query<(Entity, &mut UiKillEntry, &mut Text)>,
) {
    for (e, mut entry, mut text) in &mut q {
        entry.timer.tick(time.delta());
        let d = entry.timer.duration().as_secs_f32().max(0.0001);
        let remain = (d - entry.timer.elapsed_secs()).max(0.0) / d;
        text.sections[0].style.color = Color::srgba(1.0, 1.0, 1.0, remain.clamp(0.0, 1.0));
        if entry.timer.finished() { commands.entity(e).despawn_recursive(); }
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
    commands.insert_resource(ScoreData::default());
    commands.insert_resource(ScoreVisible::default());
    commands.insert_resource(RoundUi::default());
    commands.insert_resource(LocalAmmo { ammo: 0, reloading: false });
}

// ===== Scaffold Systems =====
fn scaffold_input_system(
    keys: Res<ButtonInput<KeyCode>>,
    cam_q: Query<&GlobalTransform, With<Camera3d>>,
    player_q: Query<Entity, With<Player>>,
    rapier: Res<RapierContext>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    local_id: Res<LocalNetInfo>,
    mut owned: ResMut<LocalScaffolds>,
    mut client: ResMut<RenetClient>,
) {
    if !keys.just_pressed(KeyCode::KeyQ) { return; }

    // 接続中はローカル生成せず、サーバへ生成要求のみ送る
    if client.is_connected() {
        let cam_g = if let Ok(v) = cam_q.get_single() { v } else { return };
        let origin = cam_g.translation();
        let dir: Vec3 = cam_g.forward().into();
        if dir.length_squared() < 1e-6 { return; }
        if let Ok(bytes) = bincode::serialize(&ClientMessage::PlaceScaffold { origin: [origin.x, origin.y, origin.z], dir: [dir.x, dir.y, dir.z] }) {
            // 順序保証のあるイベントチャネルで送る
            let _ = client.send_message(CH_RELIABLE, bytes);
        }
        return;
    }
    let cam_g = if let Ok(v) = cam_q.get_single() { v } else { return };
    let player_ent = if let Ok(e) = player_q.get_single() { e } else { return };

    let origin = cam_g.translation();
    let dir: Vec3 = cam_g.forward().into();

    let mut hit_pos = origin + dir * SCAFFOLD_RANGE;
    if let Some((entity, toi)) = rapier.cast_ray(
        origin,
        dir,
        SCAFFOLD_RANGE,
        true,
        QueryFilter::default().exclude_collider(player_ent).exclude_sensors(),
    ) {
        let _ = entity; // 現状は未使用
        hit_pos = origin + dir * toi;
    }

    // 常に水平（Y+ up）で配置。床の場合は僅かに浮かせてZファイティング回避
    let place_pos = hit_pos + Vec3::Y * (SCAFFOLD_SIZE.y * 0.5 + 0.01);

    // 3つ上限：超えたら一番古いものを消す
    if owned.0.len() >= SCAFFOLD_PER_PLAYER_LIMIT {
        if let Some(old) = owned.0.first().copied() {
            commands.entity(old).despawn_recursive();
        }
        owned.0.remove(0);
    }

    let mesh = meshes.add(Cuboid::new(SCAFFOLD_SIZE.x, SCAFFOLD_SIZE.y, SCAFFOLD_SIZE.z));
    let col = Color::srgba(0.2, 0.9, 1.0, 0.45);
    let mat = materials.add(StandardMaterial {
        base_color: col,
        emissive: Color::srgb(0.3, 0.8, 1.0).into(),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });

    let ent = commands
        .spawn((
            PbrBundle {
                mesh,
                material: mat,
                transform: Transform::from_translation(place_pos),
                ..default()
            },
            Scaffold { hp: SCAFFOLD_HP, life: Timer::from_seconds(SCAFFOLD_LIFETIME, TimerMode::Once), owner: local_id.id },
            Collider::cuboid(SCAFFOLD_SIZE.x * 0.5, SCAFFOLD_SIZE.y * 0.5, SCAFFOLD_SIZE.z * 0.5),
            RigidBody::Fixed,
        ))
        .id();

    owned.0.push(ent);
}

fn scaffold_tick_and_cleanup(
    time: Res<Time>,
    mut commands: Commands,
    mut q: Query<(Entity, &mut Scaffold)>,
    mut owned: ResMut<LocalScaffolds>,
) {
    for (e, mut sc) in &mut q {
        sc.life.tick(time.delta());
        if sc.life.finished() {
            commands.entity(e).despawn_recursive();
            // 所有リストからも除去
            if let Some(idx) = owned.0.iter().position(|x| *x == e) {
                owned.0.remove(idx);
            }
        }
    }
}

fn hud_update_ammo(mut q: Query<&mut Text, With<UiAmmo>>, ammo: Res<LocalAmmo>) {
    if let Ok(mut t) = q.get_single_mut() {
        if ammo.reloading {
            t.sections[0].value = format!("Reloading...");
        } else {
            t.sections[0].value = format!("Ammo: {}", ammo.ammo);
        }
    }
}

fn fps_update_system(
    time: Res<Time>,
    diagnostics: Res<DiagnosticsStore>,
    mut timer: ResMut<FpsTextTimer>,
    mut q: Query<&mut Text, With<UiFps>>,
) {
    timer.0.tick(time.delta());
    if !timer.0.finished() { return; }

    if let Ok(mut t) = q.get_single_mut() {
        if let Some(fps) = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS) {
            if let Some(avg) = fps.smoothed() {
                t.sections[0].value = format!("FPS: {:.0}", avg);
            } else if let Some(val) = fps.value() {
                t.sections[0].value = format!("FPS: {:.0}", val);
            }
        }
    }
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
    mut kinds: ResMut<ActorKindsMap>,
    mut positions: ResMut<ActorPositions>,
) {
    while let Some(raw) = client.receive_message(CH_SNAPSHOT) {
        if let Ok(ServerMessage::Snapshot(snap)) = bincode::deserialize::<ServerMessage>(&raw) {
            if matches!(std::env::var("NET_SNAPSHOT_LOG").ok(), Some(_)) && snap.players.len() > 0 {
                info!("client: snapshot players={}", snap.players.len());
            }
            for p in snap.players {
                kinds.0.insert(p.id, p.kind);
                positions.0.insert(p.id, Vec3::new(p.pos[0], p.pos[1], p.pos[2]));
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
                        let mat = materials.add(match p.kind { ActorKind::Human => Color::srgb(0.2, 0.9, 0.3), ActorKind::Bot => Color::srgb(0.9, 0.2, 0.2) });
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
    mut sc_assets: NetScaffoldAssets,
    local: Res<LocalNetInfo>,
    mut self_auth: ResMut<AuthoritativeSelf>,
    mut my_hp: ResMut<LocalHealth>,
    mut hit_q: Query<&mut UiHitMarker>,
    log_root_q: Query<Entity, With<UiKillLog>>,
    mut score_data: ResMut<ScoreData>,
    board_root_q: Query<Entity, With<UiScoreboard>>,
    mut round_ui: ResMut<RoundUi>,
    mut local_ammo: ResMut<LocalAmmo>,
    mut kinds: ResMut<ActorKindsMap>,
    mut player_q: Query<(&mut Transform, &mut Controller), With<Player>>,
) {
    while let Some(raw) = client.receive_message(CH_RELIABLE) {
        if let Ok(msg) = bincode::deserialize::<ServerMessage>(&raw) {
            match msg {
                ServerMessage::Event(ev) => match ev {
                    EventMsg::Spawn { id, pos, kind } => {
                    let p = Vec3::new(pos[0], pos[1], pos[2]);
                    kinds.0.insert(id, kind);
                    if id == local.id {
                        self_auth.pos = Some(p);
                        my_hp.hp = 100;
                        // Teleport local player to server spawn to avoid later corrections.
                        if let Ok((mut tf, mut ctrl)) = player_q.get_single_mut() {
                            tf.translation = p;
                            ctrl.vy = 0.0; ctrl.on_ground = true; ctrl.jumps = 0;
                        }
                    } else {
                        if let Some(&ent) = remap.0.get(&id) {
                            if let Some(mut ec) = commands.get_entity(ent) {
                                ec.insert(Transform::from_translation(p));
                            }
                        } else {
                            let mesh = sc_assets.meshes.add(Cuboid::new(0.4, 1.8, 0.4));
                            let mat = sc_assets.materials.add(match kind { ActorKind::Human => Color::srgb(0.2, 0.9, 0.3), ActorKind::Bot => Color::srgb(0.9, 0.2, 0.2) });
                            let ent = commands.spawn((PbrBundle { mesh, material: mat, transform: Transform::from_translation(p), ..default() }, RemoteAvatar { id })).id();
                            remap.0.insert(id, ent);
                        }
                    }
                }
                EventMsg::Despawn { id } => {
                    if let Some(ent) = remap.0.remove(&id) { commands.entity(ent).despawn_recursive(); }
                }
                EventMsg::Hit { target_id, new_hp, by } => {
                    if target_id == local.id { my_hp.hp = new_hp; }
                    if by == local.id {
                        if let Ok(mut hm) = hit_q.get_single_mut() {
                            hm.timer.set_duration(Duration::from_secs_f32(0.15));
                            hm.timer.reset();
                        }
                    }
                    if target_id == local.id {
                        // Add damage vignette overlay
                        commands.spawn((
                            NodeBundle {
                                style: Style { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
                                background_color: BackgroundColor(Color::rgba(0.8, 0.0, 0.0, 0.35)),
                                ..default()
                            },
                            UiDamageVignette { timer: Timer::from_seconds(0.4, TimerMode::Once) },
                        ));
                    }
                }
                EventMsg::Death { target_id, by } => {
                    if target_id == local.id { my_hp.hp = 0; }
                    if let Some(ent) = remap.0.remove(&target_id) { commands.entity(ent).despawn_recursive(); }
                    // キルログ追加
                    let killer = if by == local.id { "You".to_string() } else { format!("{}", by) };
                    let victim = if target_id == local.id { "You".to_string() } else { format!("{}", target_id) };
                    let line = format!("{} → {}", killer, victim);
                    if let Ok(root) = log_root_q.get_single() {
                        commands.entity(root).with_children(|p| {
                            p.spawn((
                                TextBundle::from_section(
                                    line,
                                    TextStyle { font_size: 24.0, color: Color::BLACK, ..default() },
                                ),
                                UiKillEntry { timer: Timer::from_seconds(3.0, TimerMode::Once) },
                            ));
                        });
                    }
                }
                EventMsg::Fire { id, origin, dir, hit } => {
                    // VFX: muzzle + tracer (+ impact)
                    let o = Vec3::new(origin[0], origin[1], origin[2]);
                    let d = Vec3::new(dir[0], dir[1], dir[2]).normalize_or_zero();
                    let end = match hit { Some(h) => Vec3::new(h[0], h[1], h[2]), None => o + d * 50.0 };
                    let col = match kinds.0.get(&id).copied() { Some(ActorKind::Bot) => Color::srgb(0.95, 0.25, 0.2), _ => Color::srgb(0.95, 0.9, 0.2) };
                    // muzzle
                    let mmesh = sc_assets.meshes.add(Cuboid::new(0.06, 0.06, 0.06));
                    let mmat = sc_assets.materials.add(StandardMaterial { base_color: col, emissive: col.into(), unlit: true, ..default() });
                    commands.spawn((PbrBundle { mesh: mmesh, material: mmat, transform: Transform::from_translation(o), ..default() }, MuzzleFx { timer: Timer::from_seconds(0.06, TimerMode::Once) }));
                    // tracer
                    let seg = end - o; let len = seg.length();
                    if len > 0.001 {
                        let tmesh = sc_assets.meshes.add(Cuboid::new(0.02, 0.02, len.max(0.05)));
                        let tmat = sc_assets.materials.add(StandardMaterial { base_color: col, emissive: col.into(), unlit: true, ..default() });
                        let rot = Quat::from_rotation_arc(Vec3::Z, seg.normalize());
                        let pos = o + seg * 0.5;
                        commands.spawn((PbrBundle { mesh: tmesh, material: tmat, transform: Transform { translation: pos, rotation: rot, scale: Vec3::ONE }, ..default() }, TracerFx { timer: Timer::from_seconds(0.06, TimerMode::Once) }));
                    }
                    // impact
                    if let Some(h) = hit { let hp = Vec3::new(h[0], h[1], h[2]); let imesh = sc_assets.meshes.add(Cuboid::new(0.05, 0.05, 0.02)); let imat = sc_assets.materials.add(StandardMaterial { base_color: Color::srgb(1.0, 0.6, 0.3), emissive: Color::srgb(1.0, 0.6, 0.3).into(), unlit: true, ..default() }); commands.spawn((PbrBundle { mesh: imesh, material: imat, transform: Transform::from_translation(hp), ..default() }, ImpactFx { timer: Timer::from_seconds(0.2, TimerMode::Once) })); }
                }
                EventMsg::RoundStart { time_left_sec } => {
                    round_ui.phase_end = None;
                    round_ui.time_left = time_left_sec as f32;
                    round_ui.winner = None;
                }
                EventMsg::RoundEnd { winner_id, next_in_sec } => {
                    round_ui.winner = winner_id;
                    round_ui.phase_end = Some(Timer::from_seconds(next_in_sec as f32, TimerMode::Once));
                }
                EventMsg::Ammo { id, ammo, reloading } => {
                    if id == local.id {
                        local_ammo.ammo = ammo;
                        local_ammo.reloading = reloading;
                    }
                }
                EventMsg::ScaffoldSpawn { sid, owner: _owner, pos } => {
                    let p = Vec3::new(pos[0], pos[1], pos[2]);
                    let mesh = sc_assets.meshes.add(Cuboid::new(SCAFFOLD_SIZE.x, SCAFFOLD_SIZE.y, SCAFFOLD_SIZE.z));
                    let col = Color::srgba(0.2, 0.9, 1.0, 0.45);
                    let mat = sc_assets.materials.add(StandardMaterial {
                        base_color: col,
                        emissive: Color::srgb(0.3, 0.8, 1.0).into(),
                        alpha_mode: AlphaMode::Blend,
                        unlit: true,
                        ..default()
                    });
                    let ent = commands
                        .spawn((
                            PbrBundle { mesh, material: mat, transform: Transform::from_translation(p), ..default() },
                            NetScaffold { sid },
                            Collider::cuboid(SCAFFOLD_SIZE.x * 0.5, SCAFFOLD_SIZE.y * 0.5, SCAFFOLD_SIZE.z * 0.5),
                            RigidBody::Fixed,
                        ))
                        .id();
                    sc_assets.map.0.insert(sid, ent);
                }
                EventMsg::ScaffoldDespawn { sid } => {
                    if let Some(ent) = sc_assets.map.0.remove(&sid) {
                        commands.entity(ent).despawn_recursive();
                    }
                }
            },
                ServerMessage::Score(entries) => {
                    // 更新して、スコアボードUIを再構築
                    score_data.0 = entries.into_iter().map(|e| (e.id, e.kills, e.deaths)).collect();
                    if let Ok(root) = board_root_q.get_single() {
                        if let Some(mut ec) = commands.get_entity(root) { ec.despawn_descendants(); }
                        commands.entity(root).with_children(|p| {
                    p.spawn(TextBundle::from_section(
                        format!("{:>6}  {:>5} {:>6}", "ID", "K", "D"),
                        TextStyle { font_size: 28.0, color: Color::BLACK, ..default() },
                    ));
                            let mut rows = score_data.0.clone();
                            rows.sort_by_key(|e| (-(e.1 as i32), e.2 as i32));
                            for (id, k, d) in rows {
                        p.spawn(TextBundle::from_section(
                            format!("{:>6}  {:>5} {:>6}", id, k, d),
                            TextStyle { font_size: 24.0, color: Color::BLACK, ..default() },
                        ));
                            }
                        });
                    }
                }
                _ => {}
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
    // Position reconciliation is opt-in; default off to prefer client authority.
    if !matches!(std::env::var("RECONCILE_POS").ok().as_deref(), Some("1" | "true" | "TRUE")) { return; }
    let mut tf = if let Ok(t) = q.get_single_mut() { t } else { return };
    if let Some(target) = self_auth.pos {
        let diff = target - tf.translation;
        let d = diff.length();
        // Deadband: ignore tiny differences to avoid visible jitter.
        if d <= POS_DEADBAND { return; }
        // Snap when far out-of-bounds to recover quickly.
        if d >= POS_SNAP { tf.translation = target; return; }
        // Smooth correction within thresholds.
        let rate = 10.0; // per second
        let step = (rate * time.delta_seconds()).min(1.0);
        tf.translation += diff * step;
    }
    if matches!(std::env::var("RECONCILE_YAW").ok().as_deref(), Some("1" | "true" | "TRUE")) { if let Some(yaw) = self_auth.yaw {
        // 軽い追従のみ（強いワープは避ける）
        let current_yaw = tf.rotation.to_euler(EulerRot::YXZ).0;
        let delta = wrap_pi(yaw - current_yaw);
        let step = (6.0 * time.delta_seconds()).min(1.0);
        tf.rotation = Quat::from_rotation_y(wrap_pi(current_yaw + delta * step));
    }
}

fn scoreboard_toggle(
    keys: Res<ButtonInput<KeyCode>>,
    mut q: Query<&mut Visibility, With<UiScoreboard>>,
) {
    if keys.just_pressed(KeyCode::Tab) {
        if let Ok(mut v) = q.get_single_mut() {
            *v = match *v {
                Visibility::Hidden => Visibility::Visible,
                _ => Visibility::Hidden,
            };
        }
    } }
}

// --- VFX tickers ---
fn vfx_tick_and_cleanup(
    time: Res<Time>,
    mut commands: Commands,
    mut q_muzzle: Query<(Entity, &mut MuzzleFx)>,
    mut q_tracer: Query<(Entity, &mut TracerFx)>,
    mut q_impact: Query<(Entity, &mut ImpactFx)>,
    mut q_vign: Query<(Entity, &mut UiDamageVignette)>,
    mut bg_colors: Query<&mut BackgroundColor>,
) {
    for (e, mut fx) in &mut q_muzzle { fx.timer.tick(time.delta()); if fx.timer.finished() { commands.entity(e).despawn_recursive(); } }
    for (e, mut fx) in &mut q_tracer { fx.timer.tick(time.delta()); if fx.timer.finished() { commands.entity(e).despawn_recursive(); } }
    for (e, mut fx) in &mut q_impact { fx.timer.tick(time.delta()); if fx.timer.finished() { commands.entity(e).despawn_recursive(); } }
    for (e, mut v) in &mut q_vign {
        v.timer.tick(time.delta());
        if let Ok(mut col) = bg_colors.get_mut(e) {
            let t = (1.0 - (v.timer.elapsed_secs() / v.timer.duration().as_secs_f32())).clamp(0.0, 1.0);
            col.0 = Color::rgba(0.8, 0.0, 0.0, 0.35 * t);
        }
        if v.timer.finished() { commands.entity(e).despawn_recursive(); }
    }
}
