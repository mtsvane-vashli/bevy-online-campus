use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;
use bevy::window::CursorGrabMode;

// ===== Config =====
const MAP_SCENE_PATH: &str = "maps/map.glb#Scene0"; // assets 配下に maps/map.glb を置いてください
const PLAYER_START: Vec3 = Vec3::new(0.0, 1.6, 5.0);
const MOVE_SPEED: f32 = 6.0; // m/s
const RUN_MULTIPLIER: f32 = 1.7;
const MOUSE_SENSITIVITY: f32 = 0.0018; // rad/pixel
const BULLET_SPEED: f32 = 40.0; // m/s
const BULLET_LIFETIME: f32 = 2.0; // sec

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

fn main() {
    App::new()
        .insert_resource(ClearColor(Color::rgb(0.02, 0.02, 0.03)))
        .insert_resource(AmbientLight { color: Color::WHITE, brightness: 300.0 })
        .insert_resource(CursorLocked(true))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bevy FPS".into(),
                present_mode: bevy::window::PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }))
        .add_systems(Startup, (setup_world, setup_player, setup_ui))
        .add_systems(Update, (
            cursor_lock_controls,
            mouse_look_system,
            player_move_system,
            shoot_system,
            bullet_move_and_despawn,
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
        .id();

    // カメラはプレイヤーの子: pitch はカメラにのみ反映
    let cam = Camera3dBundle {
        transform: Transform::from_xyz(0.0, 0.0, 0.0),
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
                color: Color::rgb(1.0, 1.0, 1.0),
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
    let mut yaw_pitch: Option<(f32, f32)> = None;
    {
        let mut cam_query = q.p1();
        let Ok((mut cam_tf, mut pcam)) = cam_query.get_single_mut() else { return };
        pcam.yaw -= delta.x * MOUSE_SENSITIVITY;
        pcam.pitch = (pcam.pitch - delta.y * MOUSE_SENSITIVITY).clamp(-1.54, 1.54);
        yaw_pitch = Some((pcam.yaw, pcam.pitch));
        cam_tf.rotation = Quat::from_rotation_x(pcam.pitch);
    }

    // 次にプレイヤーの yaw 回転を反映（別スコープで別クエリを借用）
    if let Some((yaw, _pitch)) = yaw_pitch {
        let mut player_query = q.p0();
        if let Ok(mut player_tf) = player_query.get_single_mut() {
            player_tf.rotation = Quat::from_rotation_y(yaw);
        }
    }
}

fn player_move_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    cam_q: Query<&PlayerCamera, With<Camera3d>>,
    mut player_q: Query<&mut Transform, (With<Player>, Without<Camera3d>)>,
) {
    let mut tf = if let Ok(v) = player_q.get_single_mut() { v } else { return };
    let cam = if let Ok(v) = cam_q.get_single() { v } else { return };

    let mut input = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) { input += Vec3::NEG_Z; }
    if keys.pressed(KeyCode::KeyS) { input += Vec3::Z; }
    if keys.pressed(KeyCode::KeyA) { input += Vec3::NEG_X; }
    if keys.pressed(KeyCode::KeyD) { input += Vec3::X; }

    if input.length_squared() > 1e-6 {
        let yaw_rot = Quat::from_rotation_y(cam.yaw);
        let dir = yaw_rot * input.normalize();
        let mut speed = MOVE_SPEED;
        if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            speed *= RUN_MULTIPLIER;
        }
        tf.translation += dir * speed * time.delta_seconds();
    }
}

fn shoot_system(
    buttons: Res<ButtonInput<MouseButton>>,
    cam_global_q: Query<&GlobalTransform, With<Camera3d>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let cam_g = if let Ok(v) = cam_global_q.get_single() { v } else { return };

    let forward = cam_g.forward();
    let start = cam_g.translation();

    // 小さな弾体（可視化用）
    let mesh = meshes.add(Sphere::new(0.04).mesh().ico(4).unwrap());
    let mat = materials.add(Color::rgb(1.0, 0.9, 0.2));

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
