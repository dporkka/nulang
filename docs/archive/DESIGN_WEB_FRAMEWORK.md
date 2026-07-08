# Nulang Web Framework Design Document

## Overview

The Nulang Web Framework (`phoenix-nl`) is a high-productivity web framework for the Nulang programming language, drawing heavy inspiration from Phoenix (Elixir), ASP.NET Core, and FastAPI. It leverages Nulang's actor model for massive concurrency, pattern matching for elegant request handling, and the effect system for composable middleware. The framework provides real-time communication via WebSockets, template rendering, database integration, and live update capabilities out of the box.

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

### 1.1 Endpoint

An **Endpoint** is the HTTP entry point for a Nulang web application. It configures the HTTP server, routes, middleware pipeline, static file serving, and WebSocket upgrade handling. Each Nulang web application has at least one endpoint.

```nulang
endpoint MyApp do
  // HTTP server configuration
  http: { port: 4000, host: "0.0.0.0" },
  
  // HTTPS configuration
  https: {
    port: 443,
    certfile: "/etc/ssl/cert.pem",
    keyfile: "/etc/ssl/key.pem"
  },
  
  // Middleware pipeline
  plug RequestLogger
  plug CorsPlug, origins: ["https://myapp.com"]
  plug AuthenticationPlug
  plug RateLimitPlug, max_requests: 1000, window: 1m
  
  // Route definitions
  route "/", HomeController
  route "/api/users", UsersController
  route "/api/posts", PostsController
  route "/ws/chat", ChatChannel
  
  // Static file serving
  static "/assets", from: "priv/static"
  static "/uploads", from: "priv/uploads"
  
  // Error handling
  error_handler MyApp.ErrorHandler
end
```

### 1.2 Controller

A **Controller** handles HTTP requests using Nulang's pattern matching. Controllers are actor-based, meaning each request is processed concurrently without blocking. Pattern matching on routes enables elegant, declarative request handling.

```nulang
controller UsersController {
  // GET /api/users
  def index(conn) {
    let users = perform Database.query("SELECT * FROM users")
    json(conn, 200, users)
  }
  
  // GET /api/users/:id
  def show(conn, %{id: id}) {
    match perform Database.get(User, id) {
      Some(user) => json(conn, 200, user),
      None => json(conn, 404, %{error: "User not found"})
    }
  }
  
  // POST /api/users
  def create(conn) {
    let params = conn.body_params
    
    match User.validate(params) {
      Ok(valid_params) => {
        let user = perform Database.insert(User, valid_params)
        json(conn, 201, user)
      }
      Error(errors) => json(conn, 422, %{errors: errors})
    }
  }
}
```

### 1.3 Channel

A **Channel** provides WebSocket-based real-time bidirectional communication. Built on Nulang's actor model, each channel connection spawns a lightweight actor that can broadcast messages to topics, handle push events from clients, and maintain connection state.

```nulang
channel ChatChannel {
  // Join a room topic
  def join("room:" <> room_id, payload, socket) {
    // Authenticate join
    if payload["token"] |> Auth.verify() {
      // Subscribe to the room topic
      subscribe(socket, "room:{room_id}")
      
      // Send recent messages
      let history = perform ChatStore.get_history(room_id, limit: 50)
      
      {:ok, assign(socket, :room, room_id)
            |> assign(:user, payload["user"])
            |> push("history", history)}
    } else {
      {:error, %{reason: "unauthorized"}}
    }
  }
  
  // Handle incoming message
  def handle_in("new_msg", %{"body" => body}, socket) {
    let message = {
      user: socket.assigns.user,
      body: body,
      timestamp: now()
    }
    
    // Persist message
    perform ChatStore.save_message(socket.assigns.room, message)
    
    // Broadcast to all subscribers of the room topic
    broadcast(socket, "new_msg", message)
    
    {:noreply, socket}
  }
  
  // Handle typing indicators
  def handle_in("typing", %{"is_typing" => is_typing}, socket) {
    broadcast_from(socket, "typing", %{
      user: socket.assigns.user,
      is_typing: is_typing
    })
    
    {:noreply, socket}
  }
  
  // Handle presence changes
  def handle_info(%{event: "presence_diff"} = msg, socket) {
    push(socket, "presence_diff", msg.payload)
    {:noreply, socket}
  }
}
```

### 1.4 View & Templates

**Views** transform data into response formats. **Templates** provide HTML rendering with a Nulang-native template syntax.

```nulang
// View module
view UserView {
  def render("index.json", %{users: users}) {
    %{users: users.map(|u| render_one(u, "show.json"))}
  }
  
  def render("show.json", %{user: user}) {
    %{
      id: user.id,
      name: user.name,
      email: user.email,
      created_at: user.created_at
    }
  }
  
  def render("error.json", %{errors: errors}) {
    %{errors: errors}
  }
}

// HTML Template (templates/users/index.html.nula)
@template UsersIndex {
  <div class="users-list">
    <h1>Users</h1>
    
    @for user <- @users {
      <div class="user-card">
        <h2>{user.name}</h2>
        <p>{user.email}</p>
        <span class="badge">{user.role}</span>
      </div>
    }
    
    @if @users |> length() == 0 {
      <p class="empty">No users found.</p>
    }
    
    <pagination 
      page={@page} 
      total={@total_pages}
      base_url="/users"
    />
  </div>
}
```

### 1.5 Live Updates

**Live Updates** enable server-push updates to connected clients without polling. Built on top of Channels, Live Views allow developers to build reactive UIs where the server renders HTML and pushes diffs to the client.

```nulang
liveview DashboardLive {
  // Mount the live view
  def mount(_params, session, socket) {
    // Subscribe to data updates
    subscribe(socket, "metrics:updates")
    
    // Schedule periodic refresh
    schedule_interval(socket, :refresh, 5s)
    
    {:ok, assign(socket, 
      metrics: fetch_metrics(),
      users_online: fetch_user_count(),
      requests_per_second: 0
    )}
  }
  
  // Handle parameter changes (e.g., URL changes)
  def handle_params(params, _uri, socket) {
    let filter = params["filter"] || "all"
    
    {:noreply, assign(socket, 
      filter: filter,
      metrics: fetch_metrics(filter)
    )}
  }
  
  // Handle client events
  def handle_event("refresh", _params, socket) {
    {:noreply, assign(socket, metrics: fetch_metrics())}
  }
  
  def handle_event("set_time_range", %{"range" => range}, socket) {
    {:noreply, assign(socket, 
      time_range: range,
      metrics: fetch_metrics(range: range)
    )}
  }
  
  // Handle server events (broadcasts)
  def handle_info(%{event: "metrics:update"} = msg, socket) {
    {:noreply, assign(socket, metrics: msg.payload)}
  }
  
  def handle_info(:refresh, socket) {
    {:noreply, assign(socket, metrics: fetch_metrics())}
  }
  
  // Render function (server-side)
  def render(assigns) {
    <div class="dashboard">
      <h1>System Dashboard</h1>
      
      <div class="metrics-grid">
        <metric_card 
          title="Users Online" 
          value={@users_online}
          trend="+12%"
        />
        <metric_card 
          title="Req/s" 
          value={@requests_per_second}
        />
        <metric_card 
          title="Avg Latency" 
          value="{@metrics.avg_latency}ms"
        />
      </div>
      
      <button phx-click="refresh">Refresh</button>
    </div>
  }
}
```

---

## 2. Architecture Overview

### 2.1 System Architecture

```
+============================================================================+
|                     Nulang Web Framework Architecture                      |
+============================================================================+
|                                                                            |
|  +---------------------+  +---------------------+  +--------------------+ |
|  |   HTTP Request      |  |   WebSocket         |  |   Static File      | |
|  |   Pipeline          |  |   Channel           |  |   Serving          | |
|  |                     |  |   Pipeline          |  |                    | |
|  | - Router            |  |                     |  | - Cache headers    | |
|  | - Middleware        |  | - Connection mgmt   |  | - ETag support     | |
|  | - Controller        |  | - Topic pub/sub     |  | - Range requests   | |
|  | - View              |  | - Presence tracking |  | - Compression      | |
|  | - Response          |  | - Heartbeat         |  | - MIME types       | |
|  +--------+------------+  +----------+----------+  +---------+----------+ |
|           |                          |                       |            |
+-----------+--------------------------+-----------------------+------------+
|           |                          |                       |            |
|  +--------v------------+  +----------v----------+  +---------v----------+ |
|  |   Endpoint Layer    |  |   Transport Layer   |  |   Asset Pipeline   | |
|  |                     |  |                     |  |                    | |
|  | - HTTP/1.1          |  | - WebSocket         |  | - Sass/LESS        | |
|  | - HTTP/2            |  |   upgrade           |  | - Minification     | |
|  | - HTTPS/TLS         |  | - Binary frames     |  | - Fingerprinting   | |
|  | - Keep-alive        |  | - Compression       |  | - Source maps      | |
|  +--------+------------+  +----------+----------+  +---------+----------+ |
|           |                          |                       |            |
+-----------+--------------------------+-----------------------+------------+
|           |                          |                       |            |
|  +--------v------------+  +----------v----------+  +---------v----------+ |
|  |   Actor Layer       |  |   Pub/Sub Layer     |  |   Storage Layer    | |
|  |                     |  |                     |  |                    | |
|  | - Request actors    |  | - Topic registry    |  | - Session store    | |
|  | - Channel actors    |  | - Broadcast         |  | - Upload handling  | |
|  | - LiveView actors   |  | - Presence          |  | - Temp files       | |
|  | - Supervision       |  | - Backpressure      |  | - Streaming        | |
|  +---------------------+  +---------------------+  +--------------------+ |
|                                                                            |
|  +---------------------+  +---------------------+  +--------------------+ |
|  |   Middleware        |  |   Configuration     |  |   Observability    | |
|  |   System            |  |   System            |  |   System           | |
|  |                     |  |                     |  |                    | |
|  | - Request logging   |  | - Environment vars  |  | - Request tracing  | |
|  | - Authentication    |  | - Secret management |  | - Metrics export   | |
|  | - Authorization     |  | - Runtime config    |  | - Structured logs  | |
|  | - CORS              |  | - Feature flags     |  | - Health checks    | |
|  | - Rate limiting     |  | - Cluster config    |  | - Error tracking   | |
|  +---------------------+  +---------------------+  +--------------------+ |
|                                                                            |
+============================================================================+
```

### 2.2 Request Lifecycle

```
+---------------------------------------------------------------------+
|                     HTTP Request Lifecycle                           |
+---------------------------------------------------------------------+
|                                                                      |
|   Request                                                            |
|     |                                                                |
|     v                                                                |
|  +------------------+     +------------------+     +--------------+ |
|  | 1. Transport     |     | 2. Endpoint      |     | 3. Router    | |
|  |    Parse HTTP    | --> |    Apply         | --> |    Match     | |
|  |    headers       |     |    middleware    |     |    route     | |
|  |    body          |     |    (pipeline)    |     |    pattern   | |
|  +------------------+     +------------------+     +------+-------+ |
|                                                             |        |
|                                                             v        |
|                                                  +-----------------+ |
|                                                  | 4. Controller    | |
|                                                  |    Pattern match | |
|                                                  |    Extract params| |
|                                                  |    Call action   | |
|                                                  +--------+--------+ |
|                                                           |          |
|                                                           v          |
|                                                  +-----------------+ |
|                                                  | 5. View/Template | |
|                                                  |    Transform data| |
|                                                  |    Render HTML   | |
|                                                  |    or JSON       | |
|                                                  +--------+--------+ |
|                                                           |          |
|                                                           v          |
|                                                  +-----------------+ |
|                                                  | 6. Response      | |
|                                                  |    Set headers   | |
|                                                  |    Encode body   | |
|                                                  |    Send          | |
|                                                  +--------+--------+ |
|                                                           |          |
|                                                           v          |
|                                                      Response        |
+---------------------------------------------------------------------+
```

### 2.3 WebSocket Channel Architecture

```
+---------------------------------------------------------------------+
|                     WebSocket Channel Architecture                   |
+---------------------------------------------------------------------+
|                                                                      |
|   Client 1          Client 2          Client 3                       |
|      |                 |                 |                           |
|      v                 v                 v                           |
|  +--------+        +--------+        +--------+                      |
|  | WS     |        | WS     |        | WS     |                      |
|  | Conn   |        | Conn   |        | Conn   |                      |
|  +---+----+        +---+----+        +---+----+                      |
|      |                 |                 |                           |
|      v                 v                 v                           |
|  +--------+        +--------+        +--------+                      |
|  | Socket |        | Socket |        | Socket |                      |
|  | Actor  |        | Actor  |        | Actor  |                      |
|  +---+----+        +---+----+        +---+----+                      |
|      |                 |                 |                           |
|      |   join("room:1") |   join("room:1") |   join("room:2")        |
|      |                 |                 |                           |
|      v                 v                 v                           |
|  +-------------------------------------------+                       |
|  |           Pub/Sub Topic Registry           |                       |
|  |                                            |                       |
|  |   "room:1"  --> [Socket1, Socket2]         |                       |
|  |   "room:2"  --> [Socket3]                  |                       |
|  |                                            |                       |
|  |   Broadcast: "new_msg" to "room:1"         |                       |
|  |   --> Socket1 receives                     |                       |
|  |   --> Socket2 receives                     |                       |
|  |                                            |                       |
|  +--------------------------------------------+                       |
|                                                                      |
+---------------------------------------------------------------------+
```

### 2.4 LiveView Architecture

```
+---------------------------------------------------------------------+
|                     LiveView Architecture                            |
+---------------------------------------------------------------------+
|                                                                      |
|   Browser                        Server                              |
|                                                                      |
|   +--------+                     +------------+                      |
|   | Initial|  HTTP GET           | Endpoint   |                      |
|   | Request| ------------------> | Router     |                      |
|   |        |                     | Controller|                      |
|   |        | <------------------ | Renders    |                      |
|   |        |   HTML + JS         | HTML       |                      |
|   +---+----+                     +-----+------+                      |
|       |                                |                             |
|       |  WebSocket upgrade             |                             |
|       | ------------------------------>                              |
|       |                                |                             |
|       |                     +----------v-----------+                 |
|       |                     | LiveView Actor       |                 |
|       |                     | - Holds state        |                 |
|       |                     | - Renders template   |                 |
|       |                     | - Handles events     |                 |
|       |                     +----------+-----------+                 |
|       |                                |                             |
|       |  Push: diff                    |                             |
|       | <------------------------------                              |
|       |                                |                             |
|       |  Click: "inc"                  |                             |
|       | ------------------------------>                              |
|       |                                |                             |
|       |                     +----------v-----------+                 |
|       |                     | handle_event        |                 |
|       |                     | -> update state     |                 |
|       |                     | -> re-render        |                 |
|       |                     | -> push diff        |                 |
|       |                     +----------+-----------+                 |
|       |                                |                             |
|       |  Push: diff                    |                             |
|       | <------------------------------                              |
|       |                                |                             |
+-------+--------------------------------+-----------------------------+
```

---

## 3. API Design & Specification

### 3.1 Endpoint Definition

#### 3.1.1 Basic Endpoint

```nulang
// Minimal endpoint
endpoint MyApp do
  route "/", HomeController
end

// Full-featured endpoint
endpoint MyApp do
  // Server configuration
  server: {
    http: { port: 4000, host: "0.0.0.0", compress: true },
    https: {
      port: 443,
      certfile: env("SSL_CERT_PATH"),
      keyfile: env("SSL_KEY_PATH"),
      ciphers: :strong
    },
    http2: true,
    websocket: { path: "/live", timeout: 60s }
  },
  
  // Request configuration
  request: {
    max_body_length: 10_000_000,  // 10MB
    timeout: 30s,
    parsers: [JSONParser, URLencodedParser, MultipartParser]
  },
  
  // Middleware pipeline (executed in order)
  plug RequestIdPlug
  plug RequestLoggerPlug, format: :combined
  plug CorsPlug, 
    origins: ["https://app.example.com", "http://localhost:3000"],
    methods: ["GET", "POST", "PUT", "DELETE"],
    headers: ["authorization", "content-type"]
  plug SecurityHeadersPlug, 
    hsts: true,
    x_frame_options: "DENY",
    csp: "default-src 'self'"
  plug AuthenticationPlug, 
    strategy: JWT { secret: env("JWT_SECRET") }
  plug RateLimitPlug,
    max_requests: 1000,
    window: 1m,
    key: |conn| conn.remote_ip
  
  // Route definitions
  scope "/" do
    route "/", HomeController, :index
    route "/about", HomeController, :about
    route "/health", HealthController, :check
  end
  
  scope "/api/v1" do
    plug ApiAuthenticationPlug
    
    route "/users", UsersController
    route "/posts", PostsController
    route "/comments", CommentsController
    
    // Nested resources
    resources "/projects", ProjectsController do
      route "/tasks", TasksController
      route "/members", ProjectMembersController
    end
  end
  
  // WebSocket channels
  channel "room:*", ChatChannel
  channel "notifications:*", NotificationChannel
  channel "presence:*", PresenceChannel
  
  // LiveView routes
  live "/dashboard", DashboardLive
  live "/admin/users", AdminUsersLive
  live "/admin/settings", AdminSettingsLive
  
  // Static file serving
  static "/assets", from: "priv/static/assets", 
    headers: [{"cache-control", "max-age=31536000"}]
  static "/uploads", from: "priv/static/uploads",
    headers: [{"cache-control", "max-age=86400"}]
  
  // Error handling
  error_handler MyApp.ErrorHandler,
    render_404: "errors/404.html",
    render_500: "errors/500.html",
    log_level: :error
end
```

#### 3.1.2 Router Pattern Matching

```nulang
// Advanced routing with pattern matching
endpoint AdvancedRouter do
  // Exact match
  route "/", HomeController, :index
  
  // Named parameters
  route "/users/:id", UsersController, :show
  route "/users/:id/posts/:post_id", PostsController, :show
  
  // Pattern guards
  route "/api/:version/users", UsersController, :index,
    guard: |params| params.version in ["v1", "v2"]
  
  // Method-specific routes
  get "/users", UsersController, :index
  post "/users", UsersController, :create
  put "/users/:id", UsersController, :update
  delete "/users/:id", UsersController, :delete
  patch "/users/:id", UsersController, :patch
  
  // Route with constraints
  route "/users/:id", UsersController, :show,
    constraints: %{id: ~r/^\d+$/}  // Only numeric IDs
  
  // Route with custom matcher
  route "/:date/posts", PostsController, :by_date,
    constraints: %{date: ~r/^\d{4}-\d{2}-\d{2}$/}
  
  // Glob routes
  route "/files/*path", FilesController, :serve
  
  // Redirects
  redirect "/old-path", to: "/new-path", status: 301
  redirect "/docs", to: "https://docs.example.com", external: true
end
```

### 3.2 Controller API

#### 3.2.1 Basic Controller

```nulang
controller UsersController {
  // GET /users
  def index(conn) {
    let page = conn.query_params["page"] |> int_or(1)
    let per_page = conn.query_params["per_page"] |> int_or(20)
    
    let users = perform Database.query(User, 
      limit: per_page,
      offset: (page - 1) * per_page
    )
    
    let total = perform Database.count(User)
    
    json(conn, 200, %{
      data: users,
      pagination: %{
        page: page,
        per_page: per_page,
        total: total,
        total_pages: ceil(total / per_page)
      }
    })
  }
  
  // GET /users/:id
  def show(conn, %{id: id}) {
    match perform Database.get(User, id) {
      Some(user) => json(conn, 200, user),
      None => json(conn, 404, %{error: "Not found"})
    }
  }
  
  // POST /users
  def create(conn) {
    match User.create(conn.body_params) {
      Ok(user) => json(conn, 201, user),
      Error(changeset) => json(conn, 422, %{errors: changeset.errors})
    }
  }
  
  // PUT /users/:id
  def update(conn, %{id: id}) {
    let user = perform Database.get!(User, id)
    let changeset = User.update(user, conn.body_params)
    
    match perform Database.update(changeset) {
      Ok(updated) => json(conn, 200, updated),
      Error(changeset) => json(conn, 422, %{errors: changeset.errors})
    }
  }
  
  // DELETE /users/:id
  def delete(conn, %{id: id}) {
    let user = perform Database.get!(User, id)
    perform Database.delete!(user)
    send_resp(conn, 204, "")
  }
}
```

#### 3.2.2 Advanced Controller Patterns

```nulang
controller PostsController {
  // Action with before/after hooks
  @before :authenticate_user
  @before :load_post, only: [:show, :update, :delete]
  
  def index(conn) {
    let posts = perform Database.query(Post, 
      where: conn.assigns.current_user.can_see,
      order: :created_at_desc,
      preload: [:author, :comments]
    )
    
    json(conn, 200, %{posts: posts})
  }
  
  def show(conn, %{id: id}) {
    // @before already loaded @post
    json(conn, 200, conn.assigns.post)
  }
  
  // Streaming response
  def stream_large_file(conn, %{id: id}) {
    let file = perform Storage.get_file(id)
    
    conn
    |> put_resp_header("content-type", file.mime_type)
    |> put_resp_header("content-disposition", "attachment; filename={file.name}")
    |> send_chunked(200)
    |> stream_file(file.path, chunk_size: 64_000)
  }
  
  // Server-sent events
  def events(conn) {
    conn = put_resp_header(conn, "content-type", "text/event-stream")
    
    let stream = EventStore.subscribe("post_events")
    
    for event in stream {
      chunk(conn, "event: {event.type}\ndata: {to_json(event.data)}\n\n")
    }
  }
  
  // Private helper functions
  fn authenticate_user(conn) {
    match get_req_header(conn, "authorization") {
      ["Bearer " <> token] => {
        match Auth.verify_token(token) {
          Ok(user) => assign(conn, :current_user, user),
          Error(_) => halt(json(conn, 401, %{error: "Unauthorized"}))
        }
      }
      _ => halt(json(conn, 401, %{error: "Missing authorization header"}))
    }
  }
  
  fn load_post(conn, %{id: id}) {
    match perform Database.get(Post, id, preload: [:author, :comments]) {
      Some(post) => assign(conn, :post, post),
      None => halt(json(conn, 404, %{error: "Post not found"}))
    }
  }
}
```

#### 3.2.3 Conn (Connection) API

```nulang
// Connection structure and API
type Conn = {
  // Request
  method: HTTPMethod,
  path: String,
  query_params: Map<String, String>,
  body_params: Map<String, JSON>,
  path_params: Map<String, String>,
  headers: Map<String, [String]>,
  remote_ip: String,
  host: String,
  port: Int,
  scheme: "http" | "https",
  
  // Response
  status: Option<Int>,
  resp_headers: Map<String, String>,
  resp_body: Option<Body>,
  
  // State
  assigns: Map<Atom, Any>,
  halted: Bool,
  private: Map<Atom, Any>
}

// Conn manipulation functions
fn put_status(conn, status) -> Conn
fn put_resp_header(conn, key, value) -> Conn
fn put_resp_cookie(conn, name, value, opts) -> Conn
fn assign(conn, key, value) -> Conn
fn get_req_header(conn, key) -> Option<String>
fn halt(conn) -> Conn
fn send_resp(conn, status, body) -> Conn
fn json(conn, status, data) -> Conn
fn html(conn, status, template) -> Conn
fn redirect(conn, location, status: 302) -> Conn
fn send_file(conn, status, path) -> Conn
fn send_chunked(conn, status) -> ChunkedConn
fn chunk(conn, data) -> Result<(), Error>
```

### 3.3 Channel API

#### 3.3.1 Basic Channel

```nulang
channel RoomChannel {
  // Topic pattern: "room:123"
  
  def join("room:" <> room_id, payload, socket) {
    // Verify the user can join this room
    let user = socket.assigns.current_user
    
    if perform Chat.can_access?(user, room_id) {
      // Subscribe to the room topic
      let socket = subscribe(socket, "room:{room_id}")
      
      // Track presence
      Presence.track(socket, user.id, %{
        online_at: System.now(),
        username: user.name
      })
      
      // Send current room state
      let messages = perform Chat.get_messages(room_id, limit: 50)
      let members = Presence.list(socket, "room:{room_id}")
      
      {:ok, assign(socket, :room_id, room_id)
            |> push("messages", messages)
            |> push("presence_state", members)}
    } else {
      {:error, %{reason: "unauthorized"}}
    }
  }
  
  def handle_in("new_msg", %{"body" => body}, socket) {
    let user = socket.assigns.current_user
    let room_id = socket.assigns.room_id
    
    let message = %{
      id: UUID.generate(),
      user_id: user.id,
      username: user.name,
      body: body,
      inserted_at: System.now()
    }
    
    // Persist
    perform Chat.save_message(room_id, message)
    
    // Broadcast to all room subscribers (including sender)
    broadcast(socket, "new_msg", message)
    
    {:noreply, socket}
  }
  
  def handle_in("edit_msg", %{"id" => id, "body" => body}, socket) {
    let user = socket.assigns.current_user
    
    match perform Chat.get_message(id) {
      Some(msg) if msg.user_id == user.id => {
        perform Chat.update_message(id, body)
        broadcast(socket, "msg_updated", %{id: id, body: body})
      }
      Some(_) => push(socket, "error", %{message: "Not your message"})
      None => push(socket, "error", %{message: "Message not found"})
    }
    
    {:noreply, socket}
  }
  
  def handle_in("typing", %{"typing" => is_typing}, socket) {
    broadcast_from(socket, "typing", %{
      user_id: socket.assigns.current_user.id,
      username: socket.assigns.current_user.name,
      typing: is_typing
    })
    
    {:noreply, socket}
  }
  
  // Handle system messages
  def handle_info(%{event: "presence_diff"} = msg, socket) {
    push(socket, "presence_diff", msg.payload)
    {:noreply, socket}
  }
  
  def handle_info(: periodic_cleanup, socket) {
    // Internal timer events
    perform Chat.cleanup_old_messages(socket.assigns.room_id)
    {:noreply, socket}
  }
  
  // Called when socket disconnects
  def terminate(reason, socket) {
    Logger.info("User {socket.assigns.current_user.id} left room {socket.assigns.room_id}")
  }
}
```

#### 3.3.2 Presence System

```nulang
// Presence tracking across channels
defmodule MyApp.Presence do
  use Phoenix.Presence,
    otp_app: :my_app,
    pubsub_server: MyApp.PubSub
end

// Usage in channel
channel MyChannel {
  def join(topic, payload, socket) {
    // Track this user's presence
    Presence.track(socket, socket.assigns.user_id, %{
      online_at: System.now(),
      username: payload["username"],
      device: payload["device"]
    })
    
    // Get all present users
    let present = Presence.list(socket)
    push(socket, "presence_state", present)
    
    {:ok, socket}
  }
}

// Client receives:
// presence_state: { "user1": [{metas: [{online_at: "...", username: "..."}]}] }
// presence_diff: { joins: {...}, leaves: {...} }
```

### 3.4 Template System

#### 3.4.1 Template Syntax

```nulang
// templates/layout.html.nula
@template AppLayout {
  <!DOCTYPE html>
  <html lang="en">
  <head>
    <meta charset="UTF-8">
    <title>{@page_title} | MyApp</title>
    <link rel="stylesheet" href="/assets/app.css">
    <script src="/assets/app.js" defer></script>
    {@csrf_meta_tag}
  </head>
  <body>
    <nav class="navbar">
      <a href="/">Home</a>
      <a href="/about">About</a>
      
      @if @current_user {
        <span>Welcome, {@current_user.name}</span>
        <a href="/logout">Logout</a>
      } else {
        <a href="/login">Login</a>
      }
    </nav>
    
    <main class="container">
      {@inner_content}
    </main>
    
    <footer>
      <p>&copy; 2024 MyApp</p>
    </footer>
  </body>
  </html>
}

// templates/users/index.html.nula
@template UsersIndex, layout: AppLayout {
  <div class="users-page">
    <h1>Users ({@users |> length()})</h1>
    
    <a href="/users/new" class="btn btn-primary">New User</a>
    
    <div class="users-grid">
      @for user <- @users {
        <div class="user-card">
          <img src={user.avatar_url || "/assets/default-avatar.png"} 
               alt={user.name} 
               class="avatar" />
          <h3>{user.name}</h3>
          <p class="email">{user.email}</p>
          <span class={"badge badge-" <> user.role}>{user.role}</span>
          
          @match user.status {
            :active => <span class="status active">Active</span>,
            :inactive => <span class="status inactive">Inactive</span>,
            :suspended => <span class="status suspended">Suspended</span>
          }
          
          <div class="actions">
            <a href="/users/{user.id}" class="btn btn-sm">View</a>
            <a href="/users/{user.id}/edit" class="btn btn-sm">Edit</a>
            
            @form method: "delete", action: "/users/{user.id}", 
                  confirm: "Are you sure?" {
              <button type="submit" class="btn btn-danger btn-sm">Delete</button>
            }
          </div>
        </div>
      }
    </div>
    
    @if @users |> length() == 0 {
      <div class="empty-state">
        <p>No users found.</p>
        <a href="/users/new" class="btn btn-primary">Create your first user</a>
      </div>
    }
    
    // Reusable component
    <Pagination 
      page={@page} 
      total_pages={@total_pages}
      base_url="/users"
      query_params={@filters}
    />
  </div>
}

// templates/users/show.html.nula
@template UsersShow, layout: AppLayout {
  <div class="user-profile">
    <img src={@user.avatar_url} alt={@user.name} class="avatar-large" />
    <h1>{@user.name}</h1>
    <p>{@user.bio || "No bio provided."}</p>
    
    <div class="stats">
      <stat value={@user.posts_count} label="Posts" />
      <stat value={@user.followers_count} label="Followers" />
      <stat value={@user.following_count} label="Following" />
    </div>
    
    @component RecentActivity, user: @user, limit: 10
  </div>
}
```

#### 3.4.2 Components

```nulang
// components/pagination.nula
component Pagination {
  props: {
    page: Int,
    total_pages: Int,
    base_url: String,
    query_params: Map<String, String>
  }
  
  def render(assigns) {
    <nav class="pagination" aria-label="Pagination">
      @if @page > 1 {
        <a href={page_url(@page - 1)} class="prev">&larr; Previous</a>
      }
      
      @for p <- page_range(@page, @total_pages) {
        @match p {
          :ellipsis => <span class="ellipsis">...</span>,
          num if num == @page => <span class="current">{num}</span>,
          num => <a href={page_url(num)}>{num}</a>
        }
      }
      
      @if @page < @total_pages {
        <a href={page_url(@page + 1)} class="next">Next &rarr;</a>
      }
    </nav>
  }
  
  fn page_url(page) {
    let params = @query_params |> Map.put("page", to_string(page))
    "{@base_url}?{URI.encode_query(params)}"
  }
  
  fn page_range(current, total) -> [Int | :ellipsis] {
    // Smart pagination: show first, last, current, and neighbors
    // e.g., [1, :ellipsis, 4, 5, 6, :ellipsis, 10]
    // ...
  }
}

// components/modal.nula
component Modal {
  props: {
    id: String,
    title: String,
    open: Bool = false
  }
  
  slot body
  slot footer
  
  def render(assigns) {
    <div class={"modal " <> if @open { "open" } else { "" }} id={@id}>
      <div class="modal-backdrop"></div>
      <div class="modal-content">
        <div class="modal-header">
          <h3>{@title}</h3>
          <button class="close" data-dismiss="modal">&times;</button>
        </div>
        <div class="modal-body">
          {@body}
        </div>
        @if @footer {
          <div class="modal-footer">
            {@footer}
          </div>
        }
      </div>
    </div>
  }
}

// Usage of modal component
@template UsersDelete, layout: AppLayout {
  <Modal id="delete-confirm" title="Delete User?" open={true}>
    <@body>
      <p>Are you sure you want to delete <strong>{@user.name}</strong>?</p>
      <p>This action cannot be undone.</p>
    </@body>
    <@footer>
      <button class="btn" data-dismiss="modal">Cancel</button>
      @form method: "delete", action: "/users/{@user.id}" {
        <button type="submit" class="btn btn-danger">Delete</button>
      }
    </@footer>
  </Modal>
}
```

### 3.5 LiveView API

#### 3.5.1 LiveView Lifecycle

```nulang
liveview CounterLive {
  // Mount: Called when LiveView first connects
  def mount(_params, _session, socket) {
    // Initialize state
    {:ok, assign(socket, 
      count: 0,
      history: []
    )}
  }
  
  // Handle params: Called when URL parameters change
  def handle_params(params, _uri, socket) {
    let initial = params["initial"] |> int_or(0)
    
    {:noreply, assign(socket, count: initial)}
  }
  
  // Handle events: Called in response to client interactions
  def handle_event("increment", %{"amount" => amount}, socket) {
    let new_count = socket.assigns.count + amount
    
    {:noreply, socket
      |> assign(count: new_count)
      |> update(:history, |h| h ++ [{"increment", amount}])
    }
  }
  
  def handle_event("decrement", _params, socket) {
    {:noreply, update(socket, :count, |c| c - 1)}
  }
  
  def handle_event("reset", _params, socket) {
    {:noreply, assign(socket, count: 0, history: [])}
  }
  
  // Handle async results
  def handle_event("fetch_data", _params, socket) {
    // Start async task
    Task.async(fn => perform ExpensiveService.fetch_data())
    
    {:noreply, assign(socket, loading: true)}
  }
  
  def handle_info({:async_result, {:ok, data}}, socket) {
    {:noreply, assign(socket, data: data, loading: false)}
  }
  
  def handle_info({:async_result, {:error, reason}}, socket) {
    {:noreply, assign(socket, error: reason, loading: false)}
  }
  
  // Handle periodic updates
  def handle_info(:tick, socket) {
    schedule_interval(self(), :tick, 1s)
    {:noreply, assign(socket, time: System.now())}
  }
  
  // Handle broadcasts
  def handle_info(%Broadcast{event: "counter_update", payload: payload}, socket) {
    {:noreply, assign(socket, global_count: payload.count)}
  }
  
  // Render: Server-side HTML template
  def render(assigns) {
    <div class="counter">
      <h1>Count: {@count}</h1>
      
      @if @loading {
        <div class="spinner">Loading...</div>
      }
      
      @if @error {
        <div class="error">{@error}</div>
      }
      
      <div class="controls">
        <button phx-click="decrement">-1</button>
        <button phx-click="increment" phx-value-amount={5}>+5</button>
        <button phx-click="increment" phx-value-amount={10}>+10</button>
        <button phx-click="reset">Reset</button>
      </div>
      
      <div class="history">
        <h3>History</h3>
        <ul>
          @for {action, value} <- @history {
            <li>{action}: {value}</li>
          }
        </ul>
      </div>
      
      <p class="timestamp">Server time: {@time}</p>
    </div>
  }
}
```

#### 3.5.2 Forms in LiveView

```nulang
liveview UserFormLive {
  def mount(_params, _session, socket) {
    {:ok, assign(socket,
      form: UserForm.new(),
      changeset: User.changeset(%User{}),
      submitted: false
    )}
  }
  
  def handle_event("validate", %{"user" => params}, socket) {
    let changeset = User.changeset(%User{}, params)
      |> Map.put(:action, :validate)
    
    {:noreply, assign(socket, changeset: changeset)}
  }
  
  def handle_event("save", %{"user" => params}, socket) {
    match User.create(params) {
      Ok(user) => {
        {:noreply, socket
          |> assign(submitted: true, user: user)
          |> push_event("confetti", %{ })
        }
      }
      Error(changeset) => {
        {:noreply, assign(socket, changeset: changeset)}
      }
    }
  }
  
  def render(assigns) {
    <div class="user-form">
      @if @submitted {
        <div class="success">
          <h2>User created!</h2>
          <p>Name: {@user.name}</p>
          <p>Email: {@user.email}</p>
        </div>
      } else {
        <.form 
          for={@changeset} 
          phx-change="validate" 
          phx-submit="save"
        >
          <div class="field">
            <label for="user_name">Name</label>
            <input type="text" id="user_name" name="user[name]" 
                   value={@changeset.params["name"]} />
            @for error <- @changeset.errors[:name] || [] {
              <span class="error">{error}</span>
            }
          </div>
          
          <div class="field">
            <label for="user_email">Email</label>
            <input type="email" id="user_email" name="user[email]"
                   value={@changeset.params["email"]} />
            @for error <- @changeset.errors[:email] || [] {
              <span class="error">{error}</span>
            }
          </div>
          
          <div class="field">
            <label for="user_role">Role</label>
            <select id="user_role" name="user[role]">
              <option value="user" selected={@changeset.params["role"] == "user"}>User</option>
              <option value="admin" selected={@changeset.params["role"] == "admin"}>Admin</option>
              <option value="moderator" selected={@changeset.params["role"] == "moderator"}>Moderator</option>
            </select>
          </div>
          
          <button type="submit" class="btn btn-primary">
            @if @changeset.valid? { "Create User" } else { "Fix Errors" }
          </button>
        </.form>
      }
    </div>
  }
}
```

### 3.6 Middleware System

#### 3.6.1 Built-in Plugs

```nulang
// Request ID generation
plug RequestIdPlug, header: "x-request-id"

// Request logging
plug RequestLoggerPlug,
  format: :combined,  // :combined | :common | :short | :dev
  filter: ["password", "token", "secret"],
  level: :info

// CORS
plug CorsPlug,
  origins: ["https://app.example.com", ~r/^https:\/\/.*\.example\.com$/],
  methods: ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
  headers: ["authorization", "content-type", "x-requested-with"],
  credentials: true,
  max_age: 86400

// Security headers
plug SecurityHeadersPlug,
  hsts: "max-age=31536000; includeSubDomains",
  x_frame_options: "DENY",
  x_content_type_options: "nosniff",
  x_xss_protection: "1; mode=block",
  referrer_policy: "strict-origin-when-cross-origin",
  csp: "default-src 'self'; script-src 'self' 'unsafe-inline'"

// Rate limiting
plug RateLimitPlug,
  max_requests: 1000,
  window: 1m,
  key: |conn| conn.remote_ip,
  storage: :memory,  // :memory | Redis
  on_limit: |conn| json(conn, 429, %{error: "Rate limit exceeded"})

// Body parsing
plug BodyParserPlug,
  parsers: [
    JSONParser [max_body: 10_000_000],
    URLencodedParser [max_body: 1_000_000],
    MultipartParser [max_parts: 100, max_file_size: 50_000_000]
  ]

// Session management
plug SessionPlug,
  store: CookieSessionStore,
  key: "_myapp_session",
  signing_salt: env("SESSION_SALT"),
  encryption_salt: env("SESSION_ENCRYPT_SALT"),
  max_age: 86400 * 30  // 30 days

// Authentication
plug AuthenticationPlug,
  strategy: JWT {
    secret: env("JWT_SECRET"),
    algorithm: "HS256",
    token_location: :header,  // :header | :cookie | :query
    token_key: "authorization",
    prefix: "Bearer "
  }

// Authorization
plug AuthorizationPlug,
  policies: [
    {"/admin/*", role: "admin"},
    {"/api/users", methods: ["DELETE"], role: "admin"},
    {"/api/posts/:id", methods: ["PUT", "DELETE"], owner_check: true}
  ]
```

#### 3.6.2 Custom Plug

```nulang
// Define a custom plug
plug RequestTimingPlug do
  def init(opts) do
    opts |> Map.put(:header_name, opts[:header_name] || "x-response-time")
  end
  
  def call(conn, opts) do
    let start = System.monotonic_time()
    
    // Register callback for after response
    conn
    |> register_before_send(fn conn ->
      let elapsed = System.monotonic_time() - start
      let ms = System.convert_time_unit(elapsed, :native, :millisecond)
      
      conn
      |> put_resp_header(opts.header_name, "{ms}ms")
    end)
  end
end

// Define a custom authentication plug
plug ApiKeyAuthenticationPlug do
  def init(opts) do
    opts
  end
  
  def call(conn, opts) {
    match get_req_header(conn, "x-api-key") {
      [api_key] => {
        match perform ApiKeyStore.verify(api_key) {
          Ok(account) => assign(conn, :current_account, account),
          Error(_) => halt(json(conn, 401, %{error: "Invalid API key"}))
        }
      }
      _ => halt(json(conn, 401, %{error: "API key required"}))
    }
  end
end
```

### 3.7 Database Integration

#### 3.7.1 Repository Pattern

```nulang
defmodule MyApp.Repo do
  use Ecto.Repo,
    otp_app: :my_app,
    adapter: Ecto.Adapters.Postgres
end

// Schema definition
schema User {
  field :id, UUID, primary_key: true
  field :name, String, required: true
  field :email, String, required: true, unique: true
  field :role, Enum[:user, :admin, :moderator], default: :user
  field :active, Bool, default: true
  field :metadata, JSON, default: %{}
  
  timestamps()  // Adds inserted_at and updated_at
  
  // Relationships
  has_many :posts, Post
  has_many :comments, Comment
  belongs_to :team, Team
  
  // Validations
  validate :name, presence: true, length: { min: 2, max: 100 }
  validate :email, presence: true, format: ~r/^[^\s@]+@[^\s@]+\.[^\s@]+$/
  
  // Changeset for creation
  def create_changeset(user, attrs) {
    user
    |> cast(attrs, [:name, :email, :role, :team_id])
    |> validate_required([:name, :email])
    |> unique_constraint(:email)
    |> foreign_key_constraint(:team_id)
  }
  
  // Changeset for updates
  def update_changeset(user, attrs) {
    user
    |> cast(attrs, [:name, :email, :role, :active])
    |> validate_required([:name])
    |> unique_constraint(:email)
  }
}

// Usage in controller
controller UsersController {
  def index(conn) {
    let users = Repo.all(User, where: [active: true], preload: [:team])
    json(conn, 200, users)
  }
  
  def show(conn, %{id: id}) {
    match Repo.get(User, id, preload: [:posts, :comments]) {
      Some(user) => json(conn, 200, user),
      None => json(conn, 404, %{error: "Not found"})
    }
  }
  
  def create(conn) {
    let changeset = User.create_changeset(%User{}, conn.body_params)
    
    match Repo.insert(changeset) {
      Ok(user) => json(conn, 201, user),
      Error(changeset) => json(conn, 422, %{errors: changeset.errors})
    }
  }
  
  def update(conn, %{id: id}) {
    let user = Repo.get!(User, id)
    let changeset = User.update_changeset(user, conn.body_params)
    
    match Repo.update(changeset) {
      Ok(updated) => json(conn, 200, updated),
      Error(changeset) => json(conn, 422, %{errors: changeset.errors})
    }
  }
  
  def delete(conn, %{id: id}) {
    let user = Repo.get!(User, id)
    Repo.delete!(user)
    send_resp(conn, 204, "")
  }
}
```

#### 3.7.2 Query DSL

```nulang
// Query examples
let active_users = Repo.all(
  from u in User,
  where: u.active == true,
  order_by: [desc: u.inserted_at],
  limit: 50,
  preload: [:team]
)

// Complex query
let recent_posts = Repo.all(
  from p in Post,
  join: u in User, on: p.user_id == u.id,
  where: p.published == true and p.inserted_at > ago(7, :days),
  where: u.role == "admin",
  select: %{title: p.title, author: u.name, published: p.inserted_at},
  order_by: [desc: p.inserted_at],
  limit: 20
)

// Aggregation
let stats = Repo.one(
  from u in User,
  select: %{
    total: count(),
    active: count() |> where(u.active == true),
    avg_age: avg(u.age)
  }
)

// Transactions
Repo.transaction(fn ->
  let user = Repo.insert!(User.create_changeset(%User{}, params))
  let profile = Repo.insert!(Profile.create_changeset(%Profile{}, 
    Map.put(profile_params, :user_id, user.id)
  ))
  {user, profile}
end)
```

---

## 4. Module Reference

### 4.1 Module Hierarchy

```
phoenix-nl/
├── core/
│   ├── endpoint.nula       # Endpoint definition and HTTP server
│   ├── router.nula         # Route matching and dispatch
│   ├── controller.nula     # Controller base and helpers
│   ├── conn.nula           # Connection struct and API
│   └── types.nula          # Core type definitions
├── channels/
│   ├── channel.nula        # Channel behavior and callbacks
│   ├── socket.nula         # Socket connection management
│   ├── pubsub.nula         # Pub/Sub message bus
│   └── presence.nula       # Presence tracking
├── live/
│   ├── live_view.nula      # LiveView behavior
│   ├── diff_engine.nula    # HTML diff calculation
│   ├── js_commands.nula    # Client-side JS commands
│   └── uploads.nula        # Live upload handling
├── templates/
│   ├── engine.nula         # Template compilation
│   ├── syntax.nula         # Template syntax parser
│   ├── components.nula     # Component system
│   └── helpers.nula        # Template helpers
├── plugs/
│   ├── cors.nula           # CORS handling
│   ├── auth.nula           # Authentication
│   ├── rate_limit.nula     # Rate limiting
│   ├── body_parser.nula    # Request body parsing
│   ├── session.nula        # Session management
│   ├── security.nula       # Security headers
│   └── logger.nula         # Request logging
├── db/
│   ├── repo.nula           # Repository pattern
│   ├── schema.nula         # Schema definitions
│   ├── query.nula          # Query DSL
│   └── migration.nula      # Database migrations
├── static/
│   ├── file_server.nula    # Static file serving
│   ├── asset_pipeline.nula # Asset compilation
│   └── cache.nula          # Static asset caching
├── pubsub/
│   ├── adapter.nula        # Pub/Sub adapter behavior
│   ├── pg2.nula            # PG2-based adapter
│   └── redis.nula          # Redis adapter
└── observability/
    ├── tracing.nula        # Request tracing
    ├── metrics.nula        # Metrics collection
    └── logging.nula        # Structured logging
```

### 4.2 Core Types

```nulang
// HTTP types
type Conn = {
  adapter: { adapter: Atom, opts: Any },
  assigns: Map<Atom, Any>,
  before_send: [Conn -> Conn],
  body_params: Map<String, Any>,
  cookies: Map<String, String>,
  halted: Bool,
  host: String,
  method: HTTPMethod,
  owner: PID,
  params: Map<String, Any>,
  path_info: [String],
  path_params: Map<String, Any>,
  port: Int,
  private: Map<Atom, Any>,
  query_params: Map<String, Any>,
  query_string: String,
  remote_ip: {a: Int, b: Int, c: Int, d: Int},
  req_cookies: Map<String, String>,
  req_headers: [{String, String}],
  resp_body: Option<Body>,
  resp_cookies: Map<String, CookieOpts>,
  resp_headers: Map<String, String>,
  scheme: :http | :https,
  script_name: [String],
  secret_key_base: String,
  state: :unset | :set | :file | :chunked | :sent,
  status: Option<Int>
}

enum HTTPMethod {
  GET, POST, PUT, DELETE, PATCH, OPTIONS, HEAD
}

enum Body {
  Binary(String),
  IOData(Any),
  File(String),
  Chunked
}

// Socket types
type Socket = {
  endpoint: Atom,
  transport: Atom,
  assigns: Map<Atom, Any>,
  channel: Atom,
  handler: Atom,
  pubsub_server: Atom,
  join_ref: String,
  ref: String,
  topic: String,
  serializer: Atom,
  transport_pid: PID
}

// Channel messages
type ChannelMessage = {
  join_ref: String,
  ref: String,
  topic: String,
  event: String,
  payload: Map<String, Any>
}

// Presence types
type Presence = {
  metas: [MetaEntry]
}

type MetaEntry = {
  phx_ref: String,
  online_at: String,
  // User-defined fields
  ...
}
```

### 4.3 Effect Definitions

```nulang
// Core web effects
effect WebServer {
  fn listen(port: Int, handler: Conn -> Conn) -> ServerRef;
  fn stop(server: ServerRef) -> ();
}

effect PubSub {
  fn subscribe(topic: String) -> ();
  fn unsubscribe(topic: String) -> ();
  fn broadcast(topic: String, event: String, payload: Any) -> ();
  fn broadcast_from(topic: String, event: String, payload: Any) -> ();
}

effect PresenceTrack {
  fn track(key: String, meta: Map<String, Any>) -> ();
  fn untrack(key: String) -> ();
  fn list(topic: String) -> Map<String, Presence>;
}
```

---

## 5. Implementation Phases

### 5.1 Phase 1: Core HTTP Server (Weeks 1-4)

**Goal:** Build the foundational HTTP server with routing and request handling.

```
Milestone: v0.1.0 — "Serve"
+---------------------------------------------------------------+
| Week 1-2            | Week 3-4                                |
+---------------------+-----------------------------------------+
| HTTP parser         | Router                                  |
|                     |                                         |
| - Request parsing   | - Route matching                        |
| - Response encoding | - Pattern-based dispatch                |
| - HTTP/1.1          | - Path parameters                       |
| - Keep-alive        | - Route guards                          |
|                     | - Resource routes                       |
| Connection mgmt     |                                         |
| - Connection pool   | Controller dispatch                     |
| - Request/response  | - Action resolution                     |
|   lifecycle         | - Conn pipeline                         |
|                     | - Response rendering                    |
+---------------------+-----------------------------------------+
| Deliverable: HTTP server with routing and controllers         |
| Tests: HTTP parsing, route matching, controller dispatch      |
+---------------------------------------------------------------+
```

### 5.2 Phase 2: Middleware & Sessions (Weeks 5-8)

**Goal:** Build the plug middleware system and session management.

```
Milestone: v0.2.0 — "Secure"
+---------------------------------------------------------------+
| Week 5-6            | Week 7-8                                |
+---------------------+-----------------------------------------+
| Plug system         | Session & security                      |
|                     |                                         |
| - Plug behavior     | - Session management                    |
| - Pipeline exec     | - Cookie encryption                     |
| - Halt/resume       | - CSRF protection                       |
| - Before_send       | - Security headers                      |
|                     |                                         |
| Built-in plugs      | Authentication                          |
| - Logger            | - JWT strategy                          |
| - CORS              | - Session strategy                      |
| - Body parser       | - API key strategy                      |
| - Static files      | - OAuth2 integration                    |
+---------------------+-----------------------------------------+
```

### 5.3 Phase 3: Templates & Views (Weeks 9-12)

**Goal:** Build the template engine and view layer.

```
Milestone: v0.3.0 — "Render"
+---------------------------------------------------------------+
| Week 9-10           | Week 11-12                              |
+---------------------+-----------------------------------------+
| Template engine     | Component system                        |
|                     |                                         |
| - Template parser   | - Component definition                  |
| - HTML generation   | - Props/slots                           |
| - Expression eval   | - Nested components                     |
| - Control flow      | - Component isolation                   |
|   (@if, @for)       |                                         |
|                     | View layer                              |
| Layout system       | - JSON rendering                        |
| - Layout nesting    | - View helpers                          |
| - Content injection | - Format negotiation                    |
+---------------------+-----------------------------------------+
```

### 5.4 Phase 4: WebSockets & Channels (Weeks 13-16)

**Goal:** Real-time communication via WebSockets and channels.

```
Milestone: v0.4.0 — "Connect"
+---------------------------------------------------------------+
| Week 13-14          | Week 15-16                              |
+---------------------+-----------------------------------------+
| WebSocket transport | Pub/Sub & Presence                      |
|                     |                                         |
| - WS handshake      | - Topic-based messaging                 |
| - Frame handling    | - Broadcast/multicast                   |
| - Heartbeat         | - Presence tracking                     |
| - Reconnection      | - CRDT-based presence                   |
|                     |                                         |
| Channel system      | Channel features                        |
| - Join/leave        | - Authorization                         |
| - Message routing   | - Message validation                    |
| - Callbacks         | - Backpressure                          |
+---------------------+-----------------------------------------+
```

### 5.5 Phase 5: LiveView (Weeks 17-22)

**Goal:** Server-rendered reactive UIs with LiveView.

```
Milestone: v0.5.0 — "Live"
+---------------------------------------------------------------+
| Week 17-18          | Week 19-20         | Week 21-22         |
+---------------------+--------------------+--------------------+
| LiveView runtime    | Diff engine        | Forms & uploads    |
|                     |                    |                    |
| - Mount lifecycle   | - HTML parsing     | - Form binding     |
| - Event handling    | - Diff calculation | - Validation       |
| - State management  | - Patch generation | - File uploads     |
| - JS commands       | - Morphdom compat  | - Progress tracking|
+---------------------+--------------------+--------------------+
```

### 5.6 Phase 6: Database & Production (Weeks 23-28)

**Goal:** Full database integration and production readiness.

```
Milestone: v1.0.0 — "Production"
+---------------------------------------------------------------+
| Week 23-24          | Week 25-26         | Week 27-28         |
+---------------------+--------------------+--------------------+
| Database layer      | Observability      | Performance        |
|                     |                    |                    |
| - Repository        | - Request tracing  | - Connection       |
| - Schema DSL        | - Metrics          |   pooling          |
| - Query builder     | - Health checks    | - HTTP/2 support   |
| - Migrations        | - Error tracking   | - Compression      |
| - Transactions      |                    | - Load testing     |
+---------------------+--------------------+--------------------+
```

---

## 6. Appendices

### 6.1 Comparison with Existing Frameworks

| Feature | Phoenix | ASP.NET Core | FastAPI | Nulang Web |
|---------|---------|-------------|---------|------------|
| Language | Elixir | C# | Python | Nulang |
| Concurrency | Actor model | async/await | async/await | Actor model |
| Latency | < 1ms | < 5ms | < 10ms | < 1ms |
| WebSockets | Native | SignalR | WebSockets | Native channels |
| Live UI | LiveView | Blazor | None | LiveView |
| Pattern matching | Yes | Partial | No | Yes |
| Type safety | Dynamic | Strong | Type hints | Strong |
| Effect system | No | No | No | Yes |
| Hot reload | Yes | Yes | Yes | Yes |

### 6.2 Error Code Reference

```nulang
enum WebFrameworkError {
  // Router errors
  RouteNotFound { path: String, method: HTTPMethod },
  InvalidRoutePattern { pattern: String, reason: String },
  DuplicateRoute { path: String },
  
  // Controller errors
  ControllerNotFound { name: String },
  ActionNotFound { controller: String, action: String },
  InvalidParams { key: String, expected: String, got: String },
  
  // Channel errors
  ChannelNotFound { topic: String },
  JoinError { topic: String, reason: String },
  UnauthorizedJoin { topic: String },
  
  // Template errors
  TemplateNotFound { name: String },
  TemplateSyntaxError { line: Int, message: String },
  ComponentNotFound { name: String },
  
  // LiveView errors
  LiveViewMountError { view: String, reason: String },
  InvalidEvent { event: String },
  StateSerializationError { reason: String }
}
```

### 6.3 Performance Targets

| Metric | Target | Notes |
|--------|--------|-------|
| Request latency (P99) | < 1ms | Excluding DB time |
| Concurrent connections | > 1M | WebSocket |
| Requests/second | > 100K | Per core |
| HTML diff calculation | < 5ms | Typical page |
| Template compilation | < 50ms | Per template |
| Memory per connection | < 1KB | WebSocket |
| Channel broadcast | < 1ms | To 10K subscribers |

### 6.4 Configuration Reference

```nulang
config :phoenix_nl, MyApp.Endpoint,
  http: [port: 4000],
  https: [
    port: 443,
    cipher_suite: :strong,
    certfile: "priv/cert.pem",
    keyfile: "priv/key.pem"
  ],
  debug_errors: true,
  code_reloader: true,
  check_origin: ["https://example.com"],
  secret_key_base: env("SECRET_KEY_BASE"),
  render_errors: [view: MyApp.ErrorView, accepts: ~w(html json)],
  pubsub_server: MyApp.PubSub,
  live_view: [signing_salt: env("LIVE_VIEW_SALT")]

config :phoenix_nl, MyApp.Repo,
  adapter: Ecto.Adapters.Postgres,
  username: "postgres",
  password: env("DB_PASSWORD"),
  database: "myapp_dev",
  hostname: "localhost",
  pool_size: 10
```

### 6.5 Glossary

| Term | Definition |
|------|------------|
| **Endpoint** | HTTP entry point with routing and middleware |
| **Controller** | Request handling with pattern matching |
| **Channel** | WebSocket real-time communication handler |
| **LiveView** | Server-rendered reactive UI component |
| **Plug** | Composable middleware unit |
| **Conn** | Connection struct representing request/response |
| **Topic** | Pub/Sub message routing key |
| **Presence** | Real-time user tracking system |
| **Saga** | Long-running transaction with compensation |
| **Pub/Sub** | Publish-subscribe messaging system |

---

*Document Version: 1.0.0*  
*Last Updated: 2024*  
*Status: Ready for Implementation*
