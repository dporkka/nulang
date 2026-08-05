# RFC 0013: Authenticated, Encrypted Transport

- **Status:** Implemented
- **Tier:** Stable (extends Stable-tier cluster surface)
- **Author:** AI assistant
- **Created:** 2026-08-04
- **Language-version at effect:** 1.0.0-frozen
- **Supersedes:** none

## Summary

Replace the current plaintext-default, MITM-able TLS stub with real mutual TLS authentication and encryption. The NUL0 wire protocol handshake format is unchanged (version stays at 1): authentication is provided by the TLS layer, and node identity is derived from the certificate fingerprint rather than the spoofable socket-address hash.

## Motivation

Today's cluster transport has three security gaps:

1. **Plaintext by default.** `TlsConfig::SelfSigned` exists but is never constructed anywhere in the repository. `enable_distribution`'s `Option<TlsConfig>` parameter is passed `None` at every call site, silently meaning "no encryption, no authentication."

2. **No certificate verification.** Even if constructed, the client installs a `NoVerification` verifier (`network.rs:93-132` — accepts any certificate) — the TLS layer is pure overhead with zero security benefit.

3. **Spoofable node identity.** `NodeId` is `DefaultHasher(SocketAddr)` — any attacker who can bind to a known peer's address can claim that peer's identity with no cryptographic evidence.

## Design

### 1. `TlsConfig` — explicit security posture

The old `Option<TlsConfig>` pattern (where `None` silently means "no security") is replaced with a required `TlsConfig` value carrying two variants:

```rust
pub enum TlsConfig {
    /// Mutual TLS with a cluster CA.
    MutualTls {
        ca_cert_pem: Vec<u8>,
        server_cert_pem: Vec<u8>,
        server_key_pem: Vec<u8>,
    },
    /// Plaintext, no encryption. Explicit opt-out.
    PlaintextInsecure,
}
```

`enable_distribution` now requires a `TlsConfig`, not an `Option<TlsConfig>`. Existing `None` callers migrate to `TlsConfig::PlaintextInsecure` — same behavior, explicit name.

### 2. Certificate-derived node identity

When `TlsConfig::MutualTls` is active, the node's identity is derived from the BLAKE3 hash (truncated to 64 bits) of its X.509 server certificate's DER encoding:

```rust
impl NodeId {
    pub fn from_cert_der(cert_der: &[u8]) -> Self {
        let hash = blake3::hash(cert_der);
        NodeId(u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap()))
    }
}
```

This identity:
- Cannot be spoofed without the node's private key.
- Is stable across restarts (same cert → same id).
- Is collision-resistant: 64-bit BLAKE3 truncation has negligible collision probability at any realistic cluster size.

When `PlaintextInsecure` is active, identity falls back to the `DefaultHasher(SocketAddr)` method (backward compatible).

### 3. Certificate verification at connection time

Both sides of a MutualTLS connection present certificates signed by the same cluster CA. Verification happens at two layers:

**TLS layer** (before NUL0 handshake):
- The server requires client authentication and verifies the client certificate against the CA.
- The client verifies the server certificate against the CA.
- `rustls` built-in `WebPkiClientVerifier` and `with_root_certificates` replace the old `NoVerification` stub.

**NUL0 layer** (after TLS handshake):
- The handshake's `node_id` field is verified against the TLS peer certificate's fingerprint.
- On the client side (outbound `connect`): if the peer's cert fingerprint does not match the expected `node_id`, the connection is refused with "TLS cert identity mismatch."
- On the server side (inbound `connection_reader`): same check logged as a warning and the connection dropped.

### 4. No NUL0 wire format change

The NUL0 handshake format (`magic[4] + version[4] + node_id[8]`) is unchanged. Authentication is provided by the TLS layer beneath NUL0; the wire version stays at 1. A node configured with MutualTLS refuses plaintext connections (the listener only accepts TLS), and a plaintext node refuses TLS connections (the listener only accepts raw TCP) — they cannot accidentally interoperate. This has the same operational property as a version bump without changing any wire bytes.

### 5. Certificate provisioning

The operator is responsible for:
1. Generating a cluster CA certificate and key.
2. Generating a server certificate and key for each node, signed by the CA.
3. Distributing the CA cert, server cert, and server key to each node.

Standard tools (`openssl`, `step`, `certstrap`) can generate these. The PEM format is accepted directly; no runtime certificate generation (the old `rcgen::generate_simple_self_signed` path) is performed.

## Backwards Compatibility

- The NUL0 wire protocol format is unchanged (version 1).
- Existing `None`-passing call sites migrate to `TlsConfig::PlaintextInsecure` — no behavioral change.
- `NodeId::new(&addr)` is preserved for plaintext mode.
- The `NetworkTransport` trait signature is unchanged.

## Migration Path

1. **Development/testing clusters:** migrate `None` → `TlsConfig::PlaintextInsecure`. No operational change.
2. **Production clusters:** generate CA + per-node certs, deploy `TlsConfig::MutualTls{...}`.
3. **Mixed clusters:** not supported — a MutualTLS node cannot talk to a PlaintextInsecure node (by design: mixing authenticated and unauthenticated peers in one cluster is a security anti-pattern).

## Open Questions

- Should we add a `PreSharedKey` variant for clusters that want encryption without a PKI? (Out of scope for this RFC; can be added as a future additive change.)
- Should `rcgen` be removed from dependencies now that `SelfSigned` is gone? (It's still used by `quic_transport.rs`; removal is covered by Phase 5 deliverable 5 — QUIC's fate.)
