# Nulang Runtime Observability Demo

Grafana dashboard + Prometheus scrape config for the runtime metrics the
`nulang` binary already exports (PLAN.md Phase 5 deliverable 18, the
demo-facing consumer of deliverable 17's `--metrics-port` exporter).

## What's exported

Run any program with `--metrics-port <N>` and the runtime serves
Prometheus exposition format at `GET /metrics` on `0.0.0.0:<N>`
(`src/runtime/metrics.rs`). Exported metrics:

- **Gauges** — `nulang_actors_live`, `nulang_dlq_depth`,
  `nulang_actor_mailbox_depth{actor_id="…"}`.
- **Scheduler counters** — `nulang_scheduler_tasks_total/local/global/stolen`,
  `nulang_scheduler_steal_attempts/successes`, `nulang_scheduler_empty_polls`.
- **GC counters** — `nulang_gc_objects_allocated/freed`,
  `nulang_gc_bytes_allocated/freed`, `nulang_gc_cycles_detected`.
- **Resolver counters** — `nulang_resolver_local_resolves/remote_resolves/
  failed_resolves/cache_hits/cache_misses`.

These are runtime metrics only. The `.nula`-level `metrics.counter`/
`histogram`/`gauge`/`timer` effects are planned surface (backlog, SPEC2
§15.3), not exported today.

## Run it

1. Start a nulang program with metrics enabled:

   ```bash
   cargo run --release -- --metrics-port 9100 examples/…   # or any .nula program
   ```

2. Bring up Prometheus + Grafana:

   ```bash
   cd deploy/observability
   docker compose up -d
   ```

3. Open Grafana at http://localhost:3000 (anonymous admin access is
   enabled by this demo config). The "Nulang Runtime" dashboard is
   provisioned automatically.

## Reaching the host

The scrape target is `host.docker.internal:9100` (the host running
`nulang`). On Docker Desktop this resolves out of the box; the compose
file also adds the `host-gateway` extra_host so it works on Linux
without Docker Desktop.

If `nulang` runs on a different host/port, edit
`prometheus/prometheus.yml` and add one target per node:

```yaml
- job_name: nulang
  static_configs:
    - targets:
        - "node-a.example:9100"
        - "node-b.example:9100"
```

## Not included (by design)

No bespoke dashboard backend — this is off-the-shelf Grafana pointed at
the existing exporter, per PLAN.md D18's "point Grafana at deliverable
17's Prometheus exporter" framing. Alerts, multi-cluster templates, and
the `.nula` metric effects are out of scope here.
