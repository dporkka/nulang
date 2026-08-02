//! Actor throughput benchmarks.

use criterion::{black_box, criterion_group, Criterion};
use nulang::runtime::Runtime;
use nulang::vm::Value;

fn bench_spawn_send_receive(c: &mut Criterion) {
    c.bench_function("actor/spawn_send_receive", |b| {
        b.iter(|| {
            let mut rt = Runtime::new();
            let actor_id = rt.spawn_actor(Box::new(|| vec![]));
            let msg = Value::int(42);
            rt.send_message(actor_id, "handle", &[msg]);
            for _ in 0..20 {
                rt.run_scheduler();
                rt.process_gc_ops();
            }
            black_box(actor_id);
        })
    });
}

fn bench_message_throughput(c: &mut Criterion) {
    c.bench_function("actor/message_throughput", |b| {
        b.iter(|| {
            let mut rt = Runtime::new();
            let consumer = rt.spawn_actor(Box::new(|| vec![]));
            let msg = Value::int(1);
            for _ in 0..100 {
                rt.send_message(consumer, "handle", &[msg]);
            }
            for _ in 0..200 {
                rt.run_scheduler();
                rt.process_gc_ops();
            }
            black_box(consumer);
        })
    });
}

criterion_group!(benches, bench_spawn_send_receive, bench_message_throughput);
