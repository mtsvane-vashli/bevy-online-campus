use bevy_renet::renet::{ClientId, ConnectionConfig, RenetClient, RenetServer};
use renet::transport::{ClientAuthentication, NetcodeClientTransport, NetcodeServerTransport, ServerAuthentication, ServerConfig};
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::SystemTime;

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
    // Bind to all interfaces, but advertise loopback for local testing
    let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, SERVER_PORT));
    let public_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, SERVER_PORT));
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
    let transport = NetcodeServerTransport::new(server_config, socket).expect("transport");
    (server, transport)
}

pub fn new_client(local_port: Option<u16>) -> (RenetClient, NetcodeClientTransport, ClientId) {
    let client = RenetClient::new(connection_config());
    let server_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, SERVER_PORT));
    let client_id = ClientId::from_raw(rand::random::<u64>());
    let current_time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    // Unsecure client (development)
    let authentication = ClientAuthentication::Unsecure { server_addr, client_id: client_id.raw(), protocol_id: PROTOCOL_ID, user_data: None };
    let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, local_port.unwrap_or(0)));
    let socket = UdpSocket::bind(bind_addr).expect("bind client socket");
    let transport = NetcodeClientTransport::new(current_time, authentication, socket).expect("client transport");
    (client, transport, client_id)
}
