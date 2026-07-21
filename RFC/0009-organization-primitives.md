# RFC 0009: Organization Primitives

- **Status:** Draft
- **Tier:** Stable
- **Author:** AI assistant review
- **Created:** 2026-07-21
- **Resolved:** (pending)
- **Language-version at effect:** 2.0 (planned)
- **Supersedes:** none
- **Superseded by:** none

## Summary

Define Stable language and runtime support for `organization`, a composite identity that groups multiple entities into a single durable unit. An organization has its own stable identity, a set of member entities, a shared event journal for membership changes, and a governance model that controls how members are added, removed, and delegated authority. Organizations enable the construction of autonomous organizations (DAOs, multi-agent teams, federated services) from the same primitives as individual entities.

## Motivation

Nulang's durable computation model targets long-lived software entities. In practice, many important systems are not single entities but coordinated groups: a supply-chain workflow composed of buyer, seller, shipper, and insurer entities; a multi-agent team with specialized AI workers; a decentralized autonomous organization with voting members. The user's strategic brief explicitly asks for "organization graphs" and "agent employment contracts" as primitives.

Without a language-level organization construct, programmers must build these groups ad-hoc from actor references, process groups, and external registries. This leads to inconsistent handling of membership, lifecycle, and authority. An `organization` primitive gives Nulang a coherent model for composite durable identities.

## Design

### 1. `organization` keyword

An `organization` is a special durable entity that manages a set of members:

```nulang
organization SupplyChain {
    state members: Map[String, ActorRef] = {}

    events
        | MemberJoined(role: String, ref: ActorRef)
        | MemberLeft(role: String)
        | AuthorityDelegated(from: String, to: String, capability: String)

    behavior join(role: String, ref: ActorRef) {
        emit MemberJoined(role, ref)
        self.members = Map.insert(self.members, role, ref)
    }

    behavior leave(role: String) {
        emit MemberLeft(role)
        self.members = Map.remove(self.members, role)
    }

    behavior shipper() {
        Map.get(self.members, "shipper")
    }
}
```

Rules:

- An `organization` is an `entity` with additional structure: a member map, membership events, and governance rules.
- It has all the properties of entities: durable by default, event-sourced state, stable identity, migration contracts.
- Members are referenced by `ActorRef` (a sendable capability, see §3).
- Organizations can spawn and supervise member entities.

### 2. Member lifecycle and supervision

Organizations supervise their members. When an organization is spawned, it may declare an initial member template:

```nulang
let org = spawn SupplyChain {} named "supply-chain:acme-corp"
let buyer = spawn Buyer {} in org
let seller = spawn Seller {} in org
ask org join("buyer", buyer)
ask org join("seller", seller)
```

Rules:

- `spawn Entity {} in org` creates the entity as a child of the organization.
- If the organization terminates or is migrated, its members are notified through links/monitors (existing OTP primitives).
- Members can outlive the organization if explicitly detached.
- Organizations can define restart strategies for failed members, reusing the existing supervisor machinery.

### 3. Governance and authority

Organizations can define governance rules that constrain how members act on behalf of the organization:

```nulang
organization SupplyChain {
    governance {
        // Only the "buyer" member can emit PurchaseOrder events.
        authorize PurchaseOrder.emit(role) => role == "buyer"

        // Two-of-three signatures required to move funds.
        authorize Treasury.withdraw(amount) => signatures >= 2
    }
}
```

Rules:

- Governance rules are pure predicates evaluated by the runtime when a member attempts an action.
- They can inspect the member's role, the organization's state, the action being performed, and any attached signatures/votes.
- Governance is enforced by the runtime, not just by convention.
- The exact syntax and authorization effect model are **Planned**; this RFC establishes the requirement and initial syntax.

### 4. Shared event journal

An organization maintains a shared event journal for membership and governance events. This is in addition to each member's own journal:

- `MemberJoined`, `MemberLeft`, `AuthorityDelegated`, `GovernanceChanged`.
- The organization journal is replicated and merged across nodes using the same mechanisms as entity journals.
- Members can read the organization journal via a future Stable effect or Cloud SDK library.

### 5. Employment contracts (membership contracts)

The user's strategic brief mentions "agent employment contracts." In Nulang, a membership contract is a record attached to a member describing its role, obligations, and rewards:

```nulang
contract Member {
    role: String,
    obligations: List[Obligation],
    rewards: RewardPolicy
}
```

Rules:

- Contracts are **Planned** language surface. The initial implementation may treat them as ordinary event-sourced state in the organization.
- Contracts can be enforced by governance rules (e.g., a member must fulfill obligations before receiving rewards).
- They are a good fit for the Cloud SDK (`nlc.contracts`) until the syntax stabilizes.

### 6. Nested organizations

Organizations can contain other organizations, forming a tree:

```nulang
let division = spawn Division {} in org
let team = spawn Team {} in division
```

Rules:

- Nested organizations follow the same supervision and governance rules as member entities.
- The identity of a nested organization is stable and composed of its parent path.

### 7. Interaction with the capability system

- `ActorRef` values held by an organization must be `tag` or `val` capabilities (opaque or immutable) to prevent data races.
- Governance rules may delegate authority to members by issuing capability tokens, recorded in the organization journal.
- The existing capability lattice (`iso`, `trn`, `ref`, `val`, `box`, `tag`, `lineariso`) applies to organizations and their members.

### 8. Implementation targets

- `src/ast.rs`: Add `Decl::Organization` or desugar to `Decl::Entity` with organization markers.
- `src/parser.rs`: Parse `organization`, `governance`, and `contract` blocks.
- `src/typechecker.rs`: Type-check organization members and governance predicates.
- `src/effect_checker.rs`: Add organization effects (`Org.join`, `Org.leave`, `Org.authorize`).
- `src/hir_lower.rs`: Desugar `organization` to `entity` plus member map and governance hooks.
- `src/runtime/actor.rs`: Extend actors to track parent organization and member list.
- `src/runtime/supervisor.rs`: Support organization-wide restart strategies.
- `src/runtime/persistence.rs`: Persist organization journals and member metadata.
- `src/stdlib.rs`: Register organization effects.

### 9. Example: multi-agent team

```nulang
organization ResearchTeam {
    state lead: Option[ActorRef] = None
    state reviewers: List[ActorRef] = []

    events
        | LeadAssigned(ref: ActorRef)
        | ReviewerAdded(ref: ActorRef)
        | PaperPublished(title: String)

    behavior set_lead(ref: ActorRef) {
        emit LeadAssigned(ref)
        self.lead = Some(ref)
    }

    behavior add_reviewer(ref: ActorRef) {
        emit ReviewerAdded(ref)
        self.reviewers = List.append(self.reviewers, ref)
    }

    behavior publish(title: String) {
        // Governance could require lead + one reviewer signature.
        emit PaperPublished(title)
    }
}
```

## Tier Classification

- **Tier:** Stable.
- **Frozen Core impact:** None.
- **Breaking change:** No. `organization` is additive.
- **Relationship to other RFCs:** Builds on RFC 0005 (Durable Entities), RFC 0007 (Event Sourcing Primitives), and RFC 0008 (Migration Contracts). Governance and contracts may start as Cloud SDK features and be promoted later.

## Backwards Compatibility

This RFC is additive. Existing actors, entities, and Cloud SDK code are unaffected. Organizations desugar to entities with extra state and effects.

## Alternatives Considered

1. **Build organizations entirely in the Cloud SDK without language syntax.** Rejected because the concept is fundamental to durable composite systems and deserves a stable, first-class primitive.
2. **Use process groups (`process_groups.rs`) as organizations.** Rejected because process groups are ephemeral runtime constructs without durable identity, event journals, or governance.
3. **Make organizations a separate top-level construct outside the actor model.** Rejected because that would duplicate the concurrency/durability machinery. Organizations should reuse entities.
4. **Include governance and contracts in Stable immediately.** Rejected because the exact enforcement model needs experimentation; governance rules may start as library hooks and be promoted to Stable syntax.

## Open Questions

1. Should `organization` desugar to `entity` with extra fields, or be a distinct AST/runtime construct? Desugaring is preferred for consistency.
2. Should governance rules be pure Nulang predicates, or a separate policy language? Pure Nulang is consistent with the rest of the language.
3. How are organization membership and governance events exposed to external auditors? A Cloud SDK observability package is the likely path.
4. Should organizations support voting/quorum mechanisms in Stable, or leave that to libraries?

## Resolution

(To be filled on accept/reject.)
