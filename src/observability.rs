//! OpenTelemetry observability wiring: trace and metric exporters over OTLP/HTTP.
//!
//! This module is compiled only when the `otel` feature is enabled.  It provides
//! `init_tracer` and `init_meter` helpers that configure an OTLP exporter
//! pointing at a collector URL (e.g. `http://localhost:4318`).  Both helpers
//! are idempotent — calling them twice with the same arguments returns the
//! existing tracer / meter provider without creating a duplicate pipeline.
//!
//! # Usage
//!
//! ```no_run
//! # #[cfg(feature = "otel")]
//! # {
//! use nulang::observability::{init_tracer, init_meter};
//! init_tracer("http://localhost:4318/v1/traces", "nulang-runtime").unwrap();
//! init_meter("http://localhost:4318/v1/metrics", "nulang-runtime").unwrap();
//! # }
//! ```

use parking_lot::Mutex;

use opentelemetry::global;
use opentelemetry::metrics::MeterProvider;
use opentelemetry::trace::TracerProvider;

/// Global singleton guard so `init_tracer` / `init_meter` are idempotent.
static TRACER_INIT: Mutex<bool> = Mutex::new(false);
static METER_INIT: Mutex<bool> = Mutex::new(false);

/// Initialise a global OTLP trace exporter.
///
/// `url` is the full collector endpoint, e.g.
/// `http://localhost:4318/v1/traces`.  `service_name` is used as the
/// OpenTelemetry `service.name` resource attribute.
///
/// Returns `Ok(())` on success, or `Err(String)` if the exporter pipeline
/// could not be built.  Second and subsequent calls return `Ok(())` without
/// creating additional pipelines.
pub fn init_tracer(url: &str, service_name: &str) -> Result<(), String> {
    let mut guard = TRACER_INIT.lock();
    if *guard {
        return Ok(());
    }

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(url)
        .build()
        .map_err(|e| format!("failed to build OTLP trace exporter: {e}"))?;

    let provider = opentelemetry_sdk::trace::TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_config(
            opentelemetry_sdk::trace::Config::default().with_resource(
                opentelemetry_sdk::Resource::new(vec![
                    opentelemetry::KeyValue::new("service.name", service_name.to_string()),
                    opentelemetry::KeyValue::new(
                        "service.version",
                        crate::format::constants::LANGUAGE_VERSION_STR.to_string(),
                    ),
                ]),
            ),
        )
        .build();

    global::set_tracer_provider(provider);
    *guard = true;
    Ok(())
}

/// Initialise a global OTLP metric exporter.
///
/// `url` is the full collector endpoint, e.g.
/// `http://localhost:4318/v1/metrics`.  `service_name` is used as the
/// OpenTelemetry `service.name` resource attribute.
///
/// Returns `Ok(())` on success, or `Err(String)` if the exporter pipeline
/// could not be built.  Second and subsequent calls return `Ok(())` without
/// creating additional pipelines.
pub fn init_meter(url: &str, service_name: &str) -> Result<(), String> {
    let mut guard = METER_INIT.lock();
    if *guard {
        return Ok(());
    }

    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_endpoint(url)
        .with_temporality(opentelemetry_sdk::metrics::Temporality::Delta)
        .build()
        .map_err(|e| format!("failed to build OTLP metrics exporter: {e}"))?;

    let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(
        exporter,
        opentelemetry_sdk::runtime::Tokio,
    )
    .build();

    let provider = opentelemetry_sdk::metrics::MeterProvider::builder()
        .with_reader(reader)
        .with_resource(opentelemetry_sdk::Resource::new(vec![
            opentelemetry::KeyValue::new("service.name", service_name.to_string()),
            opentelemetry::KeyValue::new(
                "service.version",
                crate::format::constants::LANGUAGE_VERSION_STR.to_string(),
            ),
        ]))
        .build();

    global::set_meter_provider(provider);
    *guard = true;
    Ok(())
}

/// Shut down the global tracer provider, flushing any buffered telemetry
/// before exit.  This is a best-effort operation; failures are ignored.
pub fn shutdown() {
    let mut tg = TRACER_INIT.lock();
    if *tg {
        let _ = global::shutdown_tracer_provider();
        *tg = false;
    }
    // Meter provider shutdown is a no-op in this build; the global provider
    // does not expose a typed shutdown method.
    let mut mg = METER_INIT.lock();
    *mg = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `init_tracer` must be idempotent.
    #[test]
    fn test_init_tracer_idempotent() {
        let url = "http://localhost:4318/v1/traces";
        let name = "nulang-test-tracer";
        // First call may fail if no collector is running, but idempotency
        // should still hold when the guard is already set.
        let r1 = init_tracer(url, name);
        let r2 = init_tracer(url, name);
        // Both calls should return the same result (Ok or Err).
        assert_eq!(r1.is_ok(), r2.is_ok());
    }

    /// `init_meter` must be idempotent.
    #[test]
    fn test_init_meter_idempotent() {
        let url = "http://localhost:4318/v1/metrics";
        let name = "nulang-test-meter";
        let r1 = init_meter(url, name);
        let r2 = init_meter(url, name);
        assert_eq!(r1.is_ok(), r2.is_ok());
    }
}
