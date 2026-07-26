//! Distribution benchmarks: CRDT delta sync, gossip convergence.

use criterion::{black_box, criterion_group, Criterion};

fn bench_dist_crdt_delta_sync(c: &mut Criterion) {
    c.bench_function("dist/crdt_delta_sync", |b| {
        b.iter(|| {
            use nulang::runtime::crdt::GCounter;
            let mut counter = GCounter::new(1);
            counter.increment();
            counter.increment();
            let base = GCounter::new(2);
            let delta = counter.delta_since(&base);
            black_box(delta);
        })
    });
}

fn bench_dist_gossip_convergence(c: &mut Criterion) {
    c.bench_function("dist/gossip_convergence", |b| {
        b.iter(|| {
            use nulang::runtime::cluster::{ClusterState, NodeGossip, NodeId, NodeStatus};
            use std::net::SocketAddr;
            let addr: SocketAddr = "127.0.0.1:9001".parse().unwrap();
            let node_id = NodeId::new(&addr);
            let mut cs = ClusterState::new(node_id, addr);
            let gossip: Vec<NodeGossip> = (2..=5)
                .map(|n| {
                    let a: SocketAddr = format!("127.0.0.1:900{}", n).parse().unwrap();
                    NodeGossip {
                        node_id: NodeId::new(&a),
                        address: a.to_string(),
                        status: NodeStatus::Healthy,
                        incarnation: 1,
                    }
                })
                .collect();
            cs.merge_membership(gossip);
            black_box(cs);
        })
    });
}

criterion_group!(
    benches,
    bench_dist_crdt_delta_sync,
    bench_dist_gossip_convergence
);
