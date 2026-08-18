//! Prometheus-format metrics export for VictoriaMetrics / Prometheus scraping.
//!
//! A lightweight TCP server that serves `GET /metrics` in Prometheus
//! exposition format.  No external dependencies — pure `std::net::TcpListener`
//! on a background thread.  The scheduler thread periodically calls
//! [`Runtime::publish_metrics`] to push the latest snapshot into a shared
//! buffer; the server thread serves whichever snapshot it last received.
#[cfg(feature = "tcp")]
use std::net::TcpListener;

#[cfg(feature = "tcp")]
use std::io::Write;
#[cfg(feature = "tcp")]
use std::sync::{Arc, Mutex};
#[cfg(feature = "tcp")]
use std::thread::{self, JoinHandle};
#[cfg(feature = "tcp")]
use std::time::Duration;

use super::MetricsSnapshot;

/// Start a background Prometheus-format metrics server on `port`.
///
/// Returns a handle and a shared buffer.  The caller should periodically
/// call `publish(snapshot)` to push the latest snapshot; the server
/// thread serves the most recently published snapshot.
#[cfg(feature = "tcp")]
pub struct MetricsServer {
    #[allow(dead_code)]
    handle: JoinHandle<()>,
    buffer: Arc<Mutex<String>>,
}

#[cfg(feature = "tcp")]
impl MetricsServer {
    /// Bind and start serving on `0.0.0.0:<port>`.
    pub fn start(port: u16) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        listener.set_nonblocking(false)?;
        let buffer = Arc::new(Mutex::new(String::from(
            "# Nulang metrics server starting up — no snapshot yet\n",
        )));
        let buf = buffer.clone();

        let handle = thread::Builder::new()
            .name("nulang-metrics".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(mut s) => {
                            let _ = s.set_read_timeout(Some(Duration::from_secs(1)));
                            let body = buf.lock().unwrap().clone();
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = s.write_all(response.as_bytes());
                        }
                        Err(_) => break,
                    }
                }
            })?;

        Ok(MetricsServer { handle, buffer })
    }

    /// Publish a new snapshot.  Thread-safe — call from the scheduler thread.
    pub fn publish(&self, text: String) {
        *self.buffer.lock().unwrap() = text;
    }
}

impl MetricsSnapshot {
    /// Format this snapshot as Prometheus exposition text.
    ///
    /// VictoriaMetrics and Prometheus both natively scrape this format.
    pub fn to_prometheus_text(&self) -> String {
        let mut out = String::new();

        // Gauge: live actor count
        out.push_str("# HELP nulang_actors_live Number of living actors\n");
        out.push_str("# TYPE nulang_actors_live gauge\n");
        out.push_str(&format!("nulang_actors_live {}\n", self.actors_live));

        // Gauge: DLQ depth
        out.push_str("# HELP nulang_dlq_depth Dead-letter queue depth\n");
        out.push_str("# TYPE nulang_dlq_depth gauge\n");
        out.push_str(&format!("nulang_dlq_depth {}\n", self.dlq_depth));

        // Gauge: per-actor mailbox depths (top 50 by depth)
        out.push_str("# HELP nulang_actor_mailbox_depth Per-actor mailbox depth\n");
        out.push_str("# TYPE nulang_actor_mailbox_depth gauge\n");
        let mut sorted: Vec<_> = self.actors_mailboxes.clone();
        sorted.sort_by_key(|m| -(m.depth as i64));
        for m in sorted.iter().take(50) {
            out.push_str(&format!(
                "nulang_actor_mailbox_depth{{actor_id=\"{}\"}} {}\n",
                m.actor_id, m.depth
            ));
        }

        // Scheduler counters
        let s = &self.scheduler;
        macro_rules! counter {
            ($name:expr, $help:expr, $val:expr) => {
                out.push_str(concat!("# HELP ", $name, " ", $help, "\n"));
                out.push_str(concat!("# TYPE ", $name, " counter\n"));
                out.push_str(&format!(concat!($name, " {}\n"), $val));
            };
        }
        counter!(
            "nulang_scheduler_tasks_total",
            "Total tasks processed",
            s.total_tasks_processed
        );
        counter!(
            "nulang_scheduler_tasks_local",
            "Tasks from local queue",
            s.tasks_from_local_queue
        );
        counter!(
            "nulang_scheduler_tasks_global",
            "Tasks from global queue",
            s.tasks_from_global_queue
        );
        counter!(
            "nulang_scheduler_tasks_stolen",
            "Tasks stolen from other workers",
            s.tasks_from_steal
        );
        counter!(
            "nulang_scheduler_steal_attempts",
            "Steal attempts",
            s.steal_attempts
        );
        counter!(
            "nulang_scheduler_steal_successes",
            "Successful steals",
            s.steal_successes
        );
        counter!(
            "nulang_scheduler_empty_polls",
            "Empty polls (no work found)",
            s.empty_polls
        );

        // GC counters
        let g = &self.gc;
        counter!(
            "nulang_gc_objects_allocated",
            "Objects allocated",
            g.objects_allocated
        );
        counter!("nulang_gc_objects_freed", "Objects freed", g.objects_freed);
        counter!(
            "nulang_gc_bytes_allocated",
            "Bytes allocated",
            g.bytes_allocated
        );
        counter!("nulang_gc_bytes_freed", "Bytes freed", g.bytes_freed);
        counter!(
            "nulang_gc_cycles_detected",
            "ORCA cycles detected",
            g.cycles_detected
        );

        // Resolver counters
        let r = &self.resolver;
        counter!(
            "nulang_resolver_local_resolves",
            "Local address resolutions",
            r.local_resolves
        );
        counter!(
            "nulang_resolver_remote_resolves",
            "Remote address resolutions",
            r.remote_resolves
        );
        counter!(
            "nulang_resolver_failed_resolves",
            "Failed resolutions",
            r.failed_resolves
        );
        counter!(
            "nulang_resolver_cache_hits",
            "Remote actor cache hits",
            r.cache_hits
        );
        counter!(
            "nulang_resolver_cache_misses",
            "Remote actor cache misses",
            r.cache_misses
        );

        out
    }
}

#[cfg(not(feature = "tcp"))]
pub struct MetricsServer {
    // Dummy: the `tcp` feature is disabled, so no server can start.
}

#[cfg(not(feature = "tcp"))]
impl MetricsServer {
    pub fn start(_port: u16) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "metrics server disabled (feature 'tcp' not enabled)",
        ))
    }

    /// Publish a new snapshot (no-op: no server is running).
    pub fn publish(&self, _text: String) {}
}
