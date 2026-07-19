#![allow(unused_imports)]
use quinn::{ClientConfig, Endpoint, ServerConfig};
use rustls::Certificate;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tokio::runtime::Runtime;

use crate::runtime::cluster::NodeId;
use crate::runtime::network::{IncomingPacket, NetworkTransport, Packet};

pub struct QuicTransport {
    node_id: NodeId,
    listen_addr: SocketAddr,
    #[allow(dead_code)]
    tokio_rt: Arc<Runtime>,
    #[allow(dead_code)]
    endpoint: Endpoint,
    incoming_rx: mpsc::Receiver<IncomingPacket>,
    #[allow(dead_code)]
    incoming_tx: mpsc::SyncSender<IncomingPacket>,
    connections: Arc<Mutex<HashMap<NodeId, quinn::Connection>>>,
}

impl QuicTransport {
    pub fn bind(addr: SocketAddr) -> std::io::Result<Self> {
        let tokio_rt = Arc::new(Runtime::new().unwrap());

        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert_der = cert.serialize_der().unwrap();
        let priv_key = cert.serialize_private_key_der();
        let priv_key = rustls::PrivateKey(priv_key);
        let cert_chain = vec![rustls::Certificate(cert_der.clone())];

        let server_crypto = rustls::ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(cert_chain, priv_key)
            .unwrap();

        let server_config = ServerConfig::with_crypto(Arc::new(server_crypto));
        let endpoint = tokio_rt.block_on(async { Endpoint::server(server_config, addr) })?;

        let (incoming_tx, incoming_rx) = mpsc::sync_channel(1024);

        Ok(QuicTransport {
            node_id: NodeId(0), // Set correctly later or pass to bind
            listen_addr: endpoint.local_addr()?,
            tokio_rt,
            endpoint,
            incoming_rx,
            incoming_tx,
            connections: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

impl NetworkTransport for QuicTransport {
    fn connect(&mut self, _node_id: NodeId, _addr: SocketAddr) -> std::io::Result<()> {
        Ok(()) // Stub
    }
    fn send(&mut self, _to_node: NodeId, _to_addr: SocketAddr, _packet: Packet) {}
    fn receive(&self) -> Vec<IncomingPacket> {
        let mut packets = Vec::new();
        while let Ok(packet) = self.incoming_rx.try_recv() {
            packets.push(packet);
        }
        packets
    }
    fn node_id(&self) -> NodeId {
        self.node_id
    }
    fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }
    fn disconnect(&mut self, _node_id: NodeId) {}
    fn shutdown(&mut self) {}
    fn connection_count(&self) -> usize {
        self.connections.lock().unwrap().len()
    }
    fn connection_addr(&self, _node_id: NodeId) -> Option<SocketAddr> {
        None // Stub
    }
}
