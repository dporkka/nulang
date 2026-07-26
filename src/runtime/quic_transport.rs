use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{mpsc, Arc};

use quinn::{Endpoint, ServerConfig};
use rustls::pki_types::PrivateKeyDer;
use tokio::runtime::Runtime;

use crate::runtime::cluster::NodeId;
use crate::runtime::network::{IncomingPacket, NetworkTransport, Packet};

struct QuicPeer {
    send: quinn::SendStream,
    addr: SocketAddr,
}

pub struct QuicTransport {
    node_id: NodeId,
    listen_addr: SocketAddr,
    tokio_rt: Arc<Runtime>,
    endpoint: Arc<Endpoint>,
    incoming_rx: mpsc::Receiver<IncomingPacket>,
    incoming_tx: mpsc::SyncSender<IncomingPacket>,
    peers: Arc<std::sync::Mutex<HashMap<NodeId, QuicPeer>>>,
}

impl QuicTransport {
    pub fn bind(addr: SocketAddr, node_id: NodeId) -> io::Result<Self> {
        let tokio_rt = Arc::new(Runtime::new().map_err(|e| io::Error::other(e.to_string()))?);

        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])
            .map_err(|e| io::Error::other(e.to_string()))?;
        let cert_der = cert.cert.der().clone();
        let priv_key = cert.signing_key.serialize_der();
        let priv_key =
            PrivateKeyDer::try_from(priv_key).map_err(|e| io::Error::other(e.to_string()))?;

        let server_config = ServerConfig::with_single_cert(vec![cert_der], priv_key)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let endpoint =
            Arc::new(tokio_rt.block_on(async { Endpoint::server(server_config, addr) })?);

        let listen_addr = endpoint.local_addr()?;
        let (incoming_tx, incoming_rx) = mpsc::sync_channel(1024);
        let peers = Arc::new(std::sync::Mutex::new(HashMap::new()));

        // Background accept loop
        let accept_ep = Arc::clone(&endpoint);
        let accept_tx = incoming_tx.clone();
        let accept_peers = Arc::clone(&peers);
        let accept_rt = Arc::clone(&tokio_rt);
        tokio_rt.spawn(async move {
            loop {
                match accept_ep.accept().await {
                    Some(incoming) => {
                        let tx = accept_tx.clone();
                        let p = Arc::clone(&accept_peers);
                        accept_rt.spawn(async move {
                            if let Err(e) = accept_one(incoming, tx, p).await {
                                tracing::warn!("nulang-quic accept: {}", e);
                            }
                        });
                    }
                    None => break,
                }
            }
        });

        Ok(QuicTransport {
            node_id,
            listen_addr,
            tokio_rt,
            endpoint,
            incoming_rx,
            incoming_tx,
            peers,
        })
    }
}

async fn accept_one(
    incoming: quinn::Incoming,
    tx: mpsc::SyncSender<IncomingPacket>,
    peers: Arc<std::sync::Mutex<HashMap<NodeId, QuicPeer>>>,
) -> io::Result<()> {
    let conn = incoming
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;
    let remote = conn.remote_address();

    let (mut send, mut recv) = conn
        .accept_bi()
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;
    let mut buf = [0u8; 8];
    recv.read_exact(&mut buf)
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;
    let peer_id = NodeId(u64::from_be_bytes(buf));
    send.write_all(&0u64.to_be_bytes())
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;

    let (data_send, data_recv) = conn
        .accept_bi()
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;
    peers.lock().unwrap().insert(
        peer_id,
        QuicPeer {
            send: data_send,
            addr: remote,
        },
    );
    tokio::spawn(read_loop(peer_id, data_recv, tx));
    Ok(())
}

async fn read_loop(
    peer_id: NodeId,
    mut recv: quinn::RecvStream,
    tx: mpsc::SyncSender<IncomingPacket>,
) {
    loop {
        let mut lb = [0u8; 4];
        if recv.read_exact(&mut lb).await.is_err() {
            break;
        }
        let len = u32::from_be_bytes(lb) as usize;
        if len == 0 || len > 16 * 1024 * 1024 {
            break;
        }
        let mut payload = vec![0u8; len];
        if recv.read_exact(&mut payload).await.is_err() {
            break;
        }
        if let Some((seq, packet)) = Packet::from_bytes(&payload) {
            let _ = tx.send(IncomingPacket {
                from_node: peer_id,
                seq,
                packet,
            });
        }
    }
}

impl NetworkTransport for QuicTransport {
    fn connect(&mut self, nid: NodeId, addr: SocketAddr) -> io::Result<()> {
        if self.peers.lock().unwrap().contains_key(&nid) {
            return Ok(());
        }

        let conn = self.tokio_rt.block_on(async {
            self.endpoint
                .connect(addr, "localhost")
                .map_err(|e| io::Error::other(e.to_string()))?
                .await
                .map_err(|e| io::Error::other(e.to_string()))
        })?;

        let (mut send, mut recv) = self.tokio_rt.block_on(async {
            conn.open_bi()
                .await
                .map_err(|e| io::Error::other(e.to_string()))
        })?;

        self.tokio_rt.block_on(async {
            send.write_all(&self.node_id.0.to_be_bytes())
                .await
                .map_err(|e| io::Error::other(e.to_string()))
        })?;
        let mut buf = [0u8; 8];
        self.tokio_rt.block_on(async {
            recv.read_exact(&mut buf)
                .await
                .map_err(|e| io::Error::other(e.to_string()))
        })?;

        let (data_send, data_recv) = self.tokio_rt.block_on(async {
            conn.open_bi()
                .await
                .map_err(|e| io::Error::other(e.to_string()))
        })?;
        let tx = self.incoming_tx.clone();
        tokio::spawn(read_loop(nid, data_recv, tx));
        self.peers.lock().unwrap().insert(
            nid,
            QuicPeer {
                send: data_send,
                addr,
            },
        );
        Ok(())
    }

    fn send(&mut self, to: NodeId, _addr: SocketAddr, packet: Packet) {
        if let Some(peer) = self.peers.lock().unwrap().get_mut(&to) {
            let payload = packet.to_bytes(0);
            let len = (payload.len() as u32).to_be_bytes();
            let mut framed = Vec::with_capacity(4 + payload.len());
            framed.extend_from_slice(&len);
            framed.extend_from_slice(&payload);
            let _ = self
                .tokio_rt
                .block_on(async { peer.send.write_all(&framed).await });
        }
    }

    fn receive(&self) -> Vec<IncomingPacket> {
        let mut v = Vec::new();
        while let Ok(p) = self.incoming_rx.try_recv() {
            v.push(p);
        }
        v
    }

    fn node_id(&self) -> NodeId {
        self.node_id
    }
    fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }
    fn disconnect(&mut self, nid: NodeId) {
        self.peers.lock().unwrap().remove(&nid);
    }
    fn shutdown(&mut self) {
        self.peers.lock().unwrap().clear();
        let _ = self.tokio_rt.block_on(async {
            self.endpoint.close(0u32.into(), b"shutdown");
        });
    }
    fn connection_count(&self) -> usize {
        self.peers.lock().unwrap().len()
    }
    fn connection_addr(&self, nid: NodeId) -> Option<SocketAddr> {
        self.peers.lock().unwrap().get(&nid).map(|p| p.addr)
    }
}
