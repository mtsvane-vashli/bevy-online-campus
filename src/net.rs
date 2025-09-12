use bevy_renet::renet::{ClientId, ConnectionConfig, RenetClient, RenetServer};
use renet::transport::{ClientAuthentication, NetcodeClientTransport, NetcodeServerTransport, ServerAuthentication, ServerConfig, ConnectToken};
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::SystemTime;
use std::env;

pub const PROTOCOL_ID: u64 = 7_294_871_223_100_001;
pub const SERVER_PORT: u16 = 5000;

pub const CH_INPUT: u8 = 0; // unreliable, ordered
pub const CH_SNAPSHOT: u8 = 1; // unreliable, ordered
pub const CH_RELIABLE: u8 = 2; // reliable, ordered (events)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputFrame {
    pub seq: u32,
    pub mv: [f32; 2], // x,z on local plane
    pub run: bool,
    pub jump: bool,
    pub fire: bool,
    pub yaw: f32,
    pub pitch: f32,
    pub dt: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerStateMsg {
    pub id: u64,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub alive: bool,
    pub hp: u16,
    pub kind: ActorKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMsg {
    pub tick: u32,
    pub players: Vec<PlayerStateMsg>,
    // サーバが各クライアントについて直近処理した入力seq（ACK）
    pub acks: Vec<(u64, u32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    Input(InputFrame),
    // クライアントが足場生成を要求（最終配置座標を送る：クライアント側と同一計算）
    PlaceScaffold { pos: [f32; 3] },
    // 射撃要求（クライアントのカメラ原点・方向を送る）
    Fire { origin: [f32; 3], dir: [f32; 3] },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    Snapshot(SnapshotMsg),
    Event(EventMsg),
    Score(Vec<ScoreEntry>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventMsg {
    Spawn { id: u64, pos: [f32; 3], kind: ActorKind },
    Despawn { id: u64 },
    Hit { target_id: u64, new_hp: u16, by: u64 },
    Death { target_id: u64, by: u64 },
    RoundStart { time_left_sec: u32 },
    RoundEnd { winner_id: Option<u64>, next_in_sec: u32 },
    Ammo { id: u64, ammo: u16, reloading: bool },
    Fire { id: u64, origin: [f32; 3], dir: [f32; 3], hit: Option<[f32; 3]> },
    // 足場の生成/消滅（サーバ権威）
    ScaffoldSpawn { sid: u64, owner: u64, pos: [f32; 3] },
    ScaffoldDespawn { sid: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreEntry { pub id: u64, pub kills: u32, pub deaths: u32 }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Copy)]
pub enum ActorKind { Human, Bot }

pub fn connection_config() -> ConnectionConfig { ConnectionConfig::default() }

pub fn new_server() -> (RenetServer, NetcodeServerTransport) {
    let server = RenetServer::new(connection_config());
    // SERVER_ADDR=host:port があればそれを広告先にし、ポートも合わせてバインド
    let env_server_addr = env::var("SERVER_ADDR").ok().and_then(|s| s.parse::<SocketAddr>().ok());
    let (public_addr, bind_port) = match env_server_addr {
        Some(sock) => (sock, sock.port()),
        None => (
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, SERVER_PORT)),
            SERVER_PORT,
        ),
    };
    // Bind to all interfaces for reachability on LAN
    let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, bind_port));
    let current_time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    let max_clients = 32;
    // Secure/Unsecure 切替（WAN時は SECURE=1 と NETCODE_KEY を設定）
    let secure = matches!(env::var("SECURE").ok().as_deref(), Some("1" | "true" | "TRUE"));
    let authentication = if secure {
        let key = read_netcode_key().expect("SECURE=1 ですが NETCODE_KEY/NETCODE_KEY_FILE が不正です");
        ServerAuthentication::Secure { private_key: key }
    } else {
        ServerAuthentication::Unsecure
    };

    let server_config = ServerConfig {
        current_time,
        max_clients,
        protocol_id: PROTOCOL_ID,
        public_addresses: vec![public_addr],
        authentication,
    };
    let socket = UdpSocket::bind(bind_addr).expect("bind server socket");
    if let Ok(local) = socket.local_addr() { println!("server socket bound at {} (public {})", local, server_config.public_addresses[0]); }
    let transport = NetcodeServerTransport::new(server_config, socket).expect("transport");
    (server, transport)
}

pub fn new_client(local_port: Option<u16>) -> (RenetClient, NetcodeClientTransport, ClientId) {
    let client = RenetClient::new(connection_config());
    // SERVER_ADDR=host:port があれば優先（同一Wi-Fi/別PC接続向け）。無ければ 127.0.0.1:SERVER_PORT
    let server_addr = env::var("SERVER_ADDR")
        .ok()
        .and_then(|s| s.parse::<SocketAddr>().ok())
        .unwrap_or_else(|| SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, SERVER_PORT)));
    let client_id = ClientId::from_raw(rand::random::<u64>());
    let current_time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    // Secure/Unsecure 切替（WAN時は SECURE=1 と NETCODE_KEY を設定）
    let secure = matches!(env::var("SECURE").ok().as_deref(), Some("1" | "true" | "TRUE"));
    let authentication = if secure {
        let key = read_netcode_key().expect("SECURE=1 ですが NETCODE_KEY/NETCODE_KEY_FILE が不正です");
        let token = ConnectToken::generate(
            current_time,
            PROTOCOL_ID,
            120,                         // token expire seconds
            client_id.raw(),             // client id
            15,                          // handshake timeout seconds
            vec![server_addr],           // server addresses
            None,                        // optional user data
            &key,
        ).expect("generate connect token");
        ClientAuthentication::Secure { connect_token: token }
    } else {
        // Unsecure client (development)
        ClientAuthentication::Unsecure { server_addr, client_id: client_id.raw(), protocol_id: PROTOCOL_ID, user_data: None }
    };
    // 環境変数 CLIENT_PORT があればそのポートでバインド（デバッグ用）
    let env_port = env::var("CLIENT_PORT").ok().and_then(|s| s.parse::<u16>().ok());
    let lp = local_port.or(env_port).unwrap_or(0);
    let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, lp));
    let socket = UdpSocket::bind(bind_addr).expect("bind client socket");
    if let Ok(local) = socket.local_addr() { println!("client socket bound at {} (client_id={})", local, client_id.raw()); }
    let transport = NetcodeClientTransport::new(current_time, authentication, socket).expect("client transport");
    (client, transport, client_id)
}

// --- helpers ---

fn read_netcode_key() -> Result<[u8; 32], String> {
    // 優先: NETCODE_KEY（HEX 64文字 or 0x...付き）
    if let Ok(s) = env::var("NETCODE_KEY") {
        return parse_hex_key(&s);
    }
    // 次点: NETCODE_KEY_FILE（バイナリ32バイト or HEX文字列）
    if let Ok(path) = env::var("NETCODE_KEY_FILE") {
        let data = std::fs::read(&path).map_err(|e| format!("read NETCODE_KEY_FILE: {}", e))?;
        if data.len() == 32 {
            let mut k = [0u8; 32];
            k.copy_from_slice(&data);
            return Ok(k);
        }
        let s = String::from_utf8_lossy(&data).trim().to_string();
        return parse_hex_key(&s);
    }
    Err("NETCODE_KEY か NETCODE_KEY_FILE を指定してください".into())
}

fn parse_hex_key(s: &str) -> Result<[u8; 32], String> {
    let s = s.trim();
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() != 64 { return Err("HEXキーは64桁で指定してください".into()); }
    let mut out = [0u8; 32];
    for i in 0..32 {
        let b = u8::from_str_radix(&s[i*2..i*2+2], 16).map_err(|_| "HEX変換に失敗しました")?;
        out[i] = b;
    }
    Ok(out)
}
