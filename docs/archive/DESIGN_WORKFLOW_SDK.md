# Nulang Workflow SDK Design Document

## Overview

The Nulang Workflow SDK (`nulang-workflow`) is a durable workflow orchestration framework native to Nulang's actor model. It enables developers to write long-running, fault-tolerant business processes that survive crashes, restarts, and deployments. Inspired by Temporal, Cadence, and Orleans, the SDK leverages Nulang's unique capabilities — persistent actors, pattern matching, and the effect system — to provide an ergonomic and powerful workflow programming model.

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

### 1.1 Workflow as Actor

A **Workflow** in the Nulang Workflow SDK is a special kind of actor that is automatically persisted and checkpointed. Unlike regular actors that maintain only in-memory state, workflow actors write their complete state to an event journal after every significant operation, enabling them to resume from exactly where they left off after any interruption.

```nulang
workflow PurchaseOrder {
  input: { order: Order }
  
  // Workflow steps are durable — each checkpoint survives restarts
  step validate { ... }
  step charge { ... }
  step ship { ... }
}
```

Key properties of workflow actors:
- **Deterministic**: Given the same input and event sequence, always produces the same output
- **Durable**: State automatically persisted after each step
- **Recoverable**: Resumes from last checkpoint after crashes
- **Observable**: Emits events for monitoring and debugging
- **Versionable**: Supports schema evolution without breaking in-flight workflows

### 1.2 Activity

An **Activity** is a pure or idempotent function that represents a unit of work within a workflow. Activities can fail independently and be retried without affecting workflow consistency. They execute in a separate worker pool from the workflow itself.

```nulang
// Activity definition
@activity(retry: { max_attempts: 3, backoff: exponential })
fn process_payment(order_id: String, amount: Decimal) -> PaymentResult {
  // This can be retried independently
  perform PaymentGateway.charge(order_id, amount)
}
```

Activities are the only place where external side effects (API calls, database writes, file I/O) should occur. The workflow orchestrator itself never performs side effects — it only coordinates activities.

### 1.3 Saga

A **Saga** is a long-running transaction pattern that ensures data consistency across multiple services through compensating actions. If any step in a saga fails, previously completed steps are undone by executing their compensation functions.

```nulang
saga TravelBooking {
  step book_flight {
    activity: book_flight(details),
    compensate: cancel_flight
  }
  
  step book_hotel {
    activity: book_hotel(details),
    compensate: cancel_hotel
  }
  
  step charge_card {
    activity: process_payment(total),
    compensate: refund_payment
  }
}
```

### 1.4 Timer

A **Timer** is a durable timer that survives workflow restarts. When a workflow sets a timer for "24 hours", that timer is persisted and will fire exactly 24 hours later, even if the workflow host restarts multiple times in between.

```nulang
// Durable timer
perform Timer.sleep(24h)  // Survives restarts

// Deadline timer
let deadline = Timer.deadline(from_now: 30m)
match deadline.wait() {
  Ok => println("Proceeding..."),
  Timeout => perform compensate_step()
}
```

### 1.5 Signal

A **Signal** is an external event that can advance workflow state while the workflow is running. Signals enable workflows to respond to real-world events — user approvals, webhook callbacks, sensor readings.

```nulang
// Workflow waiting for a signal
workflow ApprovalProcess {
  input: { request: ChangeRequest }
  
  step await_approval {
    let signal = perform Signal.wait("approval_decision", timeout: 48h)
    match signal {
      Approved => proceed_to_implementation(),
      Rejected => notify_requester(request, "rejected"),
      Timeout => escalate_to_manager(request)
    }
  }
}

// Sending a signal from outside
perform Workflow.signal(
  workflow_id: "approval-123",
  signal: "approval_decision",
  payload: Approved
)
```

### 1.6 Query

A **Query** allows external systems to read workflow state without mutating it. Queries are read-only operations that execute against the latest workflow state.

```nulang
// Define a query handler on the workflow
workflow PurchaseOrder {
  // ... steps ...
  
  @query
  fn get_status() -> OrderStatus {
    self.state.status  // Read current state
  }
  
  @query
  fn get_progress() -> Float {
    self.state.completed_steps / self.state.total_steps
  }
}

// Query from outside
let status = perform Workflow.query(
  workflow_id: "order-456",
  query: "get_status"
)
```

---

## 2. Architecture Overview

### 2.1 System Architecture

```
+============================================================================+
|                     Nulang Workflow SDK Architecture                       |
+============================================================================+
|                                                                            |
|  +-------------------+  +-------------------+  +-----------------------+  |
|  |   Workflow        |  |   Workflow        |  |   Workflow            |  |
|  |   Definitions     |  |   Runtime         |  |   Workers             |  |
|  |                   |  |                   |  |                       |  |
|  |  workflow{}       |  |  - Orchestrator   |  |  - Activity workers   |  |
|  |  step{}           |  |  - State machine  |  |  - Signal handlers    |  |
|  |  @activity        |  |  - Event journal  |  |  - Timer scheduler    |  |
|  |  @signal          |  |  - Checkpointing  |  |  - Query handlers     |  |
|  +--------+----------+  +--------+----------+  +-----------+-----------+  |
|           |                      |                          |              |
+-----------+----------------------+--------------------------+--------------+
|                                                                            |
|  +-------------------+  +-------------------+  +-----------------------+  |
|  |   Event Journal   |  |   State Store     |  |   Timer Service       |  |
|  |                   |  |                   |  |                       |  |
|  |  Append-only log  |  |  Workflow state   |  |  - Timer persistence  |  |
|  |  of all events    |  |  snapshots        |  |  - Fire scheduling    |  |
|  |  (Kafka/SQLite)   |  |  (SQLite/Postgres)|  |  - At-least-once      |  |
|  |                   |  |                   |  |                       |  |
|  +-------------------+  +-------------------+  +-----------------------+  |
|                                                                            |
|  +-------------------+  +-------------------+  +-----------------------+  |
|  |   Activity Worker |  |   Signal Router   |  |   Observability       |  |
|  |   Pool            |  |                   |  |                       |  |
|  |                   |  |  - Named signal   |  |  - OpenTelemetry      |  |
|  |  - Task queue     |  |    routing        |  |  - Workflow metrics   |  |
|  |  - Retry logic    |  |  - Broadcast      |  |  - Structured logging |  |
|  |  - Heartbeat      |  |  - Payload        |  |  - Distributed        |  |
|  |  - Rate limiting  |  |    validation     |  |    tracing            |  |
|  +-------------------+  +-------------------+  +-----------------------+  |
|                                                                            |
+============================================================================+
```

### 2.2 Workflow Execution Flow

```
+---------------------------------------------------------------------+
|                     Workflow Execution Lifecycle                     |
+---------------------------------------------------------------------+
|                                                                      |
|   START                                                               |
|    |                                                                  |
|    v                                                                  |
|  +------------------+    +------------------+    +----------------+ |
|  |  1. Receive      |    |  2. Rehydrate    |    |  3. Execute    | |
|  |     Start Request|--->|     from Journal |--->|     Next Step  | |
|  |                  |    |                  |    |                | |
|  |  - New workflow  |    |  - Read events   |    |  - Run activity| |
|  |  - Signal        |    |  - Replay state  |    |  - Set timer   | |
|  |  - Timer fired   |    |  - Build state   |    |  - Wait signal | |
|  +------------------+    +------------------+    +--------+-------+ |
|                                                        |            |
|                                                        v            |
|                                              +------------------+   |
|                                              |  4. Checkpoint   |   |
|                                              |                  |   |
|                                              |  - Write events  |   |
|                                              |  - Save state    |   |
|                                              |  - Schedule next |   |
|                                              +--------+---------+   |
|                                                       |             |
|                                                       v             |
|                                              +------------------+   |
|                                              |  5. Check Status |   |
|                                              |                  |   |
|                                              |  Complete?  ---> DONE |
|                                              |  Waiting?   ---> SLEEP|
|                                              |  Error?     ---> FAIL |
|                                              +------------------+   |
|                                                                      |
+---------------------------------------------------------------------+
```

### 2.3 Event Journal Replay

```
+--------------------------------------------------------------------+
|                     Event Journal Replay                            |
|                                                                     |
|   The core durability mechanism is an append-only event journal.    |
|   Workflow state is never mutated directly — only through events.   |
|                                                                     |
|   Journal Entries:                                                  |
|   +--------+--------+--------+--------+--------+--------+          |
|   | Seq 1  | Seq 2  | Seq 3  | Seq 4  | Seq 5  | Seq 6  |          |
|   +--------+--------+--------+--------+--------+--------+          |
|   |WF Start|Activity|Activity|Timer   |Signal  |Activity|          |
|   |        |Started |Completed|Set    |Received|Started |          |
|   +--------+--------+--------+--------+--------+--------+          |
|        |                                              |             |
|        v                                              v             |
|   +---------+                                   +----------+       |
|   | Initial |                                   | Snapshot |       |
|   | State   |                                   | at Seq 6 |       |
|   +---------+                                   +----------+       |
|                                                                     |
|   Recovery: Read snapshot + replay events after snapshot            |
|                                                                     |
+--------------------------------------------------------------------+
```

### 2.4 Component Interaction Diagram

```
+-------------+        +--------------+        +---------------+
|   Client    |        |   Workflow   |        |   Activity    |
|             |        |   Engine     |        |   Workers     |
+------+------+        +------+-------+        +-------+-------+
       |                      |                        |
       | 1. Start Workflow    |                        |
       |--------------------->|                        |
       |                      | 2. Write "Started"     |
       |                      |    to Journal          |
       |                      |                        |
       |                      | 3. Dispatch Activity   |
       |                      |----------------------->|
       |                      |                        |
       |                      | 4. Heartbeat           |
       |                      |<-----------------------|
       |                      |                        |
       |                      | 5. Activity Complete   |
       |                      |<-----------------------|
       |                      |                        |
       |                      | 6. Write "Completed"   |
       |                      |    to Journal          |
       |                      |                        |
       | 7. Signal            |                        |
       |--------------------->|                        |
       |                      | 8. Write "Signal"      |
       |                      |    to Journal          |
       |                      |                        |
       | 9. Query Status      |                        |
       |--------------------->|                        |
       |<---------------------|                        |
       |                      |                        |
+------+------+        +------+-------+        +-------+-------+
|   Client    |        |   Workflow   |        |   Activity    |
|             |        |   Engine     |        |   Workers     |
+-------------+        +--------------+        +---------------+
```

### 2.5 Saga Pattern Architecture

```
+--------------------------------------------------------------------+
|                        Saga Execution                               |
+--------------------------------------------------------------------+
|                                                                     |
|  Normal Flow:                                                       |
|                                                                     |
|   [Step 1] -----> [Step 2] -----> [Step 3] -----> [Step 4]        |
|   Book Flight     Book Hotel      Charge Card     Send Email        |
|      OK              OK              OK              OK             |
|                                                                     |
|  Failure at Step 3:                                                 |
|                                                                     |
|   [Step 1] -----> [Step 2] -----> [Step 3] -----> [Step 4]        |
|   Book Flight     Book Hotel     Charge Card      (skip)            |
|      OK              OK           FAILED                              |
|      |               |               |                              |
|      v               v               v                              |
|   [Cancel       [Cancel        (no action                         |
|    Flight]       Hotel]          needed)                            |
|                                                                     |
|  Compensation runs in reverse order:                                |
|  1. Cancel Hotel booking                                            |
|  2. Cancel Flight booking                                           |
|                                                                     |
+--------------------------------------------------------------------+
```

---

## 3. API Design & Specification

### 3.1 Workflow Definition DSL

#### 3.1.1 Basic Workflow

```nulang
// Simple sequential workflow
workflow GreetingWorkflow {
  input: { name: String }
  
  step create_greeting {
    activity generate_greeting(name)
  }
  
  step deliver {
    activity send_message(greeting_result)
  }
}

// Activity definition
@activity
fn generate_greeting(name: String) -> String {
  "Hello, {name}! Welcome to Nulang workflows."
}

@activity
fn send_message(content: String) -> Delivered {
  perform NotificationService.send(content)
}
```

#### 3.1.2 Workflow with Conditionals

```nulang
// Workflow with branching logic
workflow OrderProcessing {
  input: { order: Order }
  
  step validate {
    activity validate_order(order)
  }
  
  // Conditional branching
  branch {
    when validate_result.is_valid && order.total > 1000 {
      step manager_approval {
        activity request_manager_approval(order)
      }
      
      step await_manager_decision {
        signal "manager_decision"
      }
      
      // Nested condition after signal
      branch {
        when manager_decision == "approved" {
          step process {
            activity process_order(order)
          }
        }
        when manager_decision == "rejected" {
          step notify_rejection {
            activity notify_customer(order, "rejected")
          }
        }
      }
    }
    
    when validate_result.is_valid && order.total <= 1000 {
      step process {
        activity process_order(order)
      }
    }
    
    default {
      step notify_invalid {
        activity notify_customer(order, "invalid")
      }
    }
  }
  
  step complete {
    activity mark_complete(order.id)
  }
}
```

#### 3.1.3 Workflow with Loops

```nulang
// Workflow with retry loops
workflow RetryableProcessing {
  input: { items: [Item] }
  
  // For-each loop
  for item in items {
    step process_item {
      activity process_item(item)
      retry: {
        max_attempts: 5,
        backoff: exponential { initial: 1s, max: 60s, multiplier: 2.0 },
        retry_on: [TransientError, TimeoutError],
        give_up_on: [PermanentError, ValidationError]
      }
      timeout: 30s
    }
  }
  
  // While loop with condition
  step retry_with_backoff {
    let attempt = 0
    let result = None
    
    while result == None && attempt < 5 {
      result = perform Try(activity call_external_service())
      attempt = attempt + 1
      
      if result == None {
        perform Timer.sleep(2^attempt * 1s)
      }
    }
  }
}
```

#### 3.1.4 Complete E-Commerce Workflow

```nulang
workflow PurchaseOrder {
  input: { order: Order }
  output: OrderResult
  
  // Metadata
  timeout: 7d
  retries: { strategy: automatic }
  
  // Step 1: Validation
  step validate {
    activity validate_order(order)
    retry: { max_attempts: 3, backoff: exponential }
    on_error: fail_workflow("validation_failed")
  }
  
  // Step 2: Payment (with compensation)
  step charge_payment {
    activity process_payment(order.total, order.payment_method)
    compensate: refund_payment(order.payment_id)
    retry: { max_attempts: 5, backoff: exponential { max: 5m } }
    timeout: 2m
  }
  
  // Step 3: Inventory check and reservation
  step reserve_inventory {
    activity reserve_items(order.items)
    compensate: release_inventory_reservation
    retry: { max_attempts: 3 }
  }
  
  // Step 4: Parallel execution — notify while preparing shipment
  parallel {
    branch notifications {
      step send_confirmation {
        activity send_email(
          to: order.customer_email,
          template: "order_confirmed",
          data: order
        )
      }
      
      step notify_warehouse {
        activity notify_warehouse_system(order)
      }
    }
    
    branch fulfillment {
      step create_shipment {
        activity create_shipment(order.items, order.shipping_address)
        compensate: cancel_shipment
        timeout: 24h
      }
    }
  }
  
  // Step 5: Wait for shipment to be picked up
  step await_pickup {
    signal "shipment_picked_up"
    timeout: 72h
    on_timeout: escalate_to_support(order)
  }
  
  // Step 6: Delivery tracking
  step track_delivery {
    let delivered = false
    let attempts = 0
    
    while !delivered && attempts < 100 {
      let status = activity check_delivery_status(shipment_id)
      
      if status == "delivered" {
        delivered = true
      } else {
        perform Timer.sleep(1h)
        attempts = attempts + 1
      }
    }
    
    if !delivered {
      activity escalate_delivery_issue(order)
    }
  }
  
  // Step 7: Final notification
  step send_delivery_confirmation {
    activity send_email(
      to: order.customer_email,
      template: "order_delivered",
      data: { order: order, delivered_at: now() }
    )
  }
  
  // Final state
  return {
    order_id: order.id,
    status: "completed",
    payment_id: charge_payment.result.id,
    shipment_id: create_shipment.result.id
  }
}
```

### 3.2 Activity System

#### 3.2.1 Activity Definition

```nulang
// Basic activity
@activity
fn validate_order(order: Order) -> ValidationResult {
  // Business logic here
  if order.items.is_empty() {
    ValidationResult.invalid("Order must contain at least one item")
  } else {
    ValidationResult.valid()
  }
}

// Activity with retry configuration
@activity(
  retry: {
    max_attempts: 5,
    backoff: exponential {
      initial: 1s,
      max: 5m,
      multiplier: 2.0,
      jitter: 0.1
    },
    retry_on: [NetworkError, TimeoutError, RateLimitError],
    give_up_on: [AuthenticationError, ValidationError]
  },
  timeout: 30s,
  heartbeat: 10s  // Send heartbeat every 10s
)
fn process_payment(amount: Decimal, method: PaymentMethod) -> PaymentResult {
  perform PaymentGateway.charge(amount, method)
}

// Activity that returns a Result type
@activity(retry: { max_attempts: 3 })
fn fetch_exchange_rate(from: Currency, to: Currency) -> Result<Rate, FetchError> {
  perform ForexAPI.get_rate(from, to)
}

// Async activity (runs concurrently)
@activity(async: true)
fn notify_user_async(user_id: String, message: String) -> () {
  perform NotificationService.send(user_id, message)
}
```

#### 3.2.2 Activity Execution Context

```nulang
// Access activity context
@activity
fn context_aware_activity(data: String) -> String {
  let ctx = ActivityContext.current()
  
  println("Activity ID: {ctx.activity_id}")
  println("Attempt: {ctx.attempt}")
  println("Workflow ID: {ctx.workflow_id}")
  println("Started at: {ctx.started_at}")
  
  // Heartbeat to prevent timeout
  ctx.heartbeat("Processing item {data}...")
  
  // Long-running work with periodic heartbeats
  for chunk in large_dataset {
    ctx.heartbeat("Processed {chunk.index}/{chunk.total}")
    process(chunk)
  }
  
  "Done"
}
```

#### 3.2.3 Activity Worker Pool

```nulang
// Configure activity worker pool
let worker_pool = ActivityWorkerPool.new({
  max_workers: 50,
  queue_size: 10000,
  task_timeout: 5m,
  heartbeat_check_interval: 10s,
  
  // Worker selection strategy
  routing: {
    default: "general_pool",
    by_activity: [
      { pattern: "payment_*", pool: "payment_workers" },
      { pattern: "email_*", pool: "email_workers" },
      { pattern: "ml_*", pool: "gpu_workers" }
    ]
  }
})

// Specialized worker pools
worker_pool.register_pool("payment_workers", {
  max_workers: 10,
  concurrency_limit: 5,  // Max 5 concurrent payment activities
  rate_limit: { requests: 100, window: 1m }
})

worker_pool.register_pool("gpu_workers", {
  max_workers: 4,
  requires: [GPU, "cuda>=11.0"]
})
```

### 3.3 Saga Pattern

#### 3.3.1 Saga Definition

```nulang
// Saga with compensation for each step
saga TravelBooking {
  input: { request: TravelRequest }
  
  step book_flight {
    activity: book_flight(request.flight),
    compensate: cancel_flight
  }
  
  step book_hotel {
    activity: book_hotel(request.hotel),
    compensate: cancel_hotel
  }
  
  step rent_car {
    activity: rent_car(request.car),
    compensate: cancel_car_rental,
    optional: true  // Skip if this step fails, don't compensate
  }
  
  step charge_payment {
    activity: process_payment(request.total),
    compensate: refund_payment
  }
  
  // Saga-level configuration
  compensation_order: Reverse,  // Reverse or Parallel
  on_compensation_failure: Escalate
}

// Compensation activities
@activity
fn cancel_flight(booking: FlightBooking) -> () {
  perform FlightAPI.cancel(booking.id)
}

@activity
fn cancel_hotel(booking: HotelBooking) -> () {
  perform HotelAPI.cancel(booking.id)
}

@activity
fn refund_payment(payment: Payment) -> RefundResult {
  perform PaymentGateway.refund(payment.id)
}
```

#### 3.3.2 Saga with Parallel Steps

```nulang
saga DistributedOrder {
  input: { order: Order }
  
  // These steps run in parallel
  parallel {
    step reserve_payment {
      activity: authorize_payment(order.total),
      compensate: release_payment_authorization
    }
    
    step reserve_inventory {
      activity: reserve_inventory(order.items),
      compensate: release_inventory
    }
    
    step validate_fraud {
      activity: fraud_check(order),
      compensate: None  // No compensation needed
    }
  }
  
  // Sequential after parallel completes
  step confirm_order {
    activity: capture_payment(payment_authorization)
    compensate: refund_payment
  }
  
  step create_shipment {
    activity: create_shipment(order)
    compensate: cancel_shipment
  }
}
```

### 3.4 Timer System

#### 3.4.1 Basic Timers

```nulang
// Sleep for a duration (durable)
perform Timer.sleep(30m)

// Sleep until a specific time
perform Timer.sleep_until(DateTime.parse("2024-12-25T00:00:00Z"))

// Sleep with business hours calculation
perform Timer.sleep_business_hours(
  duration: 8h,
  timezone: "America/New_York",
  work_days: [Mon, Tue, Wed, Thu, Fri],
  work_hours: { start: "09:00", end: "17:00" }
)
```

#### 3.4.2 Timer with Cancellation

```nulang
workflow ApprovalWithTimeout {
  input: { request: ApprovalRequest }
  
  step request_approval {
    activity send_approval_request(request)
  }
  
  step await_response {
    // Race between signal and timer
    race {
      signal "approval_response" as approval
      timer 48h as timeout
    }
    
    match winner {
      approval => {
        if approval.decision == "approved" {
          step implement {
            activity implement_change(request)
          }
        } else {
          step notify_rejection {
            activity notify_rejection(request, approval.reason)
          }
        }
      }
      timeout => {
        step auto_reject {
          activity auto_reject(request, "timeout")
        }
      }
    }
  }
}
```

#### 3.4.3 Recurring Timers

```nulang
// Recurring workflow execution
workflow PeriodicReport {
  input: { config: ReportConfig }
  
  // Runs every day at 9 AM
  schedule: cron("0 9 * * *")
  
  step generate_report {
    activity generate_daily_report(config)
  }
  
  step send_report {
    activity send_email(
      to: config.recipients,
      subject: "Daily Report",
      attachment: generate_report.result
    )
  }
}
```

### 3.5 Signal System

#### 3.5.1 Signal Definition

```nulang
// Define signals on a workflow
workflow OrderWorkflow {
  input: { order: Order }
  
  // Signal handlers
  @signal
  fn on_customer_cancel() {
    self.state.cancelled = true
    perform Workflow.cancel()
  }
  
  @signal
  fn on_address_update(new_address: Address) {
    self.state.order.shipping_address = new_address
    activity update_shipment_address(self.state.shipment_id, new_address)
  }
  
  @signal
  fn on_priority_upgrade(level: Priority) {
    self.state.priority = level
    activity escalate_order(self.state.order.id, level)
  }
  
  // ... workflow steps ...
}
```

#### 3.5.2 Sending Signals

```nulang
// Send signal to a running workflow
fn cancel_order_example() {
  // Find workflow by business key
  let workflow_id = Workflow.find_by_key("order-12345")
  
  // Send signal
  perform Workflow.signal(
    workflow_id: workflow_id,
    signal: "on_customer_cancel",
    payload: {}
  )
}

// Send with typed payload
perform Workflow.signal(
  workflow_id: workflow_id,
  signal: "on_address_update",
  payload: Address {
    street: "123 Main St",
    city: "Springfield",
    zip: "12345"
  }
)

// Batch signal
for order_id in urgent_orders {
  perform Workflow.signal(
    workflow_id: order_id,
    signal: "on_priority_upgrade",
    payload: Priority.Urgent
  )
}
```

#### 3.5.3 Signal with Payload Types

```nulang
// Typed signal definition
type ApprovalSignal = {
  approver_id: String,
  decision: "approve" | "reject",
  comments: Option<String>,
  timestamp: DateTime
}

workflow ApprovalWorkflow {
  input: { request: ChangeRequest }
  
  @signal
  fn on_approval_received(signal: ApprovalSignal) {
    self.state.approvals.push(signal)
    
    if signal.decision == "reject" {
      self.state.status = "rejected"
      perform Workflow.complete()
    }
    
    // Check if we have enough approvals
    let approval_count = self.state.approvals
      |> filter(|a| a.decision == "approve")
      |> count()
    
    if approval_count >= self.state.required_approvals {
      self.state.status = "approved"
      perform Workflow.complete()
    }
  }
}
```

### 3.6 Query System

#### 3.6.1 Query Definition

```nulang
workflow OrderFulfillment {
  input: { order: Order }
  
  // ... steps ...
  
  @query
  fn get_order_status() -> OrderStatus {
    self.state.status
  }
  
  @query
  fn get_progress() -> ProgressInfo {
    {
      completed_steps: self.state.completed_steps,
      total_steps: self.state.total_steps,
      percentage: (self.state.completed_steps / self.state.total_steps) * 100.0,
      current_activity: self.state.current_activity,
      started_at: self.state.started_at,
      estimated_completion: self.state.estimated_completion
    }
  }
  
  @query
  fn get_activities() -> [ActivityInfo] {
    self.state.activities.map(|a| {
      {
        name: a.name,
        status: a.status,
        started_at: a.started_at,
        completed_at: a.completed_at,
        attempts: a.attempts
      }
    })
  }
  
  @query
  fn get_errors() -> [ErrorInfo] {
    self.state.errors
  }
}
```

#### 3.6.2 Query Execution

```nulang
// Query a running workflow
let status = perform Workflow.query(
  workflow_id: "order-12345",
  query: "get_order_status"
)

// Query with type safety
let progress: ProgressInfo = perform Workflow.query(
  workflow_id: "order-12345",
  query: "get_progress"
)

println("Progress: {progress.percentage}%")
println("Current: {progress.current_activity}")

// List all running workflows
let running = perform Workflow.list(
  status: "running",
  workflow_type: "OrderFulfillment",
  limit: 100
)

for workflow in running {
  let progress = perform Workflow.query(
    workflow_id: workflow.id,
    query: "get_progress"
  )
  println("{workflow.id}: {progress.percentage}%")
}
```

### 3.7 Workflow Management API

#### 3.7.1 Starting Workflows

```nulang
// Start a new workflow
let handle = perform Workflow.start(
  workflow: PurchaseOrder,
  input: { order: my_order },
  options: {
    id: "order-{my_order.id}",           // Custom workflow ID
    business_key: my_order.id,             // For lookups
    timeout: 7d,                           // Workflow timeout
    priority: Priority.High,               // Execution priority
    parent_workflow: parent_id,            // For child workflows
    search_attributes: {                   // For querying
      customer_id: order.customer_id,
      order_total: order.total
    }
  }
)

println("Started workflow: {handle.id}")
println("Status: {handle.status}")

// Start and wait for result
let result = perform Workflow.start_and_wait(
  workflow: PurchaseOrder,
  input: { order: my_order },
  timeout: 30s  // How long to wait
)

match result {
  Ok(output) => println("Completed: {output}"),
  Timeout => println("Workflow still running"),
  Error(e) => println("Failed: {e}")
}
```

#### 3.7.2 Workflow Lifecycle Operations

```nulang
// Cancel a workflow
perform Workflow.cancel(workflow_id: "order-123")

// Terminate (force stop without compensation)
perform Workflow.terminate(
  workflow_id: "order-123",
  reason: "Emergency stop"
)

// Pause (stops processing but keeps state)
perform Workflow.pause(workflow_id: "order-123")

// Resume
perform Workflow.resume(workflow_id: "order-123")

// Reset to a specific step
perform Workflow.reset(
  workflow_id: "order-123",
  to_step: "charge_payment",
  reason: "Payment failed, retrying"
)
```

#### 3.7.3 Search and Discovery

```nulang
// Search workflows
let results = perform Workflow.search(
  query: "workflow_type = 'PurchaseOrder' AND status = 'running'",
  order_by: "started_at DESC",
  limit: 50
)

// Search by business key
let workflow = perform Workflow.find_by_key("order-12345")

// List with filters
let recent = perform Workflow.list({
  status: ["running", "completed"],
  started_after: now() - 24h,
  workflow_type: "PurchaseOrder",
  search_attributes: {
    customer_id: "cust-123"
  }
})

// Get workflow history
let history = perform Workflow.get_history(
  workflow_id: "order-123",
  include_details: true
)

for event in history.events {
  println("[{event.timestamp}] {event.type}: {event.description}")
}
```

### 3.8 Child Workflows

```nulang
workflow ParentProcess {
  input: { batch: [Order] }
  
  step process_batch {
    // Start child workflows in parallel
    let child_handles = []
    
    for order in batch {
      let handle = perform Workflow.start_child(
        workflow: PurchaseOrder,
        input: { order: order },
        options: {
          id: "child-order-{order.id}",
          cancellation_type: WaitForCancellation,
          parent_close_policy: Terminate  // Cancel children if parent fails
        }
      )
      child_handles.push(handle)
    }
    
    // Wait for all children to complete
    let results = perform Workflow.wait_for_children(
      child_handles,
      strategy: All,       // Wait for all
      timeout: 1h
    )
    
    // Aggregate results
    let completed = results
      |> filter(|r| r.status == "completed")
      |> count()
    
    let failed = results
      |> filter(|r| r.status == "failed")
      |> count()
    
    self.state.summary = { completed, failed }
  }
}
```

### 3.9 External System Integration

#### 3.9.1 Webhook Workflows

```nulang
// Workflow triggered by webhook
workflow WebhookProcessor {
  input: { webhook: WebhookPayload }
  
  @webhook(path: "/webhooks/payment", method: "POST")
  fn on_payment_webhook(payload: PaymentWebhook) {
    // Process payment webhook
    match payload.status {
      "succeeded" => {
        let workflow_id = Workflow.find_by_key(payload.order_id)
        perform Workflow.signal(
          workflow_id: workflow_id,
          signal: "payment_succeeded",
          payload: payload
        )
      }
      "failed" => {
        let workflow_id = Workflow.find_by_key(payload.order_id)
        perform Workflow.signal(
          workflow_id: workflow_id,
          signal: "payment_failed",
          payload: payload
        )
      }
    }
  }
}
```

#### 3.9.2 External Trigger

```nulang
// Start workflow from external event
fn on_kafka_message(message: KafkaMessage) {
  let event = JSON.parse(message.payload)
  
  perform Workflow.start(
    workflow: OrderProcessing,
    input: { order: event.order },
    options: {
      id: "order-{event.order.id}",
      business_key: event.order.id
    }
  )
}
```

---

## 4. Module Reference

### 4.1 Module Hierarchy

```
nulang-workflow/
├── core/
│   ├── workflow.nula       # Workflow definition DSL
│   ├── activity.nula       # Activity definition and execution
│   ├── saga.nula           # Saga pattern implementation
│   ├── step.nula           # Step types and execution
│   └── types.nula          # Core type definitions
├── runtime/
│   ├── engine.nula         # Workflow execution engine
│   ├── replayer.nula       # Event journal replay
│   ├── checkpoint.nula     # State checkpointing
│   ├── state_machine.nula  # Workflow state machine
│   └── context.nula        # Execution context
├── journal/
│   ├── writer.nula         # Journal append
│   ├── reader.nula         # Journal read/replay
│   ├── snapshot.nula       # State snapshots
│   └── storage/
│       ├── sqlite.nula     # SQLite backend
│       ├── postgres.nula   # PostgreSQL backend
│       └── kafka.nula      # Kafka backend
├── workers/
│   ├── pool.nula           # Worker pool management
│   ├── dispatcher.nula     # Task dispatch
│   ├── heartbeat.nula      # Heartbeat monitoring
│   └── routing.nula        # Task routing
├── signals/
│   ├── router.nula         # Signal routing
│   ├── handler.nula        # Signal handler dispatch
│   └── delivery.nula       # At-least-once delivery
├── timers/
│   ├── scheduler.nula      # Timer scheduling
│   ├── persistence.nula    # Timer persistence
│   └── firing.nula         # Timer firing
├── queries/
│   ├── handler.nula        # Query handler dispatch
│   └── validation.nula     # Query validation
├── child/
│   ├── parent.nula         # Parent workflow coordination
│   └── cancellation.nula   # Cancellation propagation
├── observability/
│   ├── tracing.nula        # Distributed tracing
│   ├── metrics.nula        # Workflow metrics
│   └── logging.nula        # Structured logging
└── testing/
    ├── test_runtime.nula   # In-memory test runtime
    ├── assertions.nula     # Workflow assertions
    └── mocking.nula        # Activity mocking
```

### 4.2 Core Types

```nulang
// Workflow types
type WorkflowId = String
type ActivityId = String
type RunId = String  // Unique per execution

type WorkflowHandle = {
  id: WorkflowId,
  run_id: RunId,
  status: WorkflowStatus,
  started_at: DateTime,
  completed_at: Option<DateTime>
}

enum WorkflowStatus {
  Running,
  Completed,
  Failed,
  Cancelled,
  Terminated,
  Paused,
  TimedOut
}

// Event types
type WorkflowEvent = {
  sequence: Int,
  timestamp: DateTime,
  payload: EventPayload
}

enum EventPayload {
  WorkflowStarted { input: JSON },
  ActivityScheduled { activity_id: String, name: String, input: JSON },
  ActivityStarted { activity_id: String },
  ActivityCompleted { activity_id: String, result: JSON },
  ActivityFailed { activity_id: String, error: String, attempt: Int },
  TimerScheduled { timer_id: String, fire_at: DateTime },
  TimerFired { timer_id: String },
  SignalReceived { signal_name: String, payload: JSON },
  SignalProcessed { signal_name: String },
  QueryReceived { query_name: String },
  WorkflowCompleted { result: JSON },
  WorkflowFailed { error: String },
  CompensationStarted { step: String },
  CompensationCompleted { step: String },
  CompensationFailed { step: String, error: String }
}

// Activity types
type ActivityOptions = {
  retry: RetryPolicy,
  timeout: Duration,
  heartbeat: Option<Duration>,
  task_queue: String,
  priority: TaskPriority
}

type RetryPolicy = {
  max_attempts: Int,
  backoff: BackoffStrategy,
  retry_on: [ErrorType],
  give_up_on: [ErrorType],
  non_retryable_errors: [String]
}

type BackoffStrategy =
  | Exponential { initial: Duration, max: Duration, multiplier: Float, jitter: Float }
  | Linear { interval: Duration }
  | Fixed { delay: Duration }

// Timer types
type TimerOptions = {
  duration: Duration,
  absolute: Option<DateTime>,
  business_hours: Option<BusinessHoursConfig>,
  recurring: Option<CronExpression>
}
```

### 4.3 Effect Definitions

```nulang
// Core workflow effects
effect WorkflowRuntime {
  fn start_activity(name: String, input: JSON, options: ActivityOptions) -> JSON;
  fn schedule_timer(duration: Duration) -> TimerHandle;
  fn wait_for_signal(name: String, timeout: Duration) -> Option<JSON>;
  fn query_state(query: StateQuery) -> JSON;
  fn checkpoint(state: JSON) -> ();
}

effect TimerService {
  fn sleep(duration: Duration) -> ();
  fn sleep_until(datetime: DateTime) -> ();
  fn schedule(fire_at: DateTime) -> TimerId;
  fn cancel(timer_id: TimerId) -> ();
}

effect SignalService {
  fn send(workflow_id: WorkflowId, signal: String, payload: JSON) -> ();
  fn broadcast(workflow_type: String, signal: String, payload: JSON) -> ();
  fn wait(signal: String, timeout: Duration) -> Option<JSON>;
}
```

---

## 5. Implementation Phases

### 5.1 Phase 1: Core Runtime (Weeks 1-6)

**Goal:** Build the foundational workflow execution engine with event journal and replay.

```
Milestone: v0.1.0 — "Execute"
+---------------------------------------------------------------+
| Week 1-2            | Week 3-4              | Week 5-6        |
+---------------------+-----------------------+-----------------+
| Event journal       | Workflow engine       | Activity        |
|                     |                       | execution       |
| - Append-only       | - State machine       |                 |
|   log design        | - Event replay        | - Worker pool   |
| - Journal writer    | - Checkpointing       | - Task dispatch |
| - Journal reader    | - Workflow lifecycle  | - Heartbeat     |
|                     |                       |   monitoring    |
| SQLite backend      | Deterministic         | - Retry logic   |
|                     | execution             |                 |
+---------------------+-----------------------+-----------------+
| Deliverable: Can execute basic workflows with durability      |
| Tests: Unit tests for journal, integration tests for engine   |
+---------------------------------------------------------------+
```

**Key Tasks:**
- [ ] Design append-only event journal format
- [ ] Implement journal writer (append events)
- [ ] Implement journal reader (replay events)
- [ ] Build SQLite persistence backend
- [ ] Implement workflow state machine
- [ ] Build deterministic execution context
- [ ] Implement event replay for recovery
- [ ] Build activity worker pool
- [ ] Implement task dispatch and routing
- [ ] Add heartbeat monitoring
- [ ] Implement retry logic with backoff
- [ ] Write comprehensive test suite

### 5.2 Phase 2: Activities & Timers (Weeks 7-12)

**Goal:** Full activity system with retries, timers, and signals.

```
Milestone: v0.2.0 — "Orchestrate"
+---------------------------------------------------------------+
| Week 7-8            | Week 9-10             | Week 11-12      |
+---------------------+-----------------------+-----------------+
| Activity system     | Timer system          | Signal system   |
|                     |                       |                 |
| - @activity macro   | - Durable timer       | - Signal        |
| - Retry policies    |   persistence         |   definition    |
| - Timeout handling  | - Timer scheduler     | - Signal router |
| - Heartbeat system  | - Cron expressions    | - Signal        |
| - Context access    | - Business hours      |   delivery      |
| - Activity registry | - Timer cancellation  | - Race between  |
|                     |                       |   timer/signal  |
+---------------------+-----------------------+-----------------+
| Deliverable: Full activity orchestration with timers/signals  |
| Tests: Saga patterns, timer accuracy, signal delivery         |
+---------------------------------------------------------------+
```

**Key Tasks:**
- [ ] Implement `@activity` macro with full options
- [ ] Build configurable retry policies
- [ ] Implement timeout handling and cancellation
- [ ] Create heartbeat monitoring system
- [ ] Build activity context API
- [ ] Implement durable timer persistence
- [ ] Build timer scheduler with accuracy guarantees
- [ ] Add cron expression support
- [ ] Implement business hours timer
- [ ] Build signal definition and routing
- [ ] Implement signal delivery (at-least-once)
- [ ] Add race between signals and timers
- [ ] Write integration tests

### 5.3 Phase 3: Sagas & Queries (Weeks 13-18)

**Goal:** Saga compensation, queries, and advanced patterns.

```
Milestone: v0.3.0 — "Transact"
+---------------------------------------------------------------+
| Week 13-14          | Week 15-16            | Week 17-18      |
+---------------------+-----------------------+-----------------+
| Saga system         | Query system          | Advanced        |
|                     |                       | patterns        |
| - Saga DSL          | - @query macro        | - Child         |
| - Compensation      | - Query routing       |   workflows     |
|   execution         | - Read-only access    | - Parallel      |
| - Reverse           | - Query validation    |   execution     |
|   compensation      | - Typed responses     | - Workflow      |
| - Parallel          |                       |   versioning    |
|   compensation      | State search          | - Schema        |
| - Saga monitoring   | - Search by attrs     |   evolution     |
+---------------------+-----------------------+-----------------+
| Deliverable: Full saga support with queries and search        |
| Tests: Saga compensation scenarios, query correctness         |
+---------------------------------------------------------------+
```

**Key Tasks:**
- [ ] Implement `saga {}` DSL
- [ ] Build compensation execution engine
- [ ] Add reverse and parallel compensation modes
- [ ] Implement saga failure handling
- [ ] Build `@query` macro and handler
- [ ] Implement query routing and validation
- [ ] Add workflow search by attributes
- [ ] Implement child workflow support
- [ ] Build parallel execution branches
- [ ] Add workflow versioning
- [ ] Implement schema evolution
- [ ] Write comprehensive tests

### 5.4 Phase 4: Observability & Scale (Weeks 19-24)

**Goal:** Production readiness with observability and performance.

```
Milestone: v0.4.0 — "Production"
+---------------------------------------------------------------+
| Week 19-20          | Week 21-22            | Week 23-24      |
+---------------------+-----------------------+-----------------+
| Observability       | Performance           | Polish          |
|                     |                       |                 |
| - OpenTelemetry     | - PostgreSQL          | - Documentation |
|   tracing           |   backend             | - Examples      |
| - Workflow metrics  | - Kafka journal       | - Benchmarks    |
| - Structured        | - Worker scaling      | - Security      |
|   logging           | - Connection pooling  |   audit         |
| - Health checks     | - Async I/O           | - Release       |
|   events            | - Memory optimization |   checklist     |
+---------------------+-----------------------+-----------------+
| Deliverable: Production-ready workflow engine                 |
| Tests: Load tests, chaos tests, fail-over tests               |
+---------------------------------------------------------------+
```

### 5.5 Phase 5: Advanced Features (Weeks 25-30)

**Goal:** Advanced workflow patterns and ecosystem integration.

```
Milestone: v1.0.0 — "Complete"
+---------------------------------------------------------------+
| Week 25-26          | Week 27-28            | Week 29-30      |
+---------------------+-----------------------+-----------------+
| Advanced patterns   | Ecosystem             | Final release   |
|                     |                       |                 |
| - Dynamic workflows | - HTTP webhooks       | - Performance   |
| - Workflow          | - Kafka integration   |   tuning        |
|   composition       | - gRPC activities     | - Final docs    |
| - Human-in-the-loop | - Scheduled workflows | - Community     |
| - Batch processing  | - Multi-region        |   feedback      |
| - Event sourcing    |   replication         | - v1.0.0 tag    |
+---------------------+-----------------------+-----------------+
```

---

## 6. Appendices

### 6.1 Comparison with Existing Systems

| Feature | Temporal | Cadence | AWS Step Functions | Nulang Workflow |
|---------|----------|---------|-------------------|-----------------|
| Language | Multi-SDK | Go/Java | JSON/Visual | Native Nulang |
| Actor model | No | No | No | Yes (built-in) |
| Deterministic | Yes | Yes | Yes | Yes |
| Saga pattern | Manual | Manual | Partial | Native |
| Local dev | Docker required | Docker required | Cloud only | Built-in |
| Performance | High | High | Moderate | High (native) |
| Effect system | No | No | No | Yes |
| Pattern matching | No | No | No | Yes |

### 6.2 Configuration Reference

```nulang
// Workflow engine configuration
config WorkflowEngine {
  // Persistence
  journal: {
    backend: SQLite | PostgreSQL | Kafka,
    sqlite: {
      path: "./data/workflows.db",
      pool_size: 10
    },
    postgres: {
      url: env("DATABASE_URL"),
      pool_size: 20
    }
  },
  
  // Workers
  workers: {
    max_concurrent: 100,
    task_queue_size: 10000,
    heartbeat_grace_period: 30s,
    max_heartbeat_interval: 10s
  },
  
  // Timeouts
  defaults: {
    workflow_timeout: 24h,
    activity_timeout: 5m,
    activity_retry_max: 3,
    activity_heartbeat: 30s
  },
  
  // Observability
  tracing: {
    enabled: true,
    exporter: OTLP { endpoint: "http://localhost:4317" }
  },
  
  metrics: {
    enabled: true,
    endpoint: "0.0.0.0:9090"
  },
  
  // Snapshotting
  snapshots: {
    enabled: true,
    interval_events: 100,
    max_events_without_snapshot: 200
  }
}
```

### 6.3 Error Code Reference

```nulang
enum WorkflowError {
  // Engine errors
  WorkflowNotFound { id: WorkflowId },
  WorkflowAlreadyExists { id: WorkflowId },
  InvalidWorkflowState { id: WorkflowId, expected: WorkflowStatus, actual: WorkflowStatus },
  
  // Activity errors
  ActivityNotFound { name: String },
  ActivityTimeout { id: ActivityId, elapsed: Duration },
  ActivityHeartbeatTimeout { id: ActivityId },
  MaxRetriesExceeded { id: ActivityId, attempts: Int },
  
  // Timer errors
  TimerNotFound { id: TimerId },
  TimerAlreadyCancelled { id: TimerId },
  
  // Signal errors
  SignalNotHandled { workflow_id: WorkflowId, signal: String },
  InvalidSignalPayload { signal: String, error: String },
  
  // Saga errors
  SagaCompensationFailed { step: String, original_error: String, compensation_error: String },
  SagaStepFailed { step: String, error: String },
  
  // Query errors
  QueryNotFound { name: String },
  QueryTimeout { name: String, elapsed: Duration },
  InvalidQueryAccess { query: String, reason: String }
}
```

### 6.4 Performance Targets

| Metric | Target | Measurement |
|--------|--------|-------------|
| Workflow start latency | < 10ms | P99 |
| Activity dispatch latency | < 5ms | P99 |
| Event journal append | < 1ms | P99 (SQLite) |
| State replay rate | > 10K events/sec | Recovery |
| Timer accuracy | < 100ms drift | At scale |
| Signal delivery latency | < 5ms | P99 |
| Query response time | < 1ms | P99 |
| Concurrent workflows | > 100K | Per node |

### 6.5 Testing Patterns

```nulang
// Testing a workflow
test "PurchaseOrder completes successfully" {
  // Setup test runtime
  let runtime = WorkflowTestRuntime.new()
  
  // Mock activities
  runtime.mock_activity("validate_order", |input| {
    ValidationResult.valid()
  })
  
  runtime.mock_activity("process_payment", |input| {
    PaymentResult.success("pay-123")
  })
  
  runtime.mock_activity("create_shipment", |input| {
    ShipmentResult.success("ship-456")
  })
  
  // Execute workflow
  let result = runtime.execute(
    workflow: PurchaseOrder,
    input: { order: test_order }
  )
  
  // Assert
  assert(result.status == "completed")
  assert(result.payment_id == "pay-123")
  assert(result.shipment_id == "ship-456")
  
  // Verify activity calls
  assert(runtime.was_called("validate_order"))
  assert(runtime.call_count("process_payment") == 1)
}

// Testing saga compensation
test "Saga compensates on failure" {
  let runtime = WorkflowTestRuntime.new()
  
  runtime.mock_activity("book_flight", |input| {
    FlightBooking.success("flight-789")
  })
  
  runtime.mock_activity("book_hotel", |input| {
    HotelBooking.success("hotel-101")
  })
  
  // Make payment fail
  runtime.mock_activity("process_payment", |input| {
    PaymentResult.failure("insufficient_funds")
  })
  
  // These should be called during compensation
  let compensations = []
  runtime.mock_activity("cancel_flight", |input| {
    compensations.push("cancel_flight")
    Ok(())
  })
  runtime.mock_activity("cancel_hotel", |input| {
    compensations.push("cancel_hotel")
    Ok(())
  })
  
  let result = runtime.execute(TravelBooking, { request: test_request })
  
  assert(result.status == "failed")
  assert(compensations.contains("cancel_flight"))
  assert(compensations.contains("cancel_hotel"))
}
```

### 6.6 Glossary

| Term | Definition |
|------|------------|
| **Workflow** | A durable, deterministic, long-running process |
| **Activity** | A unit of work that can be retried independently |
| **Saga** | A long-running transaction with compensation |
| **Event Journal** | Append-only log of all workflow events |
| **Checkpoint** | Persisted workflow state for recovery |
| **Signal** | External event that advances workflow state |
| **Query** | Read-only access to workflow state |
| **Timer** | Durable timer that survives restarts |
| **Compensation** | Undo action for a saga step |
| **Deterministic** | Given same input/events, always same output |

---

*Document Version: 1.0.0*  
*Last Updated: 2024*  
*Status: Ready for Implementation*
