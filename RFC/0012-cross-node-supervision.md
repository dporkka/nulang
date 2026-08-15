# RFC 0012: Cross-Node Actor Supervision

- **Status:** Implemented
- **Tier:** Stable (extends Stable-tier actor surface)
- **Author:** AI assistant
- **Created:** 2026-08-03
- **Language-version at effect:** 1.0.0-frozen
- **Implemented:** 2026-08-04 (commit `0ab2c42`)
- **Supersedes:** none

## Summary
Extend `Actor.link` and `Actor.monitor` to support cross-node targets. Currently, links and monitors are strictly local-node; remote actor references have no supervision semantics. This RFC enables supervising remote actors, ensuring that failure of a remote actor propagates `DOWN` signals to local linked/monitored actors.

## Motivation
The current supervision system is strictly node-local. While the language surface allows `Actor.link`, it only functions for local actors. Distributed systems (Nulang's primary target) suffer from silent failure when a remote actor crashes, as the link remains broken without notification.

## Design
1.  **ActorAddress propagation**: The VM/runtime must allow linking/monitoring to `ActorAddress::Remote`.
2.  **Tracking**: The `Runtime` must track cross-node `link` and `monitor` registrations.
3.  **Propagation Protocol**: When a remote actor exits (or its node is declared `Failed`), the monitoring runtime sends a new `Packet::Down` (or similar) to the watchers' nodes.
4.  **Signal Delivery**: Watchers' nodes translate the network packet into the local `DOWN` signal.

## Backwards Compatibility
- Purely additive: existing local-only `link`/`monitor` behavior is preserved.
- No new NUL0 wire protocol variant is strictly required (reuse existing message transport if possible, or new `Packet` variant for supervision if needed).

## Open Questions
- Protocol overhead of cross-node link propagation.
- Handling of network partitions during supervision.
