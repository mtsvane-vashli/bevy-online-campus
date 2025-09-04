use bevy_renet::renet::{ClientId, ConnectionConfig, RenetClient, RenetServer};
use renet::transport::{ClientAuthentication, NetcodeClientTransport, NetcodeServerTransport, ServerAuthentication, ServerConfig};
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerStateMsg {
    pub id: u64,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub alive: bool,
    pub hp: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMsg {
    pub tick: u32,
    pub players: Vec<PlayerStateMsg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    Input(InputFrame),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    Snapshot(SnapshotMsg),
    Event(EventMsg),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventMsg {
    Spawn { id: u64, pos: [f32; 3] },
    Despawn { id: u64 },
    Hit { target_id: u64, new_hp: u16, by: u64 },
    Death { target_id: u64, by: u64 },
}

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
    let server_config = ServerConfig {
        current_time,
        max_clients,
        protocol_id: PROTOCOL_ID,
        public_addresses: vec![public_addr],
        authentication: ServerAuthentication::Unsecure,
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
    // Unsecure client (development)
    let authentication = ClientAuthentication::Unsecure { server_addr, client_id: client_id.raw(), protocol_id: PROTOCOL_ID, user_data: None };
    // 環境変数 CLIENT_PORT があればそのポートでバインド（デバッグ用）
    let env_port = env::var("CLIENT_PORT").ok().and_then(|s| s.parse::<u16>().ok());
    let lp = local_port.or(env_port).unwrap_or(0);
    let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, lp));
    let socket = UdpSocket::bind(bind_addr).expect("bind client socket");
    if let Ok(local) = socket.local_addr() { println!("client socket bound at {} (client_id={})", local, client_id.raw()); }
    let transport = NetcodeClientTransport::new(current_time, authentication, socket).expect("client transport");
    (client, transport, client_id)
}
