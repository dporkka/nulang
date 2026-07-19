# RFC 0001: Format Stability

- **Status:** Implemented
- **Tier:** Frozen
- **Author:** David Porkka
- **Created:** 2026-07-19
- **Resolved:** 2026-07-19 (Accepted)
- **Language-version at effect:** 1.0-frozen
- **Supersedes:** none
- **Superseded by:** none

## Summary

Establishes versioned, frozen binary formats for Nulang's durable artifacts
(`.nbc` bytecode) and wire protocol (NUL0), with a magic + version header on
every artifact and connection, a named-error rejection policy for unknown
versions, and a migration registry as the sole legal home for format upgrades.

## Motivation

`grep` for `MAGIC|VERSION` in `src/bytecode.rs`, `src/runtime/network.rs`,
`src/value_layout.rs` previously returned nothing. The wire protocol had a
packet-level magic (`NUL0`) but no protocol-version field and no handshake
magic. The bytecode was an in-memory `Vec<Instruction>` with no on-disk
header. Any change to the opcode table or `Packet` enum silently broke every
compiled artifact and every peer. There was no way to run a Nulang program
compiled in 2026 on a runtime built in 2126.

Every long-lived binary format (ELF, PE, WAV, PNG, Java classfile) has a
magic + version header and a documented stability contract. This RFC
establishes that contract for Nulang.

## Design

- New module `src/format/` with:
  - `constants.rs`: `BYTECODE_MAGIC = "NLBC"`, `BYTECODE_VERSION = 1`,
    `BYTECODE_MAX_VERSION = 1`, `WIRE_MAGIC = "NUL0"`, `WIRE_VERSION = 1`,
    `VALUE_LAYOUT_VERSION = 1`, `LANGUAGE_VERSION = 1`, and `FormatError`.
  - `nbc.rs`: `CodeModule::to_nbc(source_hash) -> Vec<u8>` and
    `CodeModule::from_nbc(bytes) -> Result<NbcArtifact, FormatError>`.
  - `migrate.rs`: `migrate_nbc(bytes, target)` — the only legal home for
    format upgrades; v1→v1 identity today.
- `.nbc` byte layout (SPEC2 §FS.2): binary header (magic, format_version,
  language_version, source_hash, instr_count) + binary instruction stream
  (4 bytes/instruction via `Instruction::encode`) + JSON metadata body.
- NUL0 handshake (SPEC2 §FS.3): 16 bytes `{magic "NUL0", version u32,
  node_id u64}`, replacing the prior 8-byte node-id-only handshake. Three
  call sites updated in `src/runtime/network.rs` via `write_handshake`/
  `read_handshake` helpers.
- `SPEC2.md` §"Format Stability" added as the stability contract.
- 17 unit tests in `src/format/` covering round-trip, header validation,
  unknown-version rejection, unknown-opcode rejection, non-finite-float
  rejection; 26 existing network tests pass under the new handshake.

## Tier Classification

Frozen. This RFC establishes the Frozen tier itself and assigns version 1 of
the bytecode and wire formats to it. No deprecation path (these formats did
not previously exist as versioned artifacts). Language version 1.0-frozen.

## Backwards Compatibility

The wire handshake changes from 8 bytes to 16 bytes. This is a
wire-incompatible change between an upgraded node and a pre-RFC node. Within
the v0.13-alpha series this is acceptable (pre-1.0, no stability promise
existed). From language version 1.0 forward, the handshake is frozen. A
future v2 wire protocol would require a new RFC and a `migrate` step; v1
nodes would refuse v2 peers rather than reinterpret.

## Alternatives Considered

- **Serde-bincode body:** rejected — couples the durable format to the
  bincode crate, which may not exist in 2226. JSON body is universally
  parseable.
- **All-binary body (hand-rolled):** rejected for the metadata — too much
  error-prone hand-rolling for `ActorMeta`, `HandlerTable`, etc. The
  instruction stream IS binary (4 bytes/instruction) for compactness and
  opcode-value coupling.
- **Version negotiation packet:** deferred. v1 is the only version; a
  `Packet::VersionNegotiate` variant is a future additive extension when v2
  exists. For now, mismatch → refuse.

## Open Questions

None blocking. A compact-binary metadata encoding is a candidate v2
additive extension.

## Resolution

Accepted 2026-07-19. Implemented in `src/format/` with 17 passing tests and
26 passing network tests. Took effect at language version 1.0-frozen. The
SPEC2 §"Format Stability" chapter is the authoritative contract.
