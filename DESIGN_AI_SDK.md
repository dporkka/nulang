# Nulang AI SDK Design Document

## Overview

The Nulang AI SDK (`nulang-ai`) is a first-class, native SDK for building intelligent applications in the Nulang programming language. It provides a unified, type-safe interface to large language models (LLMs), autonomous agent construction, multi-agent orchestration, and persistent memory subsystems. The SDK draws architectural inspiration from the OpenAI Python SDK, LangChain's composable primitives, and AutoGen's multi-agent conversation patterns, while leveraging Nulang's unique actor model, pattern matching, and effect system.

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

### 1.1 Agent

An **Agent** is the fundamental unit of intelligence in the Nulang AI SDK. It is an actor that encapsulates a language model configuration, a set of bound tools, a memory subsystem, and behavioral policies. Agents are declared with the `agent` keyword and are first-class citizens in the Nulang type system.

```nulang
agent Researcher = {
  model: "gpt-4o",
  tools: [search_web, read_pdf, summarize_text],
  memory: { type: episodic, max_tokens: 32000 },
  policy: SafeToolUse,
  description: "A research assistant that searches the web and reads documents"
}
```

Agents are:
- **Composable**: Can be combined into larger workflows
- **Observable**: Emit events for monitoring and debugging
- **Persistent**: Can checkpoint and resume state
- **Type-safe**: Tool inputs/outputs are validated at compile time

### 1.2 Tool Binding

**Tool Binding** is the mechanism by which Nulang functions are exposed to an agent as callable tools. The SDK automatically generates JSON Schema from Nulang type signatures, eliminating the need for manual schema authoring.

```nulang
@tool(description: "Search the web for information")
fn search_web(query: String, max_results: Int = 10) -> SearchResult {
  // Implementation
}
```

The `@tool` decorator:
1. Generates a JSON Schema from the function signature
2. Registers the function in the agent's tool registry
3. Handles serialization/deserialization automatically
4. Provides compile-time type safety for all parameters

### 1.3 Memory Subsystem

The SDK provides three memory types, each serving a distinct cognitive purpose:

| Memory Type | Purpose | Backend | Persistence |
|-------------|---------|---------|-------------|
| **Episodic** | Conversation history, recent interactions | In-memory ring buffer + SQLite | Per-session |
| **Semantic** | Vector search over long-term knowledge | Embedded vector DB (Qdrant-lite) | Persistent |
| **Procedural** | Learned patterns, few-shot examples | Key-value store | Persistent |

```nulang
memory MyMemory = {
  episodic: { max_turns: 50, summarize_after: 20 },
  semantic: { dimensions: 1536, collection: "knowledge" },
  procedural: { namespace: "learned_patterns" }
}
```

### 1.4 Multi-Agent Orchestration

Multiple agents can collaborate through structured communication patterns:

- **Supervisor Pattern**: A supervisor agent delegates to worker agents
- **Peer-to-Peer Pattern**: Agents communicate directly via message passing
- **Pipeline Pattern**: Output of one agent feeds into the next
- **Debate Pattern**: Multiple agents critique each other's outputs

### 1.5 Effect System Integration

The SDK leverages Nulang's effect system to model AI operations as effects, enabling:
- Cancellation of long-running LLM calls
- Timeouts on model inference
- Retry policies as effect handlers
- Testing via effect mocking

```nulang
effect AIRequest {
  fn call(model: String, messages: [Message]) -> ModelResponse;
}
```

---

## 2. Architecture Overview

### 2.1 System Architecture Diagram

```
+============================================================================+
|                          Nulang AI SDK Architecture                        |
+============================================================================+
|                                                                            |
|  +------------------+    +------------------+    +----------------------+ |
|  |   Application    |    |   Application    |    |    Application       | |
|  |   Layer          |    |   Layer          |    |    Layer             | |
|  |                  |    |                  |    |                      | |
|  |  Agent App       |    |  Multi-Agent     |    |  Embedded AI         | |
|  |  (single agent)  |    |  System          |    |  (RAG, classifier)   | |
|  +--------+---------+    +--------+---------+    +-----------+----------+ |
|           |                       |                          |            |
+-----------+-----------------------+--------------------------+------------+
|           |                       |                          |            |
|  +--------v---------+    +--------v---------+    +-----------v----------+ |
|  |   Agent DSL      |    |   Orchestrator   |    |   Memory Manager     | |
|  |   Module         |    |   Module         |    |   Module             | |
|  |                  |    |                  |    |                      | |
|  |  agent {}        |    |  supervisor()    |    |  episodic()          | |
|  |  tool {}         |    |  pipeline()      |    |  semantic()          | |
|  |  memory {}       |    |  debate()        |    |  procedural()        | |
|  +--------+---------+    +--------+---------+    +-----------+----------+ |
|           |                       |                          |            |
+-----------+-----------------------+--------------------------+------------+
|           |                       |                          |            |
|  +--------v---------+    +--------v---------+    +-----------v----------+ |
|  |   Core Engine    |    |   Core Engine    |    |   Core Engine        | |
|  |   (Agent)        |    |   (Workflow)     |    |   (Persistence)      | |
|  +--------+---------+    +--------+---------+    +-----------+----------+ |
|           |                       |                          |            |
+-----------+-----------------------+--------------------------+------------+
|           |                       |                          |            |
|  +--------v---------+    +--------v---------+    +-----------v----------+ |
|  |   LLM Client     |    |   Tool Registry  |    |   Vector Store       | |
|  |   Abstraction    |    |   & Schema Gen   |    |   (Embedded)         | |
|  |                  |    |                  |    |                      | |
|  |  - OpenAI        |    |  @tool macro     |    |  - HNSW indexing     | |
|  |  - Anthropic     |    |  JSON Schema gen |    |  - Async search      | |
|  |  - Local (GGUF)  |    |  Validation      |    |  - Persistence       | |
|  |  - Azure         |    |  Execution       |    |  - Embedding cache   | |
|  +--------+---------+    +--------+---------+    +-----------+----------+ |
|           |                       |                          |            |
+-----------+-----------------------+--------------------------+------------+
|           |                       |                          |            |
|  +--------v---------+    +--------v---------+    +-----------v----------+ |
|  |   Transport      |    |   Utilities      |    |   Configuration      | |
|  |   Layer          |    |   Layer          |    |   Layer              | |
|  |                  |    |                  |    |                      | |
|  |  HTTP/HTTPS      |    |  Token Counter   |    |  Provider configs    | |
|  |  WebSocket       |    |  Rate Limiter    |    |  API key management  | |
|  |  Streaming SSE   |    |  Retry Logic     |    |  Model registry      | |
|  |  Unix Socket     |    |  Circuit Breaker |    |  Environment vars    | |
|  +------------------+    +------------------+    +----------------------+ |
|                                                                            |
+============================================================================+
```

### 2.2 Data Flow Diagram

```
+----------+     +-------------+     +------------------+     +----------+
|  User    |     |   Agent     |     |  LLM Provider    |     |  Tool    |
|  Request |---->|  Runtime    |---->|  (OpenAI/etc.)   |<--->|  Funcs   |
|          |     |             |     |                  |     |          |
+----------+     +-------------+     +------------------+     +----------+
                      |   |                                             |
                      |   v                                             |
                      | +-------------+     +------------------+       |
                      | |   Memory    |     |   Schema         |       |
                      | |  Manager    |<--->|   Generator      |       |
                      | |             |     |                  |
                      | +-------------+     +------------------+
                      v
                +-------------+
                |  Event Bus  |---> Observers (logs, metrics, tracing)
                +-------------+
```

### 2.3 Component Interaction

```
+-------------------------------------------------------------------+
|                        Agent Lifecycle                             |
+-------------------------------------------------------------------+
|                                                                    |
|   DEFINE        COMPILE          INITIALIZE         RUN           |
|    ---->        ------->         ---------->        --->           |
|                                                                    |
|  agent{}       Type-check       Load model        Process         |
|    |           @tool schemas    credentials       request         |
|    |           Generate JSON    Init memory       Consult         |
|    |           schemas          Build tool        memory          |
|    |                           registry           Select          |
|    |                           Start event        tools           |
|    |                           bus                 Call LLM        |
|    |                                               Stream          |
|    |                                               response       |
|    |                                               Store in        |
|    |                                               memory          |
|    |                                               Return          |
|    |                                               result          |
|    v                                                               |
|  RELOAD  <--------  CHECKPOINT  <----------  HANDLE ERROR          |
|    |                   |                        |                  |
|    |                   |                        |                  |
|  Hot-swap            Save state               Retry policy         |
|  config              to disk                  Fallback model       |
|                      Resume later             Circuit breaker      |
|                                                                    |
+-------------------------------------------------------------------+
```

---

## 3. API Design & Specification

### 3.1 Agent Definition DSL

The agent definition uses a declarative syntax integrated into Nulang's type system.

#### 3.1.1 Basic Agent

```nulang
// Minimal agent definition
agent Greeter = {
  model: "gpt-4o-mini",
  system_prompt: "You are a friendly assistant."
}

// Usage
fn main() {
  let response = perform Greeter.ask("Hello! What's Nulang?")
  println(response.content)
}
```

#### 3.1.2 Agent with Tools

```nulang
@tool(description: "Calculate the sum of two numbers")
fn add(a: Float, b: Float) -> Float {
  a + b
}

@tool(description: "Search the web for current information")
fn web_search(query: String, max_results: Int = 5) -> [SearchResult] {
  perform HTTP.get("https://api.search.io/v1/search", params: {
    q: query,
    limit: max_results
  })
}

@tool(description: "Read a PDF document and extract text")
fn read_pdf(url: String, pages: Option<[Int]>) -> PdfContent {
  let response = perform HTTP.get(url)
  PdfParser.extract(response.body, pages: pages)
}

agent Calculator = {
  model: "gpt-4o",
  tools: [add, subtract, multiply, divide],
  system_prompt: "You are a precise calculator. Always use tools for math.",
  tool_policy: RequireToolUse // Force tool use instead of guessing
}

agent Researcher = {
  model: "gpt-4o",
  tools: [web_search, read_pdf, summarize],
  memory: {
    type: episodic,
    max_tokens: 16000,
    summarization: { trigger: 80%, model: "gpt-4o-mini" }
  },
  system_prompt: "You are a research assistant. Cite sources.",
  max_tool_calls: 10
}
```

#### 3.1.3 Agent Configuration Reference

```nulang
// Full configuration with all options
agent FullyConfigured = {
  // Model selection
  model: "gpt-4o",                    // Required: model identifier
  model_fallback: "gpt-4o-mini",      // Fallback if primary fails
  
  // Prompting
  system_prompt: "...",               // System message
  system_prompt_file: "prompts/agent.txt", // Load from file
  few_shot_examples: [                // In-context examples
    { user: "...", assistant: "..." },
    { user: "...", assistant: "...", tool_calls: [...] }
  ],
  
  // Tool configuration
  tools: [tool1, tool2],              // Bound tool functions
  tool_policy: Auto | RequireToolUse | DisableToolUse,
  max_tool_calls: 10,                 // Max tool invocations per request
  tool_timeout: 30s,                  // Per-tool timeout
  
  // Memory
  memory: {
    episodic: {
      max_turns: 50,                  // Max conversation turns
      max_tokens: 32000,              // Token budget for history
      summarization: {
        enabled: true,
        trigger: 80%,                 // Summarize at 80% capacity
        model: "gpt-4o-mini"          // Model for summarization
      }
    },
    semantic: {
      enabled: true,
      dimensions: 1536,               // Vector dimensions
      collection: "default",          // Collection name
      top_k: 5                        // Results to retrieve
    },
    procedural: {
      enabled: true,
      namespace: "patterns"
    }
  },
  
  // Response configuration
  response_format: Text | JSON { schema: User }, // Force JSON output
  temperature: 0.7,
  max_tokens: 4096,
  top_p: 1.0,
  
  // Safety and policies
  policy: SafeToolUse,                // Safety policy module
  rate_limit: { requests: 60, window: 1m },
  
  // Streaming
  stream: false,                      // Enable streaming responses
  
  // Observability
  tracing: true,                      // Enable OpenTelemetry tracing
  metadata: {                        // Custom metadata for logging
    team: "platform",
    version: "2.1"
  }
}
```

### 3.2 Conversation API

#### 3.2.1 Simple Request-Response

```nulang
// Single-turn interaction
let response = perform Researcher.ask(
  "What are CRDTs and how do they work?"
)

// Access response fields
println(response.content)           // The text content
println(response.tokens_used)       // { prompt: 150, completion: 300 }
println(response.model)             // "gpt-4o"
println(response.finish_reason)      // "stop" | "tool_calls" | "length"

// Multi-turn conversation (maintains context automatically)
let conversation = Researcher.start_conversation()
let msg1 = perform conversation.ask("What is a CRDT?")
let msg2 = perform conversation.ask("Can you give me an example?")
let msg3 = perform conversation.ask("How does this compare to OT?")

// Save conversation state
conversation.save("research_session.json")

// Resume later
let resumed = Researcher.load_conversation("research_session.json")
```

#### 3.2.2 Structured Output

```nulang
// Define the expected output shape
type ResearchSummary = {
  title: String,
  key_points: [String],
  sources: [{ url: String, title: String }],
  confidence: Float
}

// Request structured output
let result: ResearchSummary = perform Researcher.ask_structured(
  "Research CRDTs for me",
  schema: ResearchSummary
)

// Access typed fields
println(result.title)
println(result.key_points[0])
```

#### 3.2.3 Streaming Responses

```nulang
// Stream tokens as they arrive
let stream = perform Researcher.ask_stream(
  "Write a detailed explanation of vector clocks"
)

for chunk in stream {
  match chunk {
    Token(text) => print(text),           // Print incrementally
    ToolCall(call) => println("Using tool: {call.name}"),
    ToolResult(result) => println("Tool returned: {result}"),
    Done(final) => println("\n\nTotal tokens: {final.tokens_used}")
  }
}

// Async streaming with backpressure
async fn stream_with_backpressure() {
  let stream = perform Researcher.ask_stream("...")
  
  stream
    |> Stream.filter(|chunk| match chunk { Token(_) => true, _ => false })
    |> Stream.throttle(16ms)  // Rate limit for UI rendering
    |> Stream.for_each(|token| {
      ui.append_text(token.text)
    })
}
```

#### 3.2.4 Multi-Modal Input

```nulang
// Image input
let analysis = perform Researcher.ask(
  "Describe this diagram",
  images: [load_image("architecture.png")]
)

// Multiple images
let comparison = perform Researcher.ask(
  "Compare these two designs",
  images: [load_image("design_a.png"), load_image("design_b.png")]
)

// Audio input (for models that support it)
let transcript = perform Researcher.ask(
  "Transcribe and summarize",
  audio: [load_audio("meeting.mp3")]
)

// Mixed media
let response = perform Researcher.ask(
  "Based on this screenshot and error log, what's the bug?",
  images: [load_image("error_screenshot.png")],
  attachments: [{ type: "text/plain", content: error_log }]
)
```

### 3.3 Tool System

#### 3.3.1 Tool Definition

```nulang
// Basic tool with auto-generated schema
@tool(description: "Get current weather for a location")
fn get_weather(location: String, unit: TemperatureUnit = Celsius) -> Weather {
  perform WeatherAPI.current(location, unit)
}

// Tool with complex types (schema auto-generated)
type SearchFilter = {
  date_range: Option<{ from: Date, to: Date }>,
  domains: Option<[String]>,
  safe_search: Bool
}

@tool(description: "Search with advanced filters")
fn advanced_search(query: String, filter: SearchFilter) -> SearchResults {
  // Implementation
}

// Tool with validation
@tool(description: "Send email to a recipient")
@validate(recipient: email_format, body: min_length(10))
fn send_email(recipient: String, subject: String, body: String) -> Result<EmailId, SendError> {
  perform EmailService.send(recipient, subject, body)
}

// Async tool
@tool(description: "Run a long computation")
async fn long_computation(data: [Float], iterations: Int) -> ComputationResult {
  // Runs in a separate worker
  perform ComputeEngine.run(data, iterations)
}
```

#### 3.3.2 Tool Registry

```nulang
// Create a tool registry programmatically
let tools = ToolRegistry.new()
  |> ToolRegistry.register(add)
  |> ToolRegistry.register(search_web)
  |> ToolRegistry.register(read_pdf)

// Create agent from registry
let agent = Agent.new(
  model: "gpt-4o",
  tools: tools,
  system_prompt: "..."
)

// Inspect available tools
for tool in tools.list() {
  println("{tool.name}: {tool.description}")
  println("  Parameters: {tool.schema.parameters}")
}
```

#### 3.3.3 Dynamic Tool Selection

```nulang
// Agent decides which tools to use based on description
agent SmartAgent = {
  model: "gpt-4o",
  tools: [tool_a, tool_b, tool_c, tool_d, tool_e],
  tool_selection: Auto,  // Model picks relevant tools
  max_tool_calls: 5
}

// Or manually specify which tools for each call
let result = perform SmartAgent.ask(
  "Calculate the total",
  available_tools: [add, multiply]  // Only these tools visible
)
```

### 3.4 Memory System

#### 3.4.1 Episodic Memory

```nulang
// Episodic memory manages conversation history
let memory = EpisodicMemory.new(
  max_turns: 100,
  max_tokens: 64000,
  summarization: {
    enabled: true,
    trigger_threshold: 0.8,
    model: "gpt-4o-mini"
  }
)

// Add interactions
memory.add_turn(user: "What is Nulang?", assistant: "Nulang is a...")
memory.add_turn(user: "What are its features?", assistant: "Key features include...")

// Retrieve recent context
let recent = memory.get_recent(n: 10)

// Search within conversation
let relevant = memory.search("actor model")

// Get full history with summaries
let history = memory.get_history(strategy: SummarizeOld)

// Clear memory
memory.clear()

// Export for debugging
memory.export_to_file("conversation.json")
```

#### 3.4.2 Semantic Memory

```nulang
// Initialize semantic memory with embedding model
let semantic = SemanticMemory.new(
  embedding_model: "text-embedding-3-large",
  dimensions: 3072,
  collection: "knowledge_base"
)

// Store documents with auto-embedding
let doc_id = semantic.store(
  content: "Nulang uses an actor model for concurrency...",
  metadata: { source: "docs", topic: "concurrency" }
)

// Batch store
semantic.store_batch([
  { content: "...", metadata: {...} },
  { content: "...", metadata: {...} }
])

// Search
let results = semantic.search(
  query: "How does Nulang handle concurrency?",
  top_k: 5,
  filter: { topic: "concurrency" }
)

for result in results {
  println("Score: {result.score}, Content: {result.content}")
}

// Delete
semantic.delete(doc_id)
semantic.delete_where(filter: { topic: "deprecated" })
```

#### 3.4.3 Procedural Memory

```nulang
// Procedural memory stores learned patterns
let procedural = ProceduralMemory.new(namespace: "my_app")

// Store a learned pattern
procedural.store(
  key: "format_research_output",
  pattern: {
    input_pattern: "research_*",
    output_template: "{title}\n\n{summary}\n\nSources: {sources}"
  }
)

// Retrieve pattern
let formatter = procedural.get("format_research_output")

// Store few-shot examples
procedural.add_example(
  task: "code_review",
  example: {
    input: "fn bad() { let x = 1; x }",
    output: "Issue: Unused variable. Fix: Remove `x` or use it."
  }
)

// Get relevant examples for in-context learning
let examples = procedural.get_examples(
  task: "code_review",
  query: "unused variable",
  top_k: 3
)
```

#### 3.4.4 Composite Memory

```nulang
// Combine all memory types
let memory = CompositeMemory.new({
  episodic: EpisodicMemory.new(max_turns: 50),
  semantic: SemanticMemory.new(collection: "app_knowledge"),
  procedural: ProceduralMemory.new(namespace: "patterns")
})

// The composite memory coordinates between layers:
// 1. Recent context comes from episodic
// 2. Long-term knowledge comes from semantic
// 3. Learned patterns come from procedural

agent KnowledgeWorker = {
  model: "gpt-4o",
  memory: memory,
  tools: [search_web, read_document]
}
```

### 3.5 Multi-Agent Orchestration

#### 3.5.1 Supervisor Pattern

```nulang
// Define worker agents
agent Researcher = { model: "gpt-4o", tools: [search_web, read_pdf] }
agent Writer = { model: "gpt-4o", tools: [write_doc, edit_doc] }
agent Editor = { model: "gpt-4o", tools: [grammar_check, style_check] }

// Define supervisor
agent Supervisor = {
  model: "gpt-4o",
  system_prompt: "You coordinate research and writing tasks."
}

// Create supervised team
let team = SupervisorTeam.new(
  supervisor: Supervisor,
  workers: [
    { name: "researcher", agent: Researcher, description: "Finds information" },
    { name: "writer", agent: Writer, description: "Writes content" },
    { name: "editor", agent: Editor, description: "Reviews quality" }
  ],
  max_iterations: 10
)

// Execute task through supervisor
let article = perform team.run(
  "Write an article about CRDTs in distributed systems"
)
```

#### 3.5.2 Pipeline Pattern

```nulang
// Chain agents in a pipeline
let pipeline = Pipeline.new()
  |> Pipeline.stage("research", Researcher)
  |> Pipeline.stage("write", Writer)
  |> Pipeline.stage("edit", Editor)
  |> Pipeline.connect("research", "write", |output| {
    "Write an article based on this research: {output}"
  })
  |> Pipeline.connect("write", "edit", |output| {
    "Review this article: {output}"
  })

let result = perform pipeline.run(
  input: "Research topic: CRDTs"
)
```

#### 3.5.3 Peer-to-Peer Messaging

```nulang
// Create agent network
let network = AgentNetwork.new()

// Register agents
network.register("analyst", Analyst)
network.register("critic", Critic)
network.register("synthesizer", Synthesizer)

// Define message handlers
agent Analyst = {
  model: "gpt-4o",
  on_message: fn(msg, ctx) {
    let analysis = perform deep_analysis(msg.content)
    ctx.send("critic", analysis)      // Send to critic
    ctx.send("synthesizer", analysis)  // Send to synthesizer
  }
}

agent Critic = {
  model: "gpt-4o",
  on_message: fn(msg, ctx) {
    let critique = perform critique_analysis(msg.content)
    ctx.reply(critique)                // Reply to sender
  }
}

// Start network with initial message
let result = perform network.broadcast(
  "Analyze the pros and cons of serverless computing"
)
```

#### 3.5.4 Debate Pattern

```nulang
// Multi-agent debate for critical decisions
let debate = Debate.new(
  topic: "Should we use microservices or monolith?",
  participants: [
    { name: "advocate_microservices", stance: "pro", agent: ProAgent },
    { name: "advocate_monolith", stance: "pro", agent: ConAgent },
    { name: "moderator", agent: Moderator }
  ],
  rounds: 3,
  consensus_threshold: 0.8
)

let conclusion = perform debate.run()
// conclusion.consensus: Bool
// conclusion.arguments: [Argument]
// conclusion.recommendation: String
```

### 3.6 LLM Client Configuration

#### 3.6.1 Provider Configuration

```nulang
// OpenAI
let openai = LLMProvider.openai({
  api_key: env("OPENAI_API_KEY"),
  organization: env("OPENAI_ORG_ID"),
  base_url: "https://api.openai.com/v1",  // Optional: custom endpoint
  timeout: 60s,
  max_retries: 3
})

// Anthropic
let anthropic = LLMProvider.anthropic({
  api_key: env("ANTHROPIC_API_KEY"),
  base_url: "https://api.anthropic.com",
  timeout: 60s
})

// Local model via Ollama
let local = LLMProvider.ollama({
  base_url: "http://localhost:11434",
  model: "llama3.1:70b"
})

// Local model via llama.cpp
let gguf = LLMProvider.llama_cpp({
  model_path: "./models/mistral-7b-Q4_K_M.gguf",
  n_ctx: 32768,
  n_gpu_layers: 33
})

// Azure OpenAI
let azure = LLMProvider.azure({
  api_key: env("AZURE_OPENAI_KEY"),
  endpoint: "https://my-resource.openai.azure.com",
  deployment: "gpt-4o",
  api_version: "2024-06-01"
})

// Custom provider (any OpenAI-compatible API)
let custom = LLMProvider.custom({
  base_url: "https://api.mycorp.com/v1",
  api_key: env("MYCORP_API_KEY"),
  headers: { "X-Custom-Header": "value" }
})
```

#### 3.6.2 Model Registry

```nulang
// Register available models
let registry = ModelRegistry.new()
  |> ModelRegistry.register({
    id: "gpt-4o",
    provider: openai,
    context_window: 128000,
    supports_tools: true,
    supports_vision: true,
    supports_json: true,
    input_cost_per_1k: 0.005,
    output_cost_per_1k: 0.015
  })
  |> ModelRegistry.register({
    id: "claude-3-5-sonnet",
    provider: anthropic,
    context_window: 200000,
    supports_tools: true,
    supports_vision: true,
    input_cost_per_1k: 0.003,
    output_cost_per_1k: 0.015
  })
  |> ModelRegistry.register({
    id: "llama3.1-70b",
    provider: local,
    context_window: 128000,
    supports_tools: true,
    input_cost_per_1k: 0.0,
    output_cost_per_1k: 0.0
  })

// Select model by capability
let vision_model = registry.find(capabilities: [Vision, ToolUse])
let cheap_model = registry.find(max_cost_per_1k: 0.001)
```

### 3.7 Error Handling & Resilience

#### 3.7.1 Retry Policies

```nulang
// Configure retry behavior
let resilient_agent = Agent.new({
  model: "gpt-4o",
  retry_policy: {
    max_retries: 5,
    backoff: Exponential { base: 1s, max: 60s, multiplier: 2.0 },
    retry_on: [RateLimit, ServerError, Timeout],
    give_up_on: [AuthenticationError, InvalidRequest]
  },
  // Circuit breaker
  circuit_breaker: {
    failure_threshold: 5,
    recovery_timeout: 30s,
    half_open_max_calls: 3
  }
})
```

#### 3.7.2 Graceful Degradation

```nulang
// Fallback chain
let agent = Agent.with_fallbacks([
  { model: "gpt-4o", priority: 1 },
  { model: "gpt-4o-mini", priority: 2 },
  { model: "llama3.1-70b", provider: local, priority: 3 }
])

// Usage: tries gpt-4o first, falls back automatically
let result = perform agent.ask("Important question")
```

#### 3.7.3 Token Management

```nulang
// Token counting and budget management
let budget = TokenBudget.new(
  max_prompt_tokens: 100000,
  max_completion_tokens: 16000,
  max_total_tokens: 1000000  // Per-day limit
)

let agent = Agent.new({
  model: "gpt-4o",
  token_budget: budget,
  on_budget_exceeded: |usage| {
    Logger.warn("Token budget exceeded: {usage}")
    // Could switch to cheaper model, summarize, etc.
  }
})

// Check usage
println(agent.usage.total_tokens)
println(agent.usage.estimated_cost)
```

---

## 4. Module Reference

### 4.1 Module Hierarchy

```
nulang-ai/
├── core/
│   ├── agent.nula          # Agent definition DSL and runtime
│   ├── conversation.nula   # Conversation management
│   ├── message.nula        # Message types and serialization
│   └── types.nula          # Core type definitions
├── llm/
│   ├── client.nula         # LLM client abstraction
│   ├── providers/
│   │   ├── openai.nula     # OpenAI API client
│   │   ├── anthropic.nula  # Anthropic API client
│   │   ├── ollama.nula     # Ollama local client
│   │   ├── llama_cpp.nula  # llama.cpp bindings
│   │   └── azure.nula      # Azure OpenAI client
│   ├── streaming.nula      # Streaming response handling
│   └── tokenizer.nula      # Token counting utilities
├── tools/
│   ├── registry.nula       # Tool registration and discovery
│   ├── schema.nula         # JSON Schema generation
│   ├── execution.nula      # Tool execution engine
│   └── decorators.nula     # @tool macro and friends
├── memory/
│   ├── episodic.nula       # Conversation history
│   ├── semantic.nula       # Vector search memory
│   ├── procedural.nula     # Learned patterns
│   └── composite.nula      # Unified memory interface
├── orchestration/
│   ├── supervisor.nula     # Supervisor pattern
│   ├── pipeline.nula       # Pipeline pattern
│   ├── network.nula        # Peer-to-peer messaging
│   └── debate.nula         # Debate pattern
├── observability/
│   ├── tracing.nula        # OpenTelemetry integration
│   ├── metrics.nula        # Usage metrics
│   └── logging.nula        # Structured logging
└── utils/
    ├── retry.nula          # Retry logic
    ├── circuit_breaker.nula # Circuit breaker
    ├── rate_limiter.nula   # Rate limiting
    └── token_counter.nula  # Token counting
```

### 4.2 Core Types

```nulang
// Message types
enum Role {
  System,
  User,
  Assistant,
  Tool
}

type Message = {
  role: Role,
  content: String,
  tool_calls: Option<[ToolCall]>,
  tool_results: Option<[ToolResult]>,
  images: Option<[Image]>,
  metadata: Option<Map<String, JSON>>
}

type ToolCall = {
  id: String,
  name: String,
  arguments: Map<String, JSON>
}

type ToolResult = {
  call_id: String,
  content: JSON,
  error: Option<String>
}

type ModelResponse = {
  content: String,
  model: String,
  finish_reason: FinishReason,
  tokens_used: TokenUsage,
  tool_calls: Option<[ToolCall]>,
  latency_ms: Int
}

type TokenUsage = {
  prompt: Int,
  completion: Int,
  total: Int
}

enum FinishReason {
  Stop,
  ToolCalls,
  Length,
  ContentFilter,
  Error
}
```

### 4.3 Effect Definitions

```nulang
// Core AI effect
effect AIRequest {
  fn complete(config: RequestConfig) -> ModelResponse;
  fn stream(config: RequestConfig) -> Stream<TokenChunk>;
  fn embed(text: String) -> [Float];
}

// Tool execution effect
effect ToolExecution {
  fn execute(name: String, args: Map<String, JSON>) -> JSON;
  fn validate(name: String, args: Map<String, JSON>) -> Result<(), ValidationError>;
}

// Memory effect
effect MemoryAccess {
  fn recall(query: String, context: RecallContext) -> [MemoryEntry];
  fn store(entry: MemoryEntry) -> MemoryId;
  fn forget(id: MemoryId) -> Bool;
}
```

---

## 5. Implementation Phases

### 5.1 Phase 1: Core LLM Client (Weeks 1-4)

**Goal:** Establish the foundational LLM client abstraction with OpenAI support.

```
Milestone: v0.1.0 — "Speak"
+---------------------------------------------------------------+
| Week 1              | Week 2              | Week 3-4          |
+---------------------+---------------------+-------------------+
| HTTP client         | Message types       | Streaming         |
| (built-in + TLS)    | Serialization       | SSE parser        |
|                     |                     |                   |
| OpenAI API mapping  | Request/response    | Token counting    |
| (chat completions)  | types               | (tiktoken port)   |
|                     |                     |                   |
| Error types         | Provider trait      | Retry logic       |
| (API errors,        | definition          | (exponential      |
|  network errors)    |                     |  backoff)         |
+---------------------+---------------------+-------------------+
| Deliverable: Basic LLM client that can send/receive messages  |
| Tests: Unit tests for serialization, integration tests for API |
+---------------------------------------------------------------+
```

**Key Tasks:**
- [ ] Implement HTTP/HTTPS client with connection pooling
- [ ] Map OpenAI chat completions API to Nulang types
- [ ] Implement request/response serialization
- [ ] Build streaming SSE response parser
- [ ] Port tiktoken for token counting
- [ ] Define `LLMProvider` trait/protocol
- [ ] Implement exponential backoff retry
- [ ] Write comprehensive test suite

### 5.2 Phase 2: Tool Binding System (Weeks 5-8)

**Goal:** Enable agents to discover and call Nulang functions as tools.

```
Milestone: v0.2.0 — "Act"
+---------------------------------------------------------------+
| Week 5-6            | Week 7-8                                |
+---------------------+-----------------------------------------+
| @tool macro         | Tool execution engine                   |
| implementation      |                                         |
|                     | - Sync/async execution                  |
| JSON Schema         - Error handling                          |
| generation from     - Timeout handling                        |
| Nulang types        - Result serialization                    |
|                     |                                         |
| Tool registry       | Integration with LLM                    |
| - Registration      | client                                  |
| - Discovery         |                                         |
| - Validation        | - Parse tool_calls from response        |
|                     | - Execute and return results            |
+---------------------+-----------------------------------------+
```

**Key Tasks:**
- [ ] Implement `@tool` compile-time macro
- [ ] Build Nulang type -> JSON Schema converter
- [ ] Create tool registry with lookup
- [ ] Implement tool call parsing from LLM responses
- [ ] Build tool execution engine with error handling
- [ ] Handle async tool execution
- [ ] Integrate tool loop with LLM client
- [ ] Validate tool arguments at runtime

### 5.3 Phase 3: Agent DSL & Memory (Weeks 9-14)

**Goal:** Build the agent definition DSL and memory subsystems.

```
Milestone: v0.3.0 — "Remember"
+---------------------------------------------------------------+
| Week 9-10           | Week 11-12         | Week 13-14         |
+---------------------+--------------------+--------------------+
| Agent DSL parser    | Episodic memory    | Semantic memory    |
|                     |                    |                    |
| agent {} syntax     | Ring buffer        | Vector store       |
| Configuration       | Conversation       | embedding          |
| merging             | history            | (HNSW index)       |
|                     |                    |                    |
| Agent runtime       | Summarization      | Procedural memory  |
| - Lifecycle mgmt    | (when full)        | Pattern storage    |
| - Event emission    |                    | Few-shot retrieval |
+---------------------+--------------------++--------------------+
| Week 11: Composite memory that coordinates all three types    |
| Week 12-14: Integration testing and performance optimization  |
+---------------------+--------------------+--------------------+
```

**Key Tasks:**
- [ ] Parse `agent {}` syntax into runtime config
- [ ] Build agent lifecycle management (init, run, shutdown)
- [ ] Implement episodic memory with ring buffer
- [ ] Add automatic summarization for long conversations
- [ ] Integrate vector database (embedded Qdrant or similar)
- [ ] Implement semantic search with embeddings
- [ ] Build procedural memory for pattern storage
- [ ] Create composite memory coordinator
- [ ] Add memory persistence (SQLite backend)

### 5.4 Phase 4: Multi-Agent Orchestration (Weeks 15-20)

**Goal:** Enable multiple agents to collaborate in structured patterns.

```
Milestone: v0.4.0 — "Collaborate"
+---------------------------------------------------------------+
| Week 15-16          | Week 17-18         | Week 19-20         |
+---------------------+--------------------+--------------------+
| Supervisor pattern  | Pipeline pattern   | Network & Debate   |
|                     |                    |                    |
| Agent delegation    | Stage chaining     | P2P messaging      |
| Task routing        | Data transform     | Message routing    |
| Result aggregation  | Error propagation  | Broadcast/multicast|
|                     |                    |                    |
| Worker pool         | Saga compensation  | Debate rounds      |
| management          | for failures       | Consensus logic    |
+---------------------+--------------------+--------------------+
| Week 19-20: Integration tests, docs, examples                 |
+---------------------+--------------------+--------------------+
```

**Key Tasks:**
- [ ] Implement supervisor pattern with task delegation
- [ ] Build worker agent pool management
- [ ] Create pipeline stage chaining
- [ ] Implement data transformation between stages
- [ ] Add saga compensation for failures
- [ ] Build peer-to-peer message routing
- [ ] Implement broadcast and multicast
- [ ] Create debate pattern with consensus
- [ ] Write comprehensive integration tests

### 5.5 Phase 5: Observability & Polish (Weeks 21-24)

**Goal:** Production readiness with observability and documentation.

```
Milestone: v1.0.0 — "Production"
+---------------------------------------------------------------+
| Week 21-22          | Week 23-24                                |
+---------------------+-----------------------------------------+
| Observability       | Documentation & Examples                |
|                     |                                         |
| OpenTelemetry       | API reference docs                      |
| tracing             |                                         |
|                     | Tutorial: Building your first agent     |
| Metrics collection  | Example: Customer support bot           |
| (token usage,       | Example: Research assistant             |
|  latency)           | Example: Code review system             |
|                     |                                         |
| Structured logging  | Performance benchmarks                  |
|                     | Security audit                          |
+---------------------+-----------------------------------------+
```

---

## 6. Appendices

### 6.1 Comparison with Existing SDKs

| Feature | OpenAI SDK | LangChain | AutoGen | Nulang AI SDK |
|---------|-----------|-----------|---------|---------------|
| Native language | Python | Python | Python | Nulang |
| Type-safe tools | No | Partial | No | Yes (compile-time) |
| Actor model | No | No | No | Yes (built-in) |
| Effect system | No | No | No | Yes |
| Memory types | Manual | Chains | Yes | Episodic/Semantic/Procedural |
| Multi-agent | No | LangGraph | Yes | Native patterns |
| Streaming | Yes | Yes | Yes | Yes + backpressure |
| Local models | Via extra | Via extra | Via extra | First-class |

### 6.2 Error Code Reference

```nulang
enum AIError {
  // Provider errors
  AuthenticationError { provider: String, message: String },
  RateLimitError { provider: String, retry_after: Duration },
  ServerError { provider: String, status: Int },
  InvalidRequest { message: String },
  
  // Tool errors
  ToolNotFound { name: String },
  ToolExecutionError { name: String, error: String },
  ToolValidationError { name: String, field: String },
  ToolTimeout { name: String, timeout: Duration },
  
  // Memory errors
  MemoryFull { type: String, capacity: Int },
  EmbeddingError { message: String },
  
  // Orchestration errors
  AgentNotFound { name: String },
  MaxIterationsReached { count: Int },
  ConsensusNotReached { votes: Map<String, String> }
}
```

### 6.3 Performance Targets

| Metric | Target | Notes |
|--------|--------|-------|
| LLM request latency | < 100ms overhead | Excluding network time |
| Tool call overhead | < 5ms | Serialization + dispatch |
| Memory search | < 50ms | Semantic search with 10K docs |
| Agent initialization | < 10ms | From config to first message |
| Streaming throughput | > 10K tokens/sec | Token parsing rate |

### 6.4 Security Considerations

1. **API Key Management**: Keys stored in environment variables or Nulang's secrets manager
2. **Tool Sandbox**: Tools execute in a sandboxed context with resource limits
3. **Input Validation**: All tool arguments validated against schemas before execution
4. **Output Sanitization**: LLM outputs sanitized when used in sensitive contexts
5. **Audit Logging**: All AI requests logged for compliance and debugging

### 6.5 Glossary

| Term | Definition |
|------|------------|
| **Agent** | An AI-powered actor with model, tools, and memory |
| **Tool** | A typed function exposed to an agent |
| **Episodic Memory** | Short-term conversation history |
| **Semantic Memory** | Long-term knowledge via vector search |
| **Procedural Memory** | Learned patterns and examples |
| **Orchestration** | Coordination of multiple agents |
| **Saga** | Long-running transaction with compensation |
| **Circuit Breaker** | Pattern to prevent cascade failures |

---

*Document Version: 1.0.0*  
*Last Updated: 2024*  
*Status: Ready for Implementation*
