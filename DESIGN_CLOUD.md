# Nulang Cloud Platform Design Document

## Overview

The Nulang Cloud Platform (`nulang-cloud`) is a fully managed, serverless cloud platform designed specifically for deploying and running Nulang applications at scale. Inspired by Cloudflare Workers, Deno Deploy, and Fly.io, the platform leverages Nulang's actor model to provide a natural, zero-configuration deployment experience where each actor can be independently scaled, migrated, and persisted. The platform handles infrastructure concerns — compute, storage, networking, and observability — so developers can focus purely on application logic.

**Version:** 1.0.0  
**Status:** Design Complete — Ready for Implementation  
**Target Nulang Edition:** 2024

---

## Table of Contents

1. [Core Concepts](#1-core-concepts)
2. [Architecture Overview](#2-architecture-overview)
3. [API Design & Specification](#3-api-design--specification)
4. [Module Reference](#4-module-reference)
5. [Implementation Phases](#5-implementation-phases)
6. [Appendices](#6-appendices)

---

## 1. Core Concepts

### 1.1 Actor-as-a-Service

At the heart of the Nulang Cloud Platform is **Actor-as-a-Service**, the idea that Nulang's fundamental concurrency primitive — the actor — is also the fundamental deployment primitive. When you deploy a Nulang application to the cloud, each actor type becomes an independently scalable service.

```nulang
// Local development — actors run in a single process
let system = ActorSystem.new()
  |> ActorSystem.spawn(ChatRoom, name: "lobby")
  |> ActorSystem.spawn(ChatRoom, name: "support")
  |> ActorSystem.spawn(UserPresence, name: "presence")

// Cloud deployment — the same code, but each actor type
// becomes a distributed, auto-scaled service
cloud deploy ./my-app --region us-east, eu-west
```

Key properties:
- **Location Transparency**: Sending a message to a local actor and a cloud actor uses the same syntax
- **Independent Scaling**: Each actor type scales based on its own message queue depth and CPU usage
- **Zero Cold Start**: Frequently used actors are kept warm; idle actors hibernate and resume on message arrival
- **State Migration**: Stateful actors can be migrated between nodes without losing state

### 1.2 Deployment Unit

A **Deployment Unit** is the atomic unit of deployment. It maps to a Nulang module or application and contains:

- Compiled Nulang bytecode
- Actor type definitions
- Static assets
- Environment configuration
- Resource specifications

```nulang
// cloud.nl — Deployment manifest
cloud {
  name: "chat-app",
  version: "1.2.3",
  
  // Compute configuration
  compute: {
    // Each actor type can have its own resource spec
    ChatRoom: { memory: "256MB", cpu: "0.5", min_instances: 2 },
    UserPresence: { memory: "128MB", cpu: "0.25" },
    MessageRouter: { memory: "512MB", cpu: "1.0", max_instances: 10 }
  },
  
  // Environment variables
  env: {
    DATABASE_URL: env("DATABASE_URL"),
    REDIS_URL: env("REDIS_URL"),
    API_SECRET: secret("api-secret")  // Encrypted
  },
  
  // Static assets
  assets: {
    "public/**/*": { cache: "1h" },
    "assets/**/*": { cache: "1d", fingerprint: true }
  },
  
  // Regions for deployment
  regions: ["us-east", "us-west", "eu-west", "ap-south"],
  
  // Routing rules
  routes: [
    { path: "/api/*", target: ChatRoom },
    { path: "/ws/*", target: MessageRouter, websocket: true },
    { path: "/*", target: StaticAssets }
  ]
}
```

### 1.3 Stateful Migration

**Stateful Migration** is the platform's ability to move running actors between compute nodes while preserving their in-memory state and message queues. This enables:

- **Load balancing**: Move actors from overloaded nodes
- **Maintenance**: Evacuate nodes for updates
- **Geographic placement**: Move actors closer to users
- **Cost optimization**: Consolidate on fewer nodes during low traffic

```nulang
// The migration is transparent to application code
// Platform handles serialization and restoration

actor GameRoom {
  state: {
    players: Map<PlayerId, Player>,
    game_state: GameState,
    history: [GameEvent]
  }
  
  // This actor can be migrated at any point
  // State is automatically captured and restored
  
  on_message(Move move) {
    // Process move — state is preserved across migration
    self.state.game_state = apply_move(self.state.game_state, move)
    self.state.history.push({ player: move.player, move: move })
    broadcast_to_players(self.state.players, move)
  }
}
```

### 1.4 Edge Placement

**Edge Placement** places actors in the region closest to their users, minimizing latency. The platform automatically:

1. Detects where messages to an actor originate
2. Migrates or replicates the actor to edge regions
3. Routes subsequent messages to the nearest instance

```nulang
// A user in Sydney connects to a global service
// The platform automatically creates an instance in ap-south

actor ShoppingCart {
  placement: Edge  // Automatically place near users
  
  // Cart state is local to the edge region
  // Backend sync happens asynchronously
  
  on_message(AddItem item) {
    self.state.items.push(item)
    sync_to_origin(self.state)  // Background replication
  }
}
```

### 1.5 Event Mesh

The **Event Mesh** is a global, serverless message bus that connects all actors across all regions. It provides:

- At-least-once message delivery
- Exactly-once processing for idempotent actors
- Cross-region broadcasting
- Event sourcing and replay

---

## 2. Architecture Overview

### 2.1 Platform Architecture

```
+============================================================================+
|                     Nulang Cloud Platform Architecture                     |
+============================================================================+
|                                                                            |
|  +------------------+  +------------------+  +-------------------------+  |
|  |   Developer      |  |   Control        |  |   Observability         |  |
|  |   Interface      |  |   Plane          |  |   Stack                 |  |
|  |                  |  |                  |  |                         |  |
|  |  nu cloud deploy |  |  - Scheduler     |  |  - Distributed tracing  |  |
|  |  nu cloud logs   |  |  - Provisioner   |  |  - Metrics (Prometheus) |  |
|  |  nu cloud scale  |  |  - Router        |  |  - Logs (Loki)          |  |
|  |  nu cloud config |  |  - Migrator      |  |  - Alerts (Alertmanager)|  |
|  |                  |  |  - Auto-scaler   |  |  - Dashboard (Grafana)  |  |
|  +--------+---------+  +--------+---------+  +-------------------------+  |
|           |                      |                                         |
+-----------+----------------------+-----------------------------------------+
|                                                                            |
|  +------------------+  +------------------+  +-------------------------+  |
|  |   Compute        |  |   Storage        |  |   Networking            |  |
|  |   Layer          |  |   Layer          |  |   Layer                 |  |
|  |                  |  |                  |  |                         |  |
|  |  - Actor Runtime |  |  - Object Store  |  |  - Global Load Balancer |  |
|  |  - VM/Container  |  |  - Key-Value DB  |  |  - Anycast IPs          |  |
|  |  - WebAssembly   |  |  - Relational DB |  |  - Private Mesh         |  |
|  |  - Sandboxing    |  |  - Vector DB     |  |  - DDoS Protection      |  |
|  |  - Resource Mgmt |  |  - Event Journal |  |  - TLS Termination      |  |
|  +--------+---------+  +--------+---------+  +-------------------------+  |
|           |                      |                                         |
+-----------+----------------------+-----------------------------------------+
|                                                                            |
|  +------------------+  +------------------+  +-------------------------+  |
|  |   Edge Nodes     |  |   Origin Nodes   |  |   Cross-Region          |  |
|  |                  |  |                  |  |   Replication           |  |
|  |  300+ locations  |  |  Core datastores |  |                         |  |
|  |  - Actor hosting |  |  - Source of     |  |  - Event log shipping   |  |
|  |  - Request       |  |    truth         |  |  - State sync           |  |
|  |    handling      |  |  - Analytics     |  |  - Conflict resolution  |  |
|  |  - Caching       |  |  - Backups       |  |  - Consistency model    |  |
|  +------------------+  +------------------+  +-------------------------+  |
|                                                                            |
+============================================================================+
```

### 2.2 Data Flow Architecture

```
+---------------------------------------------------------------------+
|                     Request Flow Through Platform                    |
+---------------------------------------------------------------------+
|                                                                      |
|   User Request                                                       |
|      |                                                               |
|      v                                                               |
|  +------------------+                                                |
|  | Global LB        |  Anycast DNS routes to nearest edge            |
|  | (Anycast)        |                                                |
|  +--------+---------+                                                |
|           |                                                          |
|           v                                                          |
|  +------------------+                                                |
|  | Edge Router      |  TLS termination, rate limiting, WAF           |
|  |                  |                                                |
|  +--------+---------+                                                |
|           |                                                          |
|           v                                                          |
|  +------------------+     +------------------+                       |
|  | Actor Router     | --> | Local Actor      |  Actor exists locally? |
|  |                  |     | Instance         |                        |
|  +--------+---------+     +--------+---------+                       |
|           |                        |                                 |
|           | No local instance      | Yes                             |
|           v                        v                                 |
|  +------------------+     +------------------+                       |
|  | Actor Locator    |     | Process Message  |                       |
|  | (finds actor)    |     |                  |                       |
|  +--------+---------+     +--------+---------+                       |
|           |                        |                                 |
|           v                        v                                 |
|  +------------------+     +------------------+                       |
|  | Fetch from       |     | Return Response  |                       |
|  | Origin / Spawn   |     |                  |                       |
|  +--------+---------+     +--------+---------+                       |
|           |                        |                                 |
|           v                        v                                 |
|     Actor Cached               Response to User                     |
|     at Edge                                                        |
+---------------------------------------------------------------------+
```

### 2.3 Actor Lifecycle in the Cloud

```
+---------------------------------------------------------------------+
|                     Cloud Actor Lifecycle                            |
+---------------------------------------------------------------------+
|                                                                      |
|   DEPLOY        ROUTE         SCALE         MIGRATE       TERMINATE |
|    ---->        ----->        ----->        ------>       -------->|
|                                                                      |
|  Compile to    Register in   Monitor        Move to        Save     |
|  WASM/Native   global actor  message        different      snapshot |
|  bytecode      directory     queue          region         if state |
|                with                             or                 |
|                placement    depth &         consolidate   Archive  |
|                rules        CPU use         on fewer      logs     |
|                             Scale up/down   nodes during  Free     |
|                              based on       low traffic   resources|
|                              policy                       Mark for |
|                                                           GC       |
|                                                                      |
+---------------------------------------------------------------------+
```

### 2.4 Regional Architecture

```
+=====================================================================+
|                    Multi-Region Deployment                           |
+=====================================================================+
|                                                                      |
|   us-east                eu-west                ap-south            |
|   +-------+              +-------+              +-------+           |
|   |Primary|              |Edge   |              |Edge   |           |
|   |Region |              |Region |              |Region |           |
|   |       |              |       |              |       |           |
|   | - API |              | - API |              | - API |           |
|   | - DB  |              | - Cache|             | - Cache|          |
|   | - Auth|              | - Actors|            | - Actors|         |
|   | - Jobs|              |       |              |       |           |
|   +---+---+              +---+---+              +---+---+           |
|       |                      |                      |               |
|       |  Replication         |  Replication         |               |
|       |  (async)             |  (async)             |               |
|       v                      v                      v               |
|   +-------+              +-------+              +-------+           |
|   | Backup|              | Backup|              | Backup|           |
|   | (S3)  |              | (S3)  |              | (S3)  |           |
|   +-------+              +-------+              +-------+           |
|                                                                      |
|   Cross-Region Mesh: Event-driven consistency                        |
|   Conflict Resolution: CRDTs for concurrent updates                  |
+=====================================================================+
```

---

## 3. API Design & Specification

### 3.1 Cloud Deployment DSL

#### 3.1.1 Basic Deployment

```nulang
// cloud.nl — Deployment configuration
cloud {
  name: "my-api",
  version: "1.0.0",
  
  // Entry point for HTTP requests
  http {
    port: 8080,
    
    // Route definitions
    routes {
      GET "/" => HomeController.index,
      GET "/users" => UsersController.list,
      POST "/users" => UsersController.create,
      GET "/users/:id" => UsersController.get,
      PUT "/users/:id" => UsersController.update,
      DELETE "/users/:id" => UsersController.delete
    }
  },
  
  // Actor deployment
  actors {
    UserService: { memory: "256MB", cpu: "0.5" },
    EmailWorker: { memory: "128MB", cpu: "0.25", max_concurrent: 50 },
    AnalyticsCollector: { memory: "512MB", cpu: "1.0" }
  },
  
  // Database
  database {
    provider: PostgreSQL,
    plan: "standard-1",
    backup: "7d"
  },
  
  // Cache
  cache {
    provider: Redis,
    plan: "cache-256mb"
  },
  
  // Storage
  storage {
    buckets: [
      { name: "uploads", public: false },
      { name: "assets", public: true, cdn: true }
    ]
  },
  
  // Environment
  env {
    API_KEY: secret("api-key"),
    LOG_LEVEL: "info"
  }
}
```

#### 3.1.2 Advanced Deployment with Scaling Rules

```nulang
cloud {
  name: "realtime-game",
  version: "2.1.0",
  
  // Global configuration
  regions: ["us-east", "us-west", "eu-west", "ap-south", "sa-east"],
  
  // Actor definitions with scaling policies
  actors {
    // Core game room — stateful, co-located with players
    GameRoom: {
      memory: "512MB",
      cpu: "1.0",
      
      scaling: {
        min_per_region: 1,
        max_per_region: 100,
        
        // Scale based on player count
        metric: "active_players",
        target: 50,        // 50 players per room
        
        // Placement strategy
        placement: Edge,   // Place near players
        
        // State persistence
        persistent: true,
        snapshot_interval: "30s",
        
        // Migration policy
        migration: {
          enabled: true,
          drain_timeout: "5m",
          preserve_state: true
        }
      }
    },
    
    // Matchmaker — stateless, scales aggressively
    Matchmaker: {
      memory: "128MB",
      cpu: "0.25",
      stateless: true,
      
      scaling: {
        min_per_region: 2,
        max_per_region: 500,
        metric: "queue_depth",
        target: 10,
        scale_up_cooldown: "10s",
        scale_down_cooldown: "5m"
      }
    },
    
    // Leaderboard — centralized, high-memory
    Leaderboard: {
      memory: "4GB",
      cpu: "2.0",
      
      scaling: {
        min_per_region: 1,
        max_per_region: 1,  // Single instance per region
        placement: Origin   // Centralized
      }
    },
    
    // Background processor
    EventProcessor: {
      memory: "256MB",
      cpu: "0.5",
      
      scaling: {
        min_per_region: 0,   // Scale to zero when idle
        max_per_region: 20,
        metric: "message_backlog",
        target: 100,
        idle_timeout: "5m"   // Hibernate after 5m idle
      }
    }
  },
  
  // Event-driven architecture
  events {
    topics: [
      "game.events",
      "player.activity", 
      "system.metrics"
    ],
    
    subscriptions: [
      { topic: "game.events", actor: EventProcessor },
      { topic: "player.activity", actor: AnalyticsCollector }
    ]
  },
  
  // Scheduled tasks
  schedules {
    "daily-cleanup": {
      cron: "0 4 * * *",
      actor: MaintenanceWorker,
      action: "cleanup_old_games"
    },
    "hourly-reports": {
      cron: "0 * * * *",
      actor: AnalyticsCollector,
      action: "generate_report"
    }
  }
}
```

#### 3.1.3 Service Bindings

```nulang
// Bind external services to your deployment
cloud {
  name: "ecommerce-app",
  
  bindings {
    // Managed database
    db: PostgreSQL {
      plan: "standard-4",
      storage: "100GB",
      backup: "30d",
      replicas: 2,
      ssl: true
    },
    
    // Managed cache
    cache: Redis {
      plan: "premium-1gb",
      eviction: "allkeys-lru"
    },
    
    // Object storage
    uploads: ObjectStore {
      region: "us-east",
      cdn: true,
      cors: {
        origins: ["https://myapp.com"],
        methods: ["PUT", "GET"]
      }
    },
    
    // Search
    search: OpenSearch {
      plan: "standard-2",
      indexes: ["products", "orders"]
    },
    
    // Queue
    queue: MessageQueue {
      type: "FIFO",
      retention: "14d"
    },
    
    // Secrets
    secrets: SecretsManager {
      keys: ["STRIPE_KEY", "JWT_SECRET", "API_TOKEN"]
    }
  }
}
```

### 3.2 CLI Commands

#### 3.2.1 Deployment Commands

```bash
# Initialize a new cloud project
$ nu cloud init
  Created cloud.nl
  Created .nulang-cloud/

# Deploy to cloud
$ nu cloud deploy
  Building project...
  Uploading (12.4 MB)...
  Deploying to us-east, eu-west...
  ✓ Deployed my-api v1.0.0
  URL: https://my-api.nulang.cloud

# Deploy specific environment
$ nu cloud deploy --staging
$ nu cloud deploy --production

# Deploy with canary rollout
$ nu cloud deploy --canary 10%    # 10% traffic
$ nu cloud deploy --canary 50%
$ nu cloud deploy --promote       # Promote canary to 100%

# Rollback
$ nu cloud rollback
$ nu cloud rollback v1.0.2
$ nu cloud rollback --to 30m      # Rollback to 30 min ago

# Status
$ nu cloud status
  my-api v1.0.3
  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Region    Instances  CPU    Memory  Requests/s
  us-east   5          23%    41%     1,234
  eu-west   3          18%    35%     876
  ap-south  2          31%    52%     543
  
  Total: 10 instances, 2,653 req/s

# Scale manually
$ nu cloud scale GameRoom --instances 20
$ nu cloud scale --auto GameRoom    # Re-enable auto-scaling

# SSH into a running instance (debugging)
$ nu cloud ssh --actor GameRoom --instance abc123
```

#### 3.2.2 Development Commands

```bash
# Local cloud simulation
$ nu cloud dev
  Starting local cloud runtime...
  ✓ API server: http://localhost:8080
  ✓ Actor runtime: 3 actor types running
  ✓ Database: PostgreSQL (local)
  ✓ Cache: Redis (local)
  
  Hot reload enabled. Press r to reload, q to quit.

# Test with cloud bindings
$ nu cloud test
  Running tests with cloud bindings...
  ✓ All 45 tests passed

# Inspect logs
$ nu cloud logs
  2024-06-20T10:23:45Z  INFO  Request: GET /users
  2024-06-20T10:23:45Z  DEBUG Cache hit: users:list
  2024-06-20T10:23:46Z  INFO  Request: POST /users
  2024-06-20T10:23:46Z  INFO  User created: id=123

# Follow logs
$ nu cloud logs --follow

# Filter logs
$ nu cloud logs --actor GameRoom --level error
$ nu cloud logs --since 1h --json

# Metrics
$ nu cloud metrics
  Requests/sec: 2,456
  Latency p50: 12ms, p99: 89ms
  Error rate: 0.02%
  CPU: 34%, Memory: 42%

# Tracing
$ nu cloud trace --request-id abc-123
  [10:23:45.123] → GET /api/users
  [10:23:45.125] → Actor: UserService.get_users()
  [10:23:45.126] → Cache.get("users:list")
  [10:23:45.127] ← Cache hit (0.8ms)
  [10:23:45.134] ← Response: 200 OK (11ms)
```

#### 3.2.3 Configuration Commands

```bash
# Set environment variables
$ nu cloud env set DATABASE_URL "postgres://..."
$ nu cloud env set API_KEY --secret  # Encrypted

# Get environment variables
$ nu cloud env get DATABASE_URL
$ nu cloud env list

# Secrets rotation
$ nu cloud secrets rotate API_KEY

# Update bindings
$ nu cloud binding add queue MessageQueue
$ nu cloud binding update db --plan standard-8

# Config validation
$ nu cloud config validate
  ✓ cloud.nl is valid
  ✓ All bindings resolve
  ✓ Environment variables set
```

### 3.3 Actor Placement API

```nulang
// Programmatic control over actor placement
use nulang::cloud;

// Spawn an actor in a specific region
let eu_actor = cloud::spawn(
  actor: GameRoom,
  region: "eu-west",
  args: { room_id: "lobby-1" }
)

// Spawn with placement constraints
let edge_actor = cloud::spawn(
  actor: ShoppingCart,
  placement: Edge { near: request.geo_location },
  persistent: true
)

// Migrate an actor
cloud::migrate(eu_actor, to: "ap-south")

// Replicate an actor across regions (read replicas)
let primary = cloud::spawn(UserProfile, region: "us-east")
cloud::replicate(primary, to: ["eu-west", "ap-south"])

// Get actor location
let location = cloud::locate(actor_ref)
println("Actor running in {location.region} on {location.node}")

// List all instances of an actor type
let instances = cloud::list_instances(GameRoom)
for inst in instances {
  println("{inst.id}: {inst.region}, {inst.status}, {inst.cpu_usage}% CPU")
}
```

### 3.4 Storage API

#### 3.4.1 Object Store

```nulang
use nulang::cloud::storage;

// Upload a file
let url = storage::upload(
  bucket: "uploads",
  path: "avatars/user-123.png",
  data: image_bytes,
  content_type: "image/png",
  metadata: { user_id: "123" }
)

// Generate presigned URL
let download_url = storage::presigned_url(
  bucket: "uploads",
  path: "avatars/user-123.png",
  expires: 1h,
  operation: Read
)

// Download
let data = storage::download(
  bucket: "uploads",
  path: "avatars/user-123.png"
)

// List objects
let objects = storage::list(
  bucket: "uploads",
  prefix: "avatars/",
  limit: 100
)

// Delete
storage::delete(bucket: "uploads", path: "old-file.txt")
```

#### 3.4.2 Key-Value Store

```nulang
use nulang::cloud::kv;

// Basic operations
kv::set("session:{session_id}", user_data, ttl: 24h)
let data = kv::get("session:{session_id}")
kv::delete("session:{session_id}")

// Atomic operations
kv::increment("counter:visits", by: 1)
kv::compare_and_swap("lock:resource", expected: None, new: "node-1")

// Batch operations
kv::set_batch([
  ("key1", "value1"),
  ("key2", "value2")
])

// Distributed locks
let lock = kv::acquire_lock("process-invoices", ttl: 30s)
if lock.acquired {
  process_invoices()
  lock.release()
}
```

#### 3.4.3 Managed Database

```nulang
use nulang::cloud::db;

// Automatic connection pooling
let pool = db::pool("my-postgres")

// Query
let users = db::query(
  pool,
  "SELECT * FROM users WHERE active = $1",
  params: [true]
)

// Transaction
db::transaction(pool, |tx| {
  let order = db::query(tx, 
    "INSERT INTO orders (...) VALUES (...) RETURNING *",
    params: [...]
  )
  db::execute(tx,
    "UPDATE inventory SET quantity = quantity - $1 WHERE id = $2",
    params: [order.quantity, order.item_id]
  )
  order
})

// Change data capture (CDC)
let changes = db::cdc("orders", since: last_checkpoint)
for change in changes {
  match change.op {
    Insert => event_bus.publish("order.created", change.new),
    Update => event_bus.publish("order.updated", change.new),
    Delete => event_bus.publish("order.deleted", change.old)
  }
}
```

### 3.5 Event System

```nulang
use nulang::cloud::events;

// Publish an event
events::publish(
  topic: "orders.created",
  payload: order,
  ordering_key: order.customer_id  // FIFO within key
)

// Subscribe to events
let subscription = events::subscribe(
  topic: "orders.created",
  handler: |event| {
    let order = Order.from_json(event.payload)
    perform EmailService.send_confirmation(order)
  }
)

// Subscribe with filtering
events::subscribe(
  topic: "orders.*",
  filter: |event| event.payload.total > 1000,
  handler: |event| {
    perform FraudDetection.analyze(event.payload)
  }
)

// Schedule a delayed event
events::schedule(
  topic: "reminders.send",
  payload: { user_id: "123", message: "Your appointment is in 1 hour" },
  deliver_at: now() + 1h
)

// Event replay (for debugging/recovery)
events::replay(
  topic: "orders.created",
  from: "2024-06-01T00:00:00Z",
  to: "2024-06-02T00:00:00Z",
  handler: |event| process_order(event.payload)
)
```

### 3.6 Observability API

```nulang
use nulang::cloud::observability;

// Logging
observability::log_info("User {user_id} logged in")
observability::log_warn("High latency detected: {latency}ms")
observability::log_error("Database connection failed", error: err)

// Structured logging
observability::log_info("Request processed", %{
  method: request.method,
  path: request.path,
  duration_ms: elapsed,
  status_code: response.status
})

// Metrics
observability::counter("requests_total", labels: %{method: "GET", path: "/users"})
observability::gauge("active_connections", 42)
observability::histogram("request_duration_seconds", elapsed / 1000.0, 
  buckets: [0.01, 0.05, 0.1, 0.5, 1.0, 5.0])

// Distributed tracing
let span = observability::start_span("process_payment")
span.set_tag("order_id", order.id)
span.set_tag("amount", order.total)

let db_span = observability::start_span("db_query", parent: span)
let result = perform Database.query(...)
db_span.finish()

span.finish()

// Health checks
cloud::health_check("database", || {
  match perform Database.ping() {
    Ok(_) => Healthy,
    Error(e) => Unhealthy(message: e.to_string())
  }
})
```

---

## 4. Module Reference

### 4.1 Module Hierarchy

```
nulang-cloud/
├── cli/
│   ├── main.nl           # CLI entry point
│   ├── commands/
│   │   ├── init.nl       # `nu cloud init`
│   │   ├── deploy.nl     # `nu cloud deploy`
│   │   ├── rollback.nl   # `nu cloud rollback`
│   │   ├── status.nl     # `nu cloud status`
│   │   ├── logs.nl       # `nu cloud logs`
│   │   ├── metrics.nl    # `nu cloud metrics`
│   │   ├── scale.nl      # `nu cloud scale`
│   │   ├── config.nl     # `nu cloud config`
│   │   ├── ssh.nl        # `nu cloud ssh`
│   │   └── dev.nl        # `nu cloud dev`
│   └── output.nl         # Terminal output
├── runtime/
│   ├── actor_host.nl     # Actor hosting engine
│   ├── wasm_runtime.nl   # WebAssembly runtime
│   ├── sandbox.nl        # Security sandbox
│   ├── scheduler.nl      # Actor scheduler
│   ├── migrator.nl       # State migration
│   └── hibernation.nl    # Actor hibernation/resume
├── control/
│   ├── provisioner.nl    # Resource provisioning
│   ├── router.nl         # Global request router
│   ├── autoscaler.nl     # Auto-scaling engine
│   ├── deployer.nl       # Deployment orchestrator
│   └── health.nl         # Health monitoring
├── storage/
│   ├── object_store.nl   # S3-compatible object storage
│   ├── kv_store.nl       # Distributed key-value store
│   ├── database.nl       # Managed database interface
│   └── event_store.nl    # Event sourcing storage
├── networking/
│   ├── load_balancer.nl  # Global load balancing
│   ├── service_mesh.nl   # Inter-service communication
│   ├── gateway.nl        # API gateway
│   └── edge_cache.nl     # Edge caching layer
├── observability/
│   ├── logging.nl        # Centralized logging
│   ├── metrics.nl        # Metrics collection
│   ├── tracing.nl        # Distributed tracing
│   ├── alerting.nl       # Alert management
│   └── dashboard.nl      # Dashboard API
├── security/
│   ├── iam.nl            # Identity and access management
│   ├── secrets.nl        # Secrets management
│   ├── tls.nl            # Certificate management
│   └── firewall.nl       # Network firewall
└── bindings/
    ├── postgres.nl       # PostgreSQL binding
    ├── redis.nl          # Redis binding
    ├── s3.nl             # S3-compatible binding
    ├── elasticsearch.nl  # OpenSearch binding
    └── kafka.nl          # Kafka binding
```

### 4.2 Core Types

```nulang
// Cloud deployment configuration
type CloudConfig = {
  name: String,
  version: String,
  regions: [String],
  actors: Map<String, ActorConfig>,
  http: Option<HttpConfig>,
  database: Option<DatabaseBinding>,
  cache: Option<CacheBinding>,
  storage: Option<StorageConfig>,
  events: Option<EventsConfig>,
  schedules: Map<String, ScheduleConfig>,
  env: Map<String, EnvValue>,
  bindings: Map<String, ServiceBinding>
}

type ActorConfig = {
  memory: String,        // e.g., "256MB"
  cpu: String,           // e.g., "0.5" = 0.5 vCPU
  stateless: Bool,
  persistent: Bool,
  placement: PlacementStrategy,
  scaling: ScalingPolicy,
  snapshot_interval: Option<Duration>,
  migration: Option<MigrationPolicy>
}

enum PlacementStrategy {
  Edge,                  // Place near users
  Origin,                // Centralized in origin region
  Regional(String),      // Specific region
  MultiRegion([String])  // Replicated across regions
}

type ScalingPolicy = {
  min_per_region: Int,
  max_per_region: Int,
  metric: ScalingMetric,
  target: Float,
  scale_up_cooldown: Duration,
  scale_down_cooldown: Duration,
  idle_timeout: Option<Duration>  // For scale-to-zero
}

enum ScalingMetric {
  CpuPercentage,
  MemoryPercentage,
  MessageQueueDepth,
  RequestCount,
  Custom(String)
}

type MigrationPolicy = {
  enabled: Bool,
  drain_timeout: Duration,
  preserve_state: Bool
}

type ServiceBinding =
  | PostgreSQL { plan: String, storage: String, backup: String, replicas: Int }
  | Redis { plan: String, eviction: String }
  | ObjectStore { region: String, cdn: Bool, cors: CorsConfig }
  | OpenSearch { plan: String, indexes: [String] }
  | MessageQueue { type: String, retention: String }
  | SecretsManager { keys: [String] }

type DeploymentStatus = {
  name: String,
  version: String,
  status: DeploymentState,
  regions: [RegionStatus],
  total_instances: Int,
  total_requests_per_second: Float,
  health: HealthStatus
}

enum DeploymentState {
  Deploying,
  Active,
  Degraded,
  RollingBack,
  Inactive
}

type RegionStatus = {
  region: String,
  instances: Int,
  cpu_usage: Float,
  memory_usage: Float,
  requests_per_second: Float,
  error_rate: Float,
  latency_p50: Duration,
  latency_p99: Duration
}
```

### 4.3 Effect Definitions

```nulang
// Cloud runtime effects
effect CloudRuntime {
  fn spawn(actor_type: Atom, region: Option<String>, args: Any) -> ActorRef;
  fn migrate(actor: ActorRef, to_region: String) -> ();
  fn locate(actor: ActorRef) -> LocationInfo;
  fn list_instances(actor_type: Atom) -> [InstanceInfo];
  fn current_region() -> String;
}

effect CloudStorage {
  fn upload(bucket: String, path: String, data: Bytes, opts: UploadOpts) -> String;
  fn download(bucket: String, path: String) -> Bytes;
  fn presigned_url(bucket: String, path: String, expires: Duration, op: Operation) -> String;
  fn list(bucket: String, prefix: String, limit: Int) -> [ObjectInfo];
  fn delete(bucket: String, path: String) -> ();
}

effect CloudKV {
  fn get(key: String) -> Option<JSON>;
  fn set(key: String, value: JSON, ttl: Option<Duration>) -> ();
  fn delete(key: String) -> ();
  fn increment(key: String, by: Int) -> Int;
  fn acquire_lock(key: String, ttl: Duration) -> LockResult;
}

effect CloudEvents {
  fn publish(topic: String, payload: JSON, ordering_key: Option<String>) -> ();
  fn subscribe(topic: String, filter: Option<EventFilter>, handler: EventHandler) -> Subscription;
  fn schedule(topic: String, payload: JSON, deliver_at: DateTime) -> ScheduledEvent;
  fn replay(topic: String, from: DateTime, to: DateTime, handler: EventHandler) -> ();
}

effect CloudObservability {
  fn log(level: LogLevel, message: String, metadata: Map<String, JSON>) -> ();
  fn counter(name: String, labels: Map<String, String>) -> ();
  fn gauge(name: String, value: Float, labels: Map<String, String>) -> ();
  fn histogram(name: String, value: Float, labels: Map<String, String>) -> ();
  fn start_span(name: String, parent: Option<Span>) -> Span;
  fn finish_span(span: Span) -> ();
}
```

---

## 5. Implementation Phases

### 5.1 Phase 1: Core Runtime (Weeks 1-8)

**Goal:** Build the actor hosting runtime with WebAssembly sandboxing.

```
Milestone: v0.1.0 — "Host"
+---------------------------------------------------------------+
| Week 1-2            | Week 3-4           | Week 5-8          |
+---------------------+--------------------+-------------------+
| WASM runtime        | Actor scheduler    | Sandbox           |
|                     |                    |                   |
| - Compile .nl to    | - Message queue    | - Resource limits |
|   WASM              |   per actor        | - Memory isolation|
| - WASI interface    | - Fair scheduling  | - Network policy  |
| - Module loading    | - Priority queues  | - File system     |
| - Memory mgmt       | - Backpressure     |   restrictions    |
|                     | - Dead letter      | - Capability-based|
| Basic HTTP server   |   queue            |   security        |
| - Request routing   | - Actor lifecycle  |                   |
| - TLS termination   |   (spawn/kill)     | Local dev server  |
+---------------------+--------------------+-------------------+
| Deliverable: Local dev server (`nu cloud dev`)               |
| Tests: WASM execution, actor scheduling, sandbox isolation    |
+---------------------------------------------------------------+
```

### 5.2 Phase 2: Distributed Runtime (Weeks 9-16)

**Goal:** Add distributed actor communication and state management.

```
Milestone: v0.2.0 — "Distribute"
+---------------------------------------------------------------+
| Week 9-10           | Week 11-12         | Week 13-16        |
+---------------------+--------------------+-------------------+
| Actor directory     | State management   | State migration   |
|                     |                    |                   |
| - Global actor      | - Snapshot capture | - Live migration  |
|   registry          | - State restore    | - State transfer  |
| - Location          | - Event sourcing   | - Queue drain     |
|   transparency      | - Conflict         | - Zero-downtime   |
| - Message routing   |   resolution       |   movement        |
|   across nodes      | - CRDT support     | - Migration       |
|                     |                    |   scheduling      |
| Clustering          | Persistence        | Geographic        |
| - Node discovery    | - Write-ahead log  | placement         |
| - Gossip protocol   | - Checkpoints      | - Edge detection  |
| - Failure detection | - Recovery         | - Latency-based   |
|                     |                    |   routing         |
+---------------------+--------------------+-------------------+
```

### 5.3 Phase 3: Auto-scaling & Management (Weeks 17-24)

**Goal:** Implement auto-scaling, deployment orchestration, and managed services.

```
Milestone: v0.3.0 — "Scale"
+---------------------------------------------------------------+
| Week 17-18          | Week 19-20         | Week 21-24        |
+---------------------+--------------------+-------------------+
| Auto-scaler         | Deployment system  | Managed services  |
|                     |                    |                   |
| - Metric collection | - Build pipeline   | - PostgreSQL      |
| - Scale up/down     | - Canary deploys   | - Redis           |
| decisions           | - Blue/green       | - Object storage  |
| - Predictive        | - Rollbacks        | - Message queues  |
|   scaling           | - Health checks    | - Secrets mgmt    |
| - Cost optimization | - Traffic shifting |                   |
|                     |                    | CLI tools         |
| Load balancer       | Control plane      | - Deploy command  |
| - Anycast routing   | - Scheduler        | - Logs command    |
| - Health-aware      | - Provisioner      | - Metrics command |
|   routing           | - Node manager     | - Config command  |
+---------------------+--------------------+-------------------+
```

### 5.4 Phase 4: Observability & Edge (Weeks 25-32)

**Goal:** Full observability stack and edge node deployment.

```
Milestone: v0.4.0 — "Observe"
+---------------------------------------------------------------+
| Week 25-26          | Week 27-28         | Week 29-32        |
+---------------------+--------------------+-------------------+
| Observability       | Edge network       | Platform polish   |
|                     |                    |                   |
| - Distributed       | - Edge node        | - Usage billing   |
|   tracing           |   deployment       | - Rate limiting   |
| - Metrics           | - Cache layer      | - Quota mgmt      |
|   aggregation       | - Request          | - Multi-tenant    |
| - Log collection    |   coalescing       |   isolation       |
|   & indexing        | - Geographic       | - Admin dashboard |
| - Alerting engine   |   routing          | - Support tools   |
| - Grafana           | - Cold start       |                   |
|   dashboards        |   optimization     | Documentation     |
|                     |                    | - Deployment      |
| Health monitoring   | DDoS protection    |   guides          |
| - Probes            | - Rate limiting    | - API reference   |
| - Self-healing      | - WAF rules        | - Best practices  |
| - On-call           | - Bot detection    | - Example apps    |
|   integration       |                    |                   |
+---------------------+--------------------+-------------------+
```

### 5.5 Phase 5: Ecosystem & GA (Weeks 33-40)

**Goal:** Production readiness with ecosystem integrations.

```
Milestone: v1.0.0 — "Production"
+---------------------------------------------------------------+
| Week 33-34          | Week 35-36         | Week 37-40        |
+---------------------+--------------------+-------------------+
| Ecosystem           | Security hardening | General           |
|                     |                    | availability      |
| - CI/CD             | - SOC 2 compliance |                   |
|   integrations      | - Penetration      | - Public signup   |
| - GitHub Actions    |   testing          | - Pricing tiers   |
|   plugin            | - Bug bounty       | - SLA guarantees  |
| - Terraform         |   program          | - Enterprise      |
|   provider          | - Security         |   support         |
| - Pulumi provider   |   advisories       | - Community       |
|                     |   automation       |   forum           |
| Marketplace         |                    |                   |
| - Pre-built         | Compliance         | Case studies      |
|   templates         | - GDPR compliance  | - Partner         |
| - Partner           | - HIPAA ready      |   integrations    |
|   integrations      | - Data residency   | - Customer        |
| - One-click         |   options          |   testimonials    |
|   deploys           |                    |                   |
+---------------------+--------------------+-------------------+
```

---

## 6. Appendices

### 6.1 Comparison with Existing Platforms

| Feature | Cloudflare Workers | Deno Deploy | Fly.io | Nulang Cloud |
|---------|-------------------|-------------|--------|--------------|
| Language | JS/TS/WASM | JS/TS | Docker | Native Nulang |
| Actor model | No | No | No | Yes (native) |
| Stateful | Limited | No | Yes (Volumes) | Yes (built-in) |
| Edge nodes | 300+ | 35 | 30+ | 300+ (planned) |
| Cold start | ~0ms | ~0ms | ~300ms | ~0ms (hibernation) |
| Auto-migration | No | No | Yes | Yes (transparent) |
| Local dev | Wrangler | CLI | Flyctl | `nu cloud dev` |
| Cost model | Per request | Per request | Per VM | Per actor-sec |

### 6.2 Pricing Model

| Resource | Unit | Price (example) |
|----------|------|-----------------|
| Compute | Actor-second (vCPU) | $0.00001 |
| Memory | GB-second | $0.000002 |
| Requests | Million requests | $0.50 |
| Bandwidth | GB | $0.05 |
| Storage (Object) | GB-month | $0.02 |
| Storage (Database) | GB-month | $0.25 |
| Cache (Redis) | GB-month | $0.15 |

### 6.3 Error Code Reference

```nulang
enum CloudError {
  // Deployment errors
  DeploymentFailed { reason: String, logs: [String] },
  InvalidConfig { field: String, reason: String },
  ResourceLimitExceeded { resource: String, limit: Int },
  
  // Runtime errors
  ActorNotFound { name: String },
  RegionNotAvailable { region: String },
  MigrationFailed { actor: String, from: String, to: String },
  StateTooLarge { size: Int, max: Int },
  
  // Scaling errors
  ScaleLimitReached { actor: String, max: Int },
  InsufficientCapacity { region: String },
  
  // Service errors
  BindingNotFound { name: String },
  SecretNotFound { name: String },
  DatabaseConnectionFailed { binding: String },
  
  // Network errors
  RequestTimeout { duration: Duration },
  CircuitBreakerOpen { service: String },
  RateLimitExceeded { limit: Int, window: Duration }
}
```

### 6.4 SLA Targets

| Metric | Target |
|--------|--------|
| Uptime | 99.99% |
| API latency (p50) | < 10ms |
| API latency (p99) | < 100ms |
| Actor cold start | < 50ms |
| Actor migration | < 1s downtime |
| State snapshot | < 100ms |
| Cross-region replication | < 5s |
| Data durability | 99.999999999% |

### 6.5 Security Model

1. **Sandboxing**: Each actor runs in a WebAssembly sandbox with capability-based security
2. **Network Isolation**: Actors can only access explicitly permitted network endpoints
3. **Encryption**: All data encrypted at rest and in transit
4. **Secret Management**: Secrets never exposed to application code as plaintext
5. **Audit Logging**: All administrative actions logged and retained for 1 year
6. **DDoS Protection**: Automatic DDoS mitigation at the edge

### 6.6 Glossary

| Term | Definition |
|------|------------|
| **Actor-as-a-Service** | Deployment model where actors are the fundamental scaling unit |
| **Deployment Unit** | The atomic unit of deployment (a Nulang application) |
| **Stateful Migration** | Moving a running actor between nodes while preserving state |
| **Edge Placement** | Automatically placing actors near their users |
| **Event Mesh** | Global message bus connecting all actors |
| **Hibernation** | Suspending idle actors to zero-cost cold storage |
| **Placement Strategy** | Rules determining where actors run geographically |
| **Service Binding** | Managed external service attached to a deployment |
| **Cold Start** | Time to activate a hibernated actor |
| **Anycast** | Network routing that directs users to the nearest edge node |

---

*Document Version: 1.0.0*  
*Last Updated: 2024*  
*Status: Ready for Implementation*
