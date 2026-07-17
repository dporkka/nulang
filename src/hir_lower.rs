//! AST -> HIR lowering.
//!
//! Converts the parsed, type-checked AST into the typed High-level IR.
//! Expression types fall back to `Type::unit()` when no explicit annotation
//! is available; the bytecode backend is dynamically typed, so structural
//! fidelity (not type fidelity) is what matters here.
//!
//! Control flow in expression position (`if`, `match`, `for`) lowers to
//! dedicated `RValue` variants whose sub-bodies end in a `Yield` terminator.
//! This keeps evaluation order correct when statements follow the control
//! flow expression — the old design stored `if` as a *body terminator*,
//! which reordered any code lowered after it.

use crate::ai::request::ToolSchema;
use crate::ai::schema::function_to_tool_schema;
use crate::ast;
use crate::ast::{BinOp, Decl, Expr, FunctionAnnotation, Literal};
use crate::hir;
use crate::types::{Capability, EffectRow, Span, Type};

pub fn lower_module(ast: &ast::AstModule) -> hir::Module {
    let mut module = hir::Module::new(&ast.name);
    let tools = collect_tool_schemas(&ast.decls);
    for decl in &ast.decls {
        module.decls.push(lower_decl(decl, &tools));
    }
    module
}

/// Collect `@tool`-annotated function signatures across the whole module
/// (including nested modules), mirroring the stable compiler's
/// `collect_functions` so `agent` declarations can resolve `tools: [...]`
/// names regardless of source order.
fn collect_tool_schemas(decls: &[Decl]) -> Vec<ToolSchema> {
    let mut tools = Vec::new();
    collect_tool_schemas_into(decls, &mut tools);
    tools
}

fn collect_tool_schemas_into(decls: &[Decl], tools: &mut Vec<ToolSchema>) {
    for decl in decls {
        match decl {
            Decl::Function {
                name,
                params,
                ret_type,
                annotations,
                ..
            } if name != "__main" => {
                if let Some(FunctionAnnotation::Tool { description }) = annotations
                    .iter()
                    .find(|a| matches!(a, FunctionAnnotation::Tool { .. }))
                {
                    let mut typed_params = Vec::with_capacity(params.len());
                    let mut all_typed = true;
                    for (param_name, param_ty) in params {
                        if let Some(ty) = param_ty {
                            typed_params.push((param_name.clone(), ty.clone()));
                        } else {
                            all_typed = false;
                            break;
                        }
                    }
                    if all_typed {
                        let ret = ret_type.clone().unwrap_or_else(Type::unit);
                        tools.push(function_to_tool_schema(
                            name,
                            description,
                            &typed_params,
                            &ret,
                        ));
                    }
                }
            }
            Decl::Module {
                decls: subdecls, ..
            } => collect_tool_schemas_into(subdecls, tools),
            _ => {}
        }
    }
}

fn lower_decl(decl: &Decl, tools: &[ToolSchema]) -> hir::Decl {
    match decl {
        Decl::Function {
            name,
            type_params,
            params,
            ret_type,
            effect,
            cap,
            body,
            annotations: _,
            public,
            span,
        } => hir::Decl::Function(hir::FunctionDef {
            name: name.clone(),
            type_params: type_params.clone(),
            params: params
                .iter()
                .map(|(n, t)| (n.clone(), resolve_type(t)))
                .collect(),
            ret: resolve_type(ret_type),
            effect: effect.clone().unwrap_or_else(EffectRow::empty),
            cap: cap.unwrap_or(Capability::Ref),
            body: lower_body(body),
            public: *public,
            span: *span,
        }),
        Decl::Actor {
            name,
            type_params,
            persistent,
            state_fields,
            behaviors,
            init,
            span,
            ..
        } => hir::Decl::Actor(hir::ActorDef {
            name: name.clone(),
            type_params: type_params.clone(),
            persistent: *persistent,
            state_fields: state_fields
                .iter()
                .map(|(n, m, t, e)| {
                    let mut body = hir::Body::new();
                    let op = lower_expr(e, &mut body);
                    (n.clone(), *m, t.clone(), op)
                })
                .collect(),
            behaviors: behaviors.iter().map(lower_behavior).collect(),
            init: init
                .iter()
                .map(|(n, e)| {
                    let mut body = hir::Body::new();
                    let op = lower_expr(e, &mut body);
                    (n.clone(), op)
                })
                .collect(),
            is_workflow: false,
            is_agent: false,
            tools: Vec::new(),
            semantic_memory_dimensions: None,
            procedural_memory_namespace: None,
            fallback_config: String::new(),
            retry_config: String::new(),
            span: *span,
        }),
        Decl::TypeAlias {
            name,
            type_params,
            body,
            public,
            span,
        } => hir::Decl::TypeAlias {
            name: name.clone(),
            type_params: type_params.clone(),
            body: body.clone(),
            public: *public,
            span: *span,
        },
        Decl::RecordType {
            name,
            type_params,
            fields,
            public,
            span,
        } => hir::Decl::RecordType {
            name: name.clone(),
            type_params: type_params.clone(),
            fields: fields.clone(),
            public: *public,
            span: *span,
        },
        Decl::VariantType {
            name,
            type_params,
            variants,
            public,
            span,
        } => hir::Decl::VariantType {
            name: name.clone(),
            type_params: type_params.clone(),
            variants: variants.clone(),
            public: *public,
            span: *span,
        },
        Decl::EffectDecl { name, ops, span } => hir::Decl::EffectDecl {
            name: name.clone(),
            ops: ops.clone(),
            span: *span,
        },
        Decl::Extern {
            library,
            funcs,
            span,
        } => hir::Decl::ExternBlock {
            library: library.clone(),
            funcs: funcs
                .iter()
                .map(|f| hir::ExternFunc {
                    name: f.name.clone(),
                    params: f
                        .params
                        .iter()
                        .map(|(n, t)| (n.clone(), t.clone()))
                        .collect(),
                    ret: f.ret.clone(),
                    span: f.span,
                })
                .collect(),
            span: *span,
        },
        Decl::Module {
            name,
            exports,
            decls,
            span,
        } => hir::Decl::Module {
            name: name.clone(),
            exports: exports.clone(),
            decls: decls.iter().map(|d| lower_decl(d, tools)).collect(),
            span: *span,
        },
        Decl::Import { path, items, span } => hir::Decl::Import {
            path: path.clone(),
            items: items.clone(),
            span: *span,
        },
        Decl::Workflow {
            name, items, span, ..
        } => desugar_workflow(name, items, *span),
        Decl::StateMachine {
            name,
            states,
            events,
            entry_hooks,
            exit_hooks,
            span,
        } => {
            // Desugar to an ordinary actor declaration, then lower it
            // through the standard actor path (mirrors desugar_workflow,
            // which also targets hir::Decl::Actor).
            lower_decl(
                &ast::desugar_state_machine(name, states, events, entry_hooks, exit_hooks, *span),
                tools,
            )
        }
        Decl::Agent {
            name,
            model,
            system_prompt,
            tools: tool_names,
            memory,
            semantic_memory,
            procedural_memory,
            pricing,
            fallback,
            retry,
            span,
        } => desugar_agent(
            name,
            model,
            system_prompt,
            tool_names,
            memory,
            semantic_memory,
            procedural_memory,
            pricing,
            fallback,
            retry,
            tools,
            *span,
        ),
        Decl::Database { name, tables, span } => hir::Decl::Database {
            name: name.clone(),
            tables: tables.clone(),
            span: *span,
        },
    }
}

fn lower_behavior(b: &ast::Behavior) -> hir::BehaviorDef {
    hir::BehaviorDef {
        name: b.name.clone(),
        params: b
            .params
            .iter()
            .map(|(n, t)| (n.clone(), resolve_type(t)))
            .collect(),
        ret: Type::unit(),
        effect: b.effect.clone().unwrap_or_else(EffectRow::empty),
        cap: b.cap,
        body: lower_body(&b.body),
        compensate: None,
        parallel_branches: None,
        span: b.span,
    }
}

/// Placeholder behavior for a memory operation the runtime intercepts by
/// name (see `is_agent`/`semantic_memory_dimensions`/
/// `procedural_memory_namespace` on `hir::ActorDef`) instead of running its
/// bytecode body.
fn placeholder_behavior(name: &str, params: Vec<(&str, Type)>, span: Span) -> ast::Behavior {
    ast::Behavior {
        name: name.to_string(),
        params: params
            .into_iter()
            .map(|(n, t)| (n.to_string(), Some(t)))
            .collect(),
        body: Expr::Literal(Literal::Unit, span),
        effect: None,
        cap: Capability::Ref,
        span,
    }
}

/// Desugar an `agent Name = { ... }` declaration into an actor: durable
/// state fields hold the model/prompt/memory configuration, and generated
/// behaviors implement `ask`/`usage` (plus memory operations, intercepted by
/// the runtime rather than executed as bytecode). Mirrors the stable
/// compiler's `compile_agent` exactly, so both backends produce the same
/// source-level shape — synthesized `ast::Behavior` bodies are lowered
/// through the ordinary `lower_behavior`/`lower_expr` path.
#[allow(clippy::too_many_arguments)]
fn desugar_agent(
    name: &str,
    model: &str,
    system_prompt: &Option<String>,
    tool_names: &[String],
    memory: &Option<ast::AgentMemoryConfig>,
    semantic_memory: &Option<ast::AgentSemanticMemoryConfig>,
    procedural_memory: &Option<ast::AgentProceduralMemoryConfig>,
    pricing: &Option<ast::AgentPricing>,
    fallback: &[ast::AgentFallbackEntry],
    retry: &Option<ast::AgentRetryConfig>,
    available_tools: &[ToolSchema],
    span: Span,
) -> hir::Decl {
    // Resolve tool names against the module's @tool-annotated functions,
    // mirroring the stable compiler's `compile_agent`. An unresolvable name
    // means the whole program is invalid; fall back honestly so the stable
    // compiler raises the same "unknown tool" error instead of miscompiling.
    let mut resolved_tools = Vec::with_capacity(tool_names.len());
    for tool_name in tool_names {
        match available_tools.iter().find(|t| &t.name == tool_name) {
            Some(schema) => resolved_tools.push(schema.clone()),
            None => {
                return hir::Decl::Agent {
                    name: name.to_string(),
                    span,
                }
            }
        }
    }

    let agent_pricing = pricing.unwrap_or(ast::AgentPricing {
        input: 0.0,
        output: 0.0,
    });
    let max_turns = memory.as_ref().map(|m| m.max_turns).unwrap_or(50);
    let initial_memory = serde_json::to_string(&crate::ai::memory::EpisodicMemory::new(max_turns))
        .unwrap_or_else(|_| "{}".to_string());

    let semantic_memory_dimensions = semantic_memory.as_ref().map(|m| m.dimensions);
    let initial_semantic_memory = semantic_memory_dimensions.map(|dimensions| {
        serde_json::to_string(&crate::ai::SemanticMemory::new(dimensions, None))
            .unwrap_or_else(|_| "{}".to_string())
    });

    let procedural_memory_namespace = procedural_memory.as_ref().map(|m| m.namespace.clone());
    let initial_procedural_memory = procedural_memory_namespace.as_ref().map(|namespace| {
        serde_json::to_string(&crate::ai::ProceduralMemory::new(namespace.clone()))
            .unwrap_or_else(|_| "{}".to_string())
    });

    let str_ty = Type::string();
    let int_ty = Type::int();
    let float_ty = Type::float();
    let lit_op = |lit: Literal, ty: Type| hir::Operand::Literal(lit, ty);

    let mut state_fields: Vec<(String, ast::StateModel, Type, hir::Operand)> = vec![
        (
            "model".to_string(),
            ast::StateModel::Durable,
            str_ty.clone(),
            lit_op(Literal::String(model.to_string()), str_ty.clone()),
        ),
        (
            "system_prompt".to_string(),
            ast::StateModel::Durable,
            str_ty.clone(),
            lit_op(
                Literal::String(system_prompt.clone().unwrap_or_default()),
                str_ty.clone(),
            ),
        ),
        (
            "episodic_memory".to_string(),
            ast::StateModel::Durable,
            str_ty.clone(),
            lit_op(Literal::String(initial_memory), str_ty.clone()),
        ),
        (
            "usage_prompt".to_string(),
            ast::StateModel::Durable,
            int_ty.clone(),
            lit_op(Literal::Int(0), int_ty.clone()),
        ),
        (
            "usage_completion".to_string(),
            ast::StateModel::Durable,
            int_ty.clone(),
            lit_op(Literal::Int(0), int_ty.clone()),
        ),
        (
            "usage_cost".to_string(),
            ast::StateModel::Durable,
            float_ty.clone(),
            lit_op(Literal::Float(0.0), float_ty.clone()),
        ),
        (
            "pricing_input".to_string(),
            ast::StateModel::Durable,
            float_ty.clone(),
            lit_op(Literal::Float(agent_pricing.input), float_ty.clone()),
        ),
        (
            "pricing_output".to_string(),
            ast::StateModel::Durable,
            float_ty.clone(),
            lit_op(Literal::Float(agent_pricing.output), float_ty.clone()),
        ),
    ];
    if let Some(json) = initial_semantic_memory {
        state_fields.push((
            "semantic_memory".to_string(),
            ast::StateModel::Durable,
            str_ty.clone(),
            lit_op(Literal::String(json), str_ty.clone()),
        ));
    }
    if let Some(json) = initial_procedural_memory {
        state_fields.push((
            "procedural_memory".to_string(),
            ast::StateModel::Durable,
            str_ty.clone(),
            lit_op(Literal::String(json), str_ty.clone()),
        ));
    }

    // Serialize fallback config into a durable JSON string.
    let fallback_config_json = serde_json::to_string(&fallback)
        .unwrap_or_else(|_| "[]".to_string());
    state_fields.push((
        "fallback_config".to_string(),
        ast::StateModel::Durable,
        str_ty.clone(),
        lit_op(Literal::String(fallback_config_json), str_ty.clone()),
    ));

    // Serialize retry config into a durable JSON string.
    let retry_config_json = serde_json::to_string(&retry)
        .unwrap_or_else(|_| "null".to_string());
    state_fields.push((
        "retry_config".to_string(),
        ast::StateModel::Durable,
        str_ty.clone(),
        lit_op(Literal::String(retry_config_json), str_ty.clone()),
    ));

    // Tracking fields for the retry/fallback state machine.
    state_fields.push((
        "llm_attempt".to_string(),
        ast::StateModel::Durable,
        int_ty.clone(),
        lit_op(Literal::Int(0), int_ty.clone()),
    ));
    state_fields.push((
        "llm_fallback_step".to_string(),
        ast::StateModel::Durable,
        int_ty.clone(),
        lit_op(Literal::Int(0), int_ty.clone()),
    ));

    // Generated ask behavior reads agent state and performs the LLM ask.
    let ask_behavior = ast::Behavior {
        name: "ask".to_string(),
        params: vec![("prompt".to_string(), Some(str_ty.clone()))],
        body: Expr::Block {
            exprs: vec![
                Expr::FieldAccess {
                    expr: Box::new(Expr::SelfRef(span)),
                    field: "model".to_string(),
                    span,
                },
                Expr::FieldAccess {
                    expr: Box::new(Expr::SelfRef(span)),
                    field: "system_prompt".to_string(),
                    span,
                },
                Expr::FieldAccess {
                    expr: Box::new(Expr::SelfRef(span)),
                    field: "episodic_memory".to_string(),
                    span,
                },
                Expr::Perform {
                    effect: "LLM".to_string(),
                    op: "ask".to_string(),
                    args: vec![Expr::Var("prompt".to_string(), span)],
                    span,
                },
            ],
            span,
        },
        effect: None,
        cap: Capability::Ref,
        span,
    };

    // Generated usage behavior returns cumulative usage/cost state as a
    // plain array [prompt_tokens, completion_tokens, cost].
    let usage_behavior = ast::Behavior {
        name: "usage".to_string(),
        params: vec![],
        body: Expr::Array(
            vec![
                Expr::FieldAccess {
                    expr: Box::new(Expr::SelfRef(span)),
                    field: "usage_prompt".to_string(),
                    span,
                },
                Expr::FieldAccess {
                    expr: Box::new(Expr::SelfRef(span)),
                    field: "usage_completion".to_string(),
                    span,
                },
                Expr::FieldAccess {
                    expr: Box::new(Expr::SelfRef(span)),
                    field: "usage_cost".to_string(),
                    span,
                },
            ],
            span,
        ),
        effect: None,
        cap: Capability::Ref,
        span,
    };

    let mut behaviors = vec![
        lower_behavior(&ask_behavior),
        lower_behavior(&usage_behavior),
    ];

    if semantic_memory_dimensions.is_some() {
        behaviors.push(lower_behavior(&placeholder_behavior(
            "store_fact",
            vec![("content", str_ty.clone())],
            span,
        )));
        behaviors.push(lower_behavior(&placeholder_behavior(
            "recall",
            vec![("query", str_ty.clone()), ("top_k", int_ty.clone())],
            span,
        )));
    }
    if procedural_memory_namespace.is_some() {
        behaviors.push(lower_behavior(&placeholder_behavior(
            "store_pattern",
            vec![
                ("key", str_ty.clone()),
                ("input_pattern", str_ty.clone()),
                ("output_template", str_ty.clone()),
            ],
            span,
        )));
        behaviors.push(lower_behavior(&placeholder_behavior(
            "get_pattern",
            vec![("key", str_ty.clone())],
            span,
        )));
        behaviors.push(lower_behavior(&placeholder_behavior(
            "add_example",
            vec![
                ("task", str_ty.clone()),
                ("input", str_ty.clone()),
                ("output", str_ty.clone()),
            ],
            span,
        )));
        behaviors.push(lower_behavior(&placeholder_behavior(
            "get_examples",
            vec![
                ("task", str_ty.clone()),
                ("query", str_ty.clone()),
                ("top_k", int_ty.clone()),
            ],
            span,
        )));
    }

    // Already serialized above for state fields; reuse for ActorDef metadata.
    let fallback_config_str = serde_json::to_string(&fallback).unwrap_or_else(|_| "[]".to_string());
    let retry_config_str = serde_json::to_string(&retry).unwrap_or_else(|_| "null".to_string());

    hir::Decl::Actor(hir::ActorDef {
        name: name.to_string(),
        type_params: Vec::new(),
        persistent: true,
        state_fields,
        behaviors,
        init: Vec::new(),
        is_workflow: false,
        is_agent: true,
        tools: resolved_tools,
        semantic_memory_dimensions,
        procedural_memory_namespace,
        fallback_config: fallback_config_str,
        retry_config: retry_config_str,
        span,
    })
}

/// Desugar a `workflow Name { step ... }` declaration into a persistent
/// actor: one behavior per step, plus a durable `step_index` counter the
/// runtime advances as steps complete. Mirrors the stable compiler's
/// `compile_workflow` for the sequential case; a workflow containing a
/// `parallel` block falls back honestly (parallel-branch synthesis and its
/// progress-counter bookkeeping is a separate, not-yet-ported effort).
fn desugar_workflow(name: &str, items: &[ast::WorkflowItem], span: Span) -> hir::Decl {
    // Flatten the ordered workflow items into a list of sequential steps.
    // Each `parallel` block becomes a synthetic step whose body runs
    // branches sequentially (guarded by a durable `parallel_progress`
    // counter so recovery skips branches that already completed before a
    // crash) and emits a `ParallelBranchCompleted` event after each branch.
    // Mirrors the stable compiler's `compile_workflow` exactly.
    let mut flattened_steps: Vec<ast::WorkflowStep> = Vec::new();
    let mut parallel_branch_names: std::collections::HashMap<usize, Vec<String>> =
        std::collections::HashMap::new();
    let mut parallel_counter = 0usize;

    for item in items {
        match item {
            ast::WorkflowItem::Step(step) => flattened_steps.push(step.clone()),
            ast::WorkflowItem::Parallel(branches) => {
                let parallel_name = format!("parallel_{}", parallel_counter);
                parallel_counter += 1;

                let progress_expr = Expr::FieldAccess {
                    expr: Box::new(Expr::SelfRef(span)),
                    field: "parallel_progress".to_string(),
                    span,
                };
                let mut body_exprs: Vec<Expr> = Vec::with_capacity(branches.len() + 1);
                for (branch_idx, branch) in branches.iter().enumerate() {
                    let threshold = (branch_idx + 1) as i64;
                    let guard = Expr::Binary {
                        op: BinOp::Lt,
                        left: Box::new(progress_expr.clone()),
                        right: Box::new(Expr::Literal(Literal::Int(threshold), span)),
                        span,
                    };
                    let branch_block = Expr::Block {
                        exprs: vec![
                            branch.body.clone(),
                            Expr::Emit {
                                event: "ParallelBranchCompleted".to_string(),
                                args: vec![
                                    Expr::Literal(Literal::String(parallel_name.clone()), span),
                                    Expr::Literal(Literal::String(branch.name.clone()), span),
                                ],
                                span,
                            },
                        ],
                        span,
                    };
                    body_exprs.push(Expr::If {
                        cond: Box::new(guard),
                        then_branch: Box::new(branch_block),
                        else_branch: None,
                        span,
                    });
                }
                // Reset the parallel-progress counter once every branch has
                // finished. The runtime advances step_index when it records
                // StepCompleted so signal-waiting branches don't
                // double-increment.
                body_exprs.push(Expr::Assign {
                    target: Box::new(progress_expr.clone()),
                    value: Box::new(Expr::Literal(Literal::Int(0), span)),
                    span,
                });

                let combined_compensate = {
                    let comp_exprs: Vec<Expr> = branches
                        .iter()
                        .rev()
                        .filter_map(|b| b.compensate.clone())
                        .collect();
                    if comp_exprs.is_empty() {
                        None
                    } else {
                        Some(Expr::Block {
                            exprs: comp_exprs,
                            span,
                        })
                    }
                };

                flattened_steps.push(ast::WorkflowStep {
                    name: parallel_name.clone(),
                    body: Expr::Block {
                        exprs: body_exprs,
                        span,
                    },
                    compensate: combined_compensate,
                    span,
                });
                parallel_branch_names.insert(
                    flattened_steps.len() - 1,
                    branches.iter().map(|b| b.name.clone()).collect(),
                );
            }
        }
    }

    let state_fields: Vec<(String, ast::StateModel, Type, hir::Operand)> = vec![
        (
            "step_index".to_string(),
            ast::StateModel::Durable,
            Type::int(),
            hir::Operand::Literal(Literal::Int(0), Type::int()),
        ),
        (
            "workflow_name".to_string(),
            ast::StateModel::Durable,
            Type::string(),
            hir::Operand::Literal(Literal::String(name.to_string()), Type::string()),
        ),
        (
            "parallel_progress".to_string(),
            ast::StateModel::Durable,
            Type::int(),
            hir::Operand::Literal(Literal::Int(0), Type::int()),
        ),
    ];

    let behaviors = flattened_steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut def = lower_behavior(&ast::Behavior {
                name: s.name.clone(),
                params: Vec::new(),
                body: s.body.clone(),
                effect: None,
                cap: Capability::Ref,
                span: s.span,
            });
            def.compensate = s.compensate.as_ref().map(lower_body);
            def.parallel_branches = parallel_branch_names.get(&i).cloned();
            def
        })
        .collect();

    hir::Decl::Actor(hir::ActorDef {
        name: name.to_string(),
        type_params: Vec::new(),
        persistent: true,
        state_fields,
        behaviors,
        init: Vec::new(),
        is_workflow: true,
        is_agent: false,
        tools: Vec::new(),
        semantic_memory_dimensions: None,
        procedural_memory_namespace: None,
        fallback_config: String::new(),
        retry_config: String::new(),
        span,
    })
}

/// Lower an expression into a fresh body that yields the expression's value.
pub fn lower_body(expr: &Expr) -> hir::Body {
    let mut body = hir::Body::new();
    let op = lower_expr(expr, &mut body);
    if !body.is_terminated() {
        body.set_terminator(hir::Terminator::Yield(op));
    }
    body
}

/// Lower an expression into a sequence of statements in `body`, returning an
/// operand that represents the expression's value.
pub fn lower_expr(expr: &Expr, body: &mut hir::Body) -> hir::Operand {
    if body.is_terminated() {
        // Dead code after an explicit `return`/`break`: don't lower it.
        return hir::Operand::Unit;
    }
    match expr {
        Expr::Literal(lit, _span) => {
            let ty = literal_type(lit);
            hir::Operand::Literal(lit.clone(), ty)
        }
        Expr::Var(name, _span) => hir::Operand::Var(name.clone(), Type::unit()),
        Expr::SelfRef(_) => hir::Operand::Var("self".to_string(), Type::unit()),
        Expr::TypeAnnotate { expr, .. } => lower_expr(expr, body),
        Expr::CapAnnotate { expr, .. } => lower_expr(expr, body),
        Expr::Lambda {
            params,
            body: lb,
            effect: _,
            span,
        } => {
            let lambda_body = lower_body(lb);
            let ty = Type::unit();
            let captures = lambda_captures(params, lb);
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Closure {
                    params: params
                        .iter()
                        .map(|(n, t)| (n.clone(), resolve_type(t)))
                        .collect(),
                    body: Box::new(lambda_body),
                    captures,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::App { func, args, span } => {
            // Intercept AI runtime builtins: Pipeline.new(), Supervisor.new(),
            // Debate.new(...), and their method chains. Also intercept .run()
            // on pipeline/supervisor/debate instances.
            if let Some((base, field)) = is_ai_builtin_call(func) {
                let ty = Type::unit();
                let temp = fresh_temp_name();
                let rv = lower_ai_builtin(base, field, args, body, *span);
                body.push(hir::Stmt::Let {
                    name: temp.clone(),
                    ty: ty.clone(),
                    value: rv,
                    span: *span,
                });
                return hir::Operand::Var(temp, ty);
            }

            // Heuristic: `.run()` on any variable → resolve via variable name.
            if let Some(rv) = try_lower_run_call(func, args, body, *span) {
                let ty = Type::unit();
                let temp = fresh_temp_name();
                body.push(hir::Stmt::Let {
                    name: temp.clone(),
                    ty: ty.clone(),
                    value: rv,
                    span: *span,
                });
                return hir::Operand::Var(temp, ty);
            }

            let fop = lower_expr(func, body);
            let aops: Vec<_> = args.iter().map(|a| lower_expr(a, body)).collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Call {
                    func: fop,
                    args: aops,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Let {
            name,
            value,
            body: b,
            span,
            ..
        } => {
            // Let-bound lambdas may reference themselves (`let fac = fn(n) ...
            // fac(n-1)`); lower them like `let rec` so the self-reference
            // resolves. Non-self-referencing lambdas stay ordinary closures so
            // they can capture the enclosing scope.
            if let Expr::Lambda {
                params,
                body: lam_body,
                ..
            } = value.as_ref()
            {
                if lambda_references(name, params, lam_body) {
                    let func_body = lower_body(lam_body);
                    body.push(hir::Stmt::Let {
                        name: name.clone(),
                        ty: Type::unit(),
                        value: hir::RValue::RecClosure {
                            name: name.clone(),
                            params: params
                                .iter()
                                .map(|(n, t)| (n.clone(), resolve_type(t)))
                                .collect(),
                            body: Box::new(func_body),
                            ty: Type::unit(),
                        },
                        span: *span,
                    });
                    return lower_expr(b, body);
                }
            }
            let vop = lower_expr(value, body);
            let ty = vop.ty();
            body.push(hir::Stmt::Let {
                name: name.clone(),
                ty: ty.clone(),
                value: hir::RValue::Use(vop),
                span: *span,
            });
            lower_expr(b, body)
        }
        Expr::LetRec {
            name,
            params,
            value,
            body: b,
            span,
        } => {
            let func_body = lower_body(value);
            body.push(hir::Stmt::Let {
                name: name.clone(),
                ty: Type::unit(),
                value: hir::RValue::RecClosure {
                    name: name.clone(),
                    params: params
                        .iter()
                        .map(|(n, t)| (n.clone(), resolve_type(t)))
                        .collect(),
                    body: Box::new(func_body),
                    ty: Type::unit(),
                },
                span: *span,
            });
            lower_expr(b, body)
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            span,
        } => {
            let cond_op = lower_expr(cond, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            let then_body = lower_body(then_branch);
            let else_body = else_branch.as_ref().map(|e| Box::new(lower_body(e)));
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::If {
                    cond: cond_op,
                    then_body: Box::new(then_body),
                    else_body,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Match {
            scrutinee,
            arms,
            span,
        } => {
            let scrut_op = lower_expr(scrutinee, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            let arms_hir: Vec<_> = arms
                .iter()
                .map(|(pat, guard, e)| {
                    let guard_hir = guard.as_ref().map(|g| Box::new(lower_body(g)));
                    (pat.clone(), guard_hir, Box::new(lower_body(e)))
                })
                .collect();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Match {
                    scrutinee: scrut_op,
                    arms: arms_hir,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Block { exprs, span: _ } => {
            let mut last = hir::Operand::Unit;
            for e in exprs {
                if body.is_terminated() {
                    break;
                }
                last = lower_expr(e, body);
            }
            last
        }
        Expr::Tuple(elems, span) => {
            let ops: Vec<_> = elems.iter().map(|e| lower_expr(e, body)).collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Tuple(ops, ty.clone()),
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Record(fields, span) => {
            let fs: Vec<_> = fields
                .iter()
                .map(|(n, e)| (n.clone(), lower_expr(e, body)))
                .collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Record(fs, ty.clone()),
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::FieldAccess { expr, field, span } => {
            let base = lower_expr(expr, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::FieldAccess {
                    base,
                    field: field.clone(),
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Array(elems, span) => {
            let ops: Vec<_> = elems.iter().map(|e| lower_expr(e, body)).collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Array(ops, ty.clone()),
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Index { arr, idx, span } => {
            let aop = lower_expr(arr, body);
            let iop = lower_expr(idx, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Index {
                    base: aop,
                    idx: iop,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        // `self.field = v`, `arr[i] = v`, `record.f = v` are NOT parsed as
        // Expr::Assign (that node is only produced for a bare `ident = v`
        // prefix) — everywhere else, `=` is picked up by the Pratt parser's
        // infix loop as an ordinary-looking BinOp::Assign. Route it through
        // the same assignment lowering as Expr::Assign below.
        Expr::Binary {
            op: BinOp::Assign,
            left,
            right,
            span,
        } => lower_assign_to(left, right, *span, body),
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => {
            let l = lower_expr(left, body);
            let r = lower_expr(right, body);
            let ty = binary_type(op);
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Binary(*op, l, r, ty.clone()),
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Unary { op, expr, span } => {
            let e = lower_expr(expr, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Unary(*op, e, ty.clone()),
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Assign {
            target,
            value,
            span,
        } => lower_assign_to(target, value, *span, body),
        Expr::Spawn {
            actor_type,
            init,
            span,
            ..
        } => {
            let name = actor_name_from_expr(actor_type).unwrap_or_default();
            let init_ops: Vec<_> = init
                .iter()
                .map(|(n, e)| (n.clone(), lower_expr(e, body)))
                .collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Spawn {
                    actor_type: name,
                    init: init_ops,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Send {
            actor,
            behavior,
            args,
            span,
            ..
        } => {
            let aop = lower_expr(actor, body);
            let aops: Vec<_> = args.iter().map(|a| lower_expr(a, body)).collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Send {
                    actor: aop,
                    behavior: behavior.clone(),
                    args: aops,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Ask {
            actor,
            behavior,
            args,
            span,
        } => {
            let aop = lower_expr(actor, body);
            let aops: Vec<_> = args.iter().map(|a| lower_expr(a, body)).collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Ask {
                    actor: aop,
                    behavior: behavior.clone(),
                    args: aops,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Perform {
            effect,
            op,
            args,
            span,
        } => {
            let aops: Vec<_> = args.iter().map(|a| lower_expr(a, body)).collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Perform {
                    effect: effect.clone(),
                    op: op.clone(),
                    args: aops,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Handle {
            body: hb,
            handlers,
            span,
        } => {
            let hbody = lower_body(hb);
            let hs: Vec<_> = handlers
                .iter()
                .map(|h| hir::EffectHandler {
                    effect_name: h.effect_name.clone(),
                    op_name: h.op_name.clone(),
                    params: h.params.iter().map(|p| (p.clone(), Type::unit())).collect(),
                    resume: h.resume,
                    body: Box::new(lower_body(&h.body)),
                    span: *span,
                })
                .collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Handle {
                    body: Box::new(hbody),
                    handlers: hs,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Receive { arms, after, span } => {
            let arms_hir: Vec<_> = arms
                .iter()
                .map(|(name, params, e)| (name.clone(), params.clone(), Box::new(lower_body(e))))
                .collect();
            let after_hir = after
                .as_ref()
                .map(|(ms, body)| (Box::new(lower_body(ms)), Box::new(lower_body(body))));
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Receive {
                    arms: arms_hir,
                    after: after_hir,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Migrate { actor, node, span } => {
            let aop = lower_expr(actor, body);
            let nop = lower_expr(node, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Migrate {
                    actor: aop,
                    node: nop,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Emit { event, args, span } => {
            let aops: Vec<_> = args.iter().map(|a| lower_expr(a, body)).collect();
            body.push(hir::Stmt::Emit {
                event: event.clone(),
                args: aops,
                span: *span,
            });
            hir::Operand::Unit
        }
        Expr::For {
            var,
            iterable,
            body: b,
            span,
        } => {
            let iop = lower_expr(iterable, body);
            let loop_body = lower_body(b);
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: Type::unit(),
                value: hir::RValue::For {
                    var: var.clone(),
                    iterable: iop,
                    body: Box::new(loop_body),
                },
                span: *span,
            });
            hir::Operand::Var(temp, Type::unit())
        }
        Expr::While { cond, body: b, span } => {
            let cond_body = lower_body(cond);
            let loop_body = lower_body(b);
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: Type::unit(),
                value: hir::RValue::While {
                    cond: Box::new(cond_body),
                    body: Box::new(loop_body),
                    span: *span,
                },
                span: *span,
            });
            hir::Operand::Var(temp, Type::unit())
        }
        Expr::Pipe { left, right, span } => {
            // Lower `x |> f(a, b)` to `f(x, a, b)`, matching the stable
            // compiler's pipe semantics.
            let app = match right.as_ref() {
                Expr::App {
                    func,
                    args,
                    span: app_span,
                } => {
                    let mut new_args = vec![left.as_ref().clone()];
                    new_args.extend(args.iter().cloned());
                    Expr::App {
                        func: func.clone(),
                        args: new_args,
                        span: *app_span,
                    }
                }
                _ => Expr::App {
                    func: right.clone(),
                    args: vec![left.as_ref().clone()],
                    span: *span,
                },
            };
            lower_expr(&app, body)
        }
        Expr::Return(val, _span) => {
            let op = val.as_ref().map(|e| lower_expr(e, body));
            body.set_terminator(hir::Terminator::FnReturn(op));
            hir::Operand::Unit
        }
        Expr::Break(val, _span) => {
            let op = val.as_ref().map(|e| lower_expr(e, body));
            body.set_terminator(hir::Terminator::Break(op));
            hir::Operand::Unit
        }
    }
}

/// Shared lowering for both `Expr::Assign` (bare `ident = v`) and
/// `Expr::Binary { op: BinOp::Assign, .. }` (`self.f = v`, `arr[i] = v`,
/// `record.f = v` — everything else, since only a bare identifier target is
/// special-cased by the parser's prefix position).
fn lower_assign_to(target: &Expr, value: &Expr, span: Span, body: &mut hir::Body) -> hir::Operand {
    let val = lower_expr(value, body);
    let place = lower_place(target, body);
    body.push(hir::Stmt::Assign {
        target: place,
        value: hir::RValue::Use(val.clone()),
        span,
    });
    // Mirrors the stable compiler's `compile_assign`, which returns the
    // assigned value (val_reg) rather than unit — load-bearing for code
    // like `(emit E(), self.x = self.x + 1)`, whose block result is the
    // assignment's value.
    val
}

fn lower_place(expr: &Expr, body: &mut hir::Body) -> hir::Place {
    match expr {
        Expr::Var(name, _) => hir::Place::Var(name.clone(), Type::unit()),
        // `self` always parses as SelfRef, never Var("self", _) — without this
        // arm, `self.field = value` would fall through to the generic
        // temp-materializing case below and lose the "this is self" marker
        // that lower_assign's place_is_self check depends on.
        Expr::SelfRef(_) => hir::Place::Var("self".to_string(), Type::unit()),
        Expr::FieldAccess {
            expr,
            field,
            span: _,
        } => {
            let base = lower_place(expr, body);
            hir::Place::Field {
                base: Box::new(base),
                field: field.clone(),
                ty: Type::unit(),
            }
        }
        Expr::Index { arr, idx, span: _ } => {
            let base = lower_place(arr, body);
            let idx_op = lower_expr(idx, body);
            hir::Place::Index {
                base: Box::new(base),
                idx: idx_op,
                ty: Type::unit(),
            }
        }
        _ => {
            let op = lower_expr(expr, body);
            let name = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: name.clone(),
                ty: op.ty(),
                value: hir::RValue::Use(op),
                span: Span::default(),
            });
            hir::Place::Var(name, Type::unit())
        }
    }
}

/// Free variables of a lambda (candidates for capture). The MIR lowering
/// filters this against what is actually in scope.
fn lambda_captures(params: &[(String, Option<Type>)], body: &Expr) -> Vec<String> {
    let bound: std::collections::HashSet<String> = params.iter().map(|(n, _)| n.clone()).collect();
    let mut free = std::collections::HashSet::new();
    free_vars(body, &bound, &mut free);
    let mut captures: Vec<String> = free.into_iter().collect();
    captures.sort(); // deterministic ordering shared with codegen
    captures
}

/// Does a let-bound lambda reference its own binding name?
fn lambda_references(name: &str, params: &[(String, Option<Type>)], body: &Expr) -> bool {
    lambda_captures(params, body).iter().any(|c| c == name)
}

fn resolve_type(ty: &Option<Type>) -> Type {
    ty.clone().unwrap_or_else(Type::unit)
}

fn literal_type(lit: &Literal) -> Type {
    use crate::types::PrimitiveType;
    match lit {
        Literal::Int(_) => Type::Primitive(PrimitiveType::Int),
        Literal::Float(_) => Type::Primitive(PrimitiveType::Float),
        Literal::String(_) => Type::Primitive(PrimitiveType::String),
        Literal::Bool(_) => Type::Primitive(PrimitiveType::Bool),
        Literal::Nil => Type::Primitive(PrimitiveType::Nil),
        Literal::Unit => Type::Primitive(PrimitiveType::Unit),
    }
}

fn binary_type(op: &ast::BinOp) -> Type {
    use crate::ast::BinOp;
    use crate::types::PrimitiveType;
    match op {
        BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or => Type::Primitive(PrimitiveType::Bool),
        _ => Type::Primitive(PrimitiveType::Int),
    }
}
/// Returns `Some(builtin_name)` if the call's func is a field access on a
/// known builtin name (Pipeline, Supervisor, Debate), `None` otherwise.
fn is_ai_builtin_call(func: &Expr) -> Option<(&str, &str)> {
    match func {
        Expr::FieldAccess { expr, field, .. } => match expr.as_ref() {
            Expr::Var(name, _) => match name.as_str() {
                "Pipeline" | "Supervisor" | "Debate" => Some((name.as_str(), field.as_str())),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// Lower an AI runtime builtin call into an HIR RValue, lowering args
/// into the caller's body.
fn lower_ai_builtin(
    base_name: &str,
    field: &str,
    args: &[Expr],
    body: &mut hir::Body,
    _span: Span,
) -> hir::RValue {
    let ty = Type::unit();
    let mut a = |i: usize| {
        if i < args.len() {
            lower_expr(&args[i], body)
        } else {
            hir::Operand::Literal(Literal::String(String::new()), Type::string())
        }
    };

    match (base_name, field) {
        ("Pipeline", "new") => hir::RValue::PipelineNew { ty },
        ("Pipeline", "stage") => hir::RValue::PipelineStage {
            id: a(0),
            name: a(1),
            actor: a(2),
            template: a(3),
            ty,
        },
        ("Pipeline", "run") => hir::RValue::PipelineRun {
            id: a(0),
            input: a(1),
            ty,
        },
        ("Supervisor", "new") => hir::RValue::SupervisorNew { ty },
        ("Supervisor", "worker") => hir::RValue::SupervisorWorker {
            id: a(0),
            name: a(1),
            actor: a(2),
            description: a(3),
            ty,
        },
        ("Supervisor", "run") => hir::RValue::SupervisorRun {
            id: a(0),
            task: a(1),
            ty,
        },
        ("Debate", "new") => hir::RValue::DebateNew {
            topic: a(0),
            rounds: a(1),
            threshold: a(2),
            ty,
        },
        ("Debate", "participant") => hir::RValue::DebateParticipant {
            id: a(0),
            name: a(1),
            stance: a(2),
            actor: a(3),
            ty,
        },
        ("Debate", "run") => hir::RValue::DebateRun { id: a(0), ty },
        _ => unreachable!("is_ai_builtin_call should filter before lower_ai_builtin"),
    }
}

/// Heuristic: resolve `.run()` on a pipeline/supervisor/debate instance
/// by inspecting the variable name, mirroring the legacy compiler.
/// Lowers the receiver and args into `body`.
fn try_lower_run_call(
    func: &Expr,
    args: &[Expr],
    body: &mut hir::Body,
    _span: Span,
) -> Option<hir::RValue> {
    // Extract receiver and field from func: FieldAccess { expr: Var(name), field }
    let (base_name, receiver_expr, field) = match func {
        Expr::FieldAccess { expr, field, .. } => match expr.as_ref() {
            Expr::Var(name, _) => (name.as_str(), expr.as_ref(), field.as_str()),
            _ => return None,
        },
        _ => return None,
    };
    if field != "run" {
        return None;
    }

    let ty = Type::unit();
    let lowered = base_name.to_lowercase();

    // Lower the receiver (the pipeline/supervisor/debate variable) as the id.
    let id = lower_expr(receiver_expr, body);
    let mut a = |i: usize| {
        if i < args.len() {
            lower_expr(&args[i], body)
        } else {
            hir::Operand::Literal(Literal::String(String::new()), Type::string())
        }
    };

    if lowered.contains("debate") {
        Some(hir::RValue::DebateRun { id, ty })
    } else if lowered.contains("supervisor") || lowered.contains("team") {
        Some(hir::RValue::SupervisorRun { id, task: a(0), ty })
    } else {
        Some(hir::RValue::PipelineRun {
            id,
            input: a(0),
            ty,
        })
    }
}

fn actor_name_from_expr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Var(name, _) => Some(name.clone()),
        _ => None,
    }
}

static TEMP_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn fresh_temp_name() -> String {
    let n = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("__tmp{}", n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lower_literal() {
        let ast = ast::AstModule {
            name: "test".to_string(),
            decls: vec![Decl::Function {
                name: "__main".to_string(),
                type_params: vec![],
                params: vec![],
                ret_type: Some(Type::int()),
                effect: None,
                cap: None,
                body: Expr::Literal(Literal::Int(42), Span::default()),
                annotations: vec![],
                public: true,
                span: Span::default(),
            }],
        };
        let hir = lower_module(&ast);
        assert_eq!(hir.decls.len(), 1);
    }

    #[test]
    fn test_lower_if_is_expression_positioned() {
        // `let x = if c then 1 else 2 in x` must keep the if as an RValue so
        // statements after it stay in evaluation order.
        let source_body = Expr::Let {
            name: "x".to_string(),
            ty: None,
            value: Box::new(Expr::If {
                cond: Box::new(Expr::Literal(Literal::Bool(true), Span::default())),
                then_branch: Box::new(Expr::Literal(Literal::Int(1), Span::default())),
                else_branch: Some(Box::new(Expr::Literal(Literal::Int(2), Span::default()))),
                span: Span::default(),
            }),
            body: Box::new(Expr::Var("x".to_string(), Span::default())),
            span: Span::default(),
        };
        let body = lower_body(&source_body);
        // The if lowers to a Let stmt with an RValue::If, then x's Let, and
        // the body yields x.
        assert!(matches!(body.terminator, hir::Terminator::Yield(_)));
        assert!(body.stmts.iter().any(|s| matches!(
            s,
            hir::Stmt::Let {
                value: hir::RValue::If { .. },
                ..
            }
        )));
    }

    /// Regression test: `self.field = value` must lower to an Assign whose
    /// target is `Place::Field { base: Place::Var("self", _), .. }`. Before
    /// the SelfRef arm was added to `lower_place`, the generic fallback
    /// materialized `self` into an unrelated temp, silently breaking the
    /// `place_is_self` check every self-assignment codegen path depends on.
    #[test]
    fn test_lower_self_field_assign_targets_self_place() {
        let expr = Expr::Assign {
            target: Box::new(Expr::FieldAccess {
                expr: Box::new(Expr::SelfRef(Span::default())),
                field: "count".to_string(),
                span: Span::default(),
            }),
            value: Box::new(Expr::Literal(Literal::Int(1), Span::default())),
            span: Span::default(),
        };
        let mut body = hir::Body::new();
        lower_expr(&expr, &mut body);
        let assign = body
            .stmts
            .iter()
            .find_map(|s| match s {
                hir::Stmt::Assign { target, .. } => Some(target),
                _ => None,
            })
            .expect("assignment statement should be present");
        match assign {
            hir::Place::Field { base, field, .. } => {
                assert_eq!(field, "count");
                assert!(
                    matches!(base.as_ref(), hir::Place::Var(name, _) if name == "self"),
                    "field base should be Place::Var(\"self\", _), got {:?}",
                    base
                );
            }
            other => panic!("expected Place::Field, got {:?}", other),
        }
    }

    /// Regression test: `free_vars` must descend into the effect/actor
    /// expression families (perform, handle, spawn, send, ask, receive,
    /// migrate, emit). Before that, variables used only inside those
    /// expressions were never captured by closures, and MIR lowering
    /// failed with "undefined variable".
    #[test]
    fn test_free_vars_covers_effect_and_actor_exprs() {
        use std::collections::HashSet;
        let span = Span::default();
        let var = |n: &str| Expr::Var(n.to_string(), span);
        let used = |expr: &Expr| {
            let mut acc = HashSet::new();
            free_vars(expr, &HashSet::new(), &mut acc);
            acc
        };

        // perform Effect.op(k)
        let perform = Expr::Perform {
            effect: "IO".to_string(),
            op: "print".to_string(),
            args: vec![var("k")],
            span,
        };
        assert!(used(&perform).contains("k"), "perform arg must be free");

        // emit Event(k)
        let emit = Expr::Emit {
            event: "E".to_string(),
            args: vec![var("k")],
            span,
        };
        assert!(used(&emit).contains("k"), "emit arg must be free");

        // a ! beh(k) and ask a beh(k): receiver and args are free
        let send = Expr::Send {
            actor: Box::new(var("a")),
            behavior: "beh".to_string(),
            args: vec![var("k")],
            remote: false,
            span,
        };
        let send_vars = used(&send);
        assert!(send_vars.contains("a") && send_vars.contains("k"));
        let ask = Expr::Ask {
            actor: Box::new(var("a")),
            behavior: "beh".to_string(),
            args: vec![var("k")],
            span,
        };
        let ask_vars = used(&ask);
        assert!(ask_vars.contains("a") && ask_vars.contains("k"));

        // spawn Actor { count = k }
        let spawn = Expr::Spawn {
            actor_type: Box::new(var("Counter")),
            init: vec![("count".to_string(), var("k"))],
            positional_args: None,
            register_as: None,
            span,
        };
        assert!(used(&spawn).contains("k"), "spawn init must be free");

        // migrate a to n
        let migrate = Expr::Migrate {
            actor: Box::new(var("a")),
            node: Box::new(var("n")),
            span,
        };
        let migrate_vars = used(&migrate);
        assert!(migrate_vars.contains("a") && migrate_vars.contains("n"));

        // handle k { | IO.print(m) => h }: body and handler-body vars are
        // free; the handler param is bound.
        let handle = Expr::Handle {
            body: Box::new(var("k")),
            handlers: vec![ast::EffectHandler {
                effect_name: "IO".to_string(),
                op_name: "print".to_string(),
                params: vec!["m".to_string()],
                body: var("h"),
                resume: true,
            }],
            span,
        };
        let handle_vars = used(&handle);
        assert!(handle_vars.contains("k"), "handle body var must be free");
        assert!(handle_vars.contains("h"), "handler body var must be free");
        assert!(!handle_vars.contains("m"), "handler param is bound");

        // receive { | Msg(p) => k + p }: arm var free, arm params bound.
        let receive = Expr::Receive {
            arms: vec![(
                "Msg".to_string(),
                vec!["p".to_string()],
                Expr::Binary {
                    op: BinOp::Add,
                    left: Box::new(var("k")),
                    right: Box::new(var("p")),
                    span,
                },
            )],
            after: None,
            span,
        };
        let receive_vars = used(&receive);
        assert!(receive_vars.contains("k"), "receive arm var must be free");
        assert!(!receive_vars.contains("p"), "receive arm param is bound");

        // receive { | Msg() => 0 } after k => t: timeout expr and body are free.
        let receive_after = Expr::Receive {
            arms: vec![("Msg".to_string(), vec![], Expr::Literal(Literal::Int(0), span))],
            after: Some((Box::new(var("k")), Box::new(var("t")))),
            span,
        };
        let after_vars = used(&receive_after);
        assert!(after_vars.contains("k"), "receive-after ms expr must be free");
        assert!(after_vars.contains("t"), "receive-after body must be free");
    }

    #[test]
    fn test_lower_state_machine_desugars_to_actor() {
        // A state_machine lowers to an ordinary hir::Decl::Actor (the desugar
        // targets the existing actor machinery — no new IR shapes).
        let sp = Span::default();
        let ast = ast::AstModule {
            name: "test".to_string(),
            decls: vec![Decl::StateMachine {
                name: "TcpConnection".to_string(),
                states: vec!["Closed".to_string(), "Connected".to_string()],
                events: vec![
                    ast::StateMachineEvent {
                        name: "connect".to_string(),
                        params: vec![("address".to_string(), None)],
                        target: "Connected".to_string(),
                        span: sp,
                    },
                    ast::StateMachineEvent {
                        name: "disconnect".to_string(),
                        params: vec![],
                        target: "Closed".to_string(),
                        span: sp,
                    },
                ],
                entry_hooks: vec![("Connected".to_string(), Expr::Literal(Literal::Unit, sp))],
                exit_hooks: vec![],
                span: sp,
            }],
        };
        let hir = lower_module(&ast);
        assert_eq!(hir.decls.len(), 1);
        match &hir.decls[0] {
            hir::Decl::Actor(def) => {
                assert_eq!(def.name, "TcpConnection");
                assert!(!def.persistent);
                assert_eq!(def.state_fields.len(), 1);
                assert_eq!(def.state_fields[0].0, "_sm_state");
                assert_eq!(def.behaviors.len(), 2);
                assert_eq!(def.behaviors[0].name, "connect");
                assert_eq!(def.behaviors[0].params.len(), 1);
                assert_eq!(def.behaviors[1].name, "disconnect");
                // The transition lowers to a `_sm_state` field assign in the
                // event behavior body.
                assert!(
                    def.behaviors[0]
                        .body
                        .stmts
                        .iter()
                        .any(|s| matches!(s, hir::Stmt::Assign { .. })),
                    "event behavior should assign _sm_state"
                );
            }
            other => panic!("Expected actor declaration, got {:?}", other),
        }
    }
}

// ---------------------------------------------------------------------------
// Free variable analysis (moved from compiler.rs)
// ---------------------------------------------------------------------------

/// Collect all variable names bound by a pattern.
fn pattern_bindings(pat: &crate::ast::Pattern, out: &mut std::collections::HashSet<String>) {
    use crate::ast::Pattern;
    match pat {
        Pattern::Wild | Pattern::Lit(_) => {}
        Pattern::Var(name) | Pattern::Alias(name, _) => {
            out.insert(name.clone());
        }
        Pattern::Tuple(pats) => {
            for p in pats {
                pattern_bindings(p, out);
            }
        }
        Pattern::Record(fields) => {
            for (_, p) in fields {
                pattern_bindings(p, out);
            }
        }
        Pattern::Variant(_, Some(inner)) => pattern_bindings(inner, out),
        Pattern::Variant(_, None) => {}
    }
}

/// Collect free variables of an expression (variables used but not bound
/// within the expression). Shared between compiler and HIR lowering.
fn free_vars(
    expr: &crate::ast::Expr,
    bound: &std::collections::HashSet<String>,
    acc: &mut std::collections::HashSet<String>,
) {
    use crate::ast::Expr;
    match expr {
        Expr::Var(name, _) => {
            if !bound.contains(name) {
                acc.insert(name.clone());
            }
        }
        Expr::Lambda { params, body, .. } => {
            let mut new_bound = bound.clone();
            for (p, _) in params {
                new_bound.insert(p.clone());
            }
            free_vars(body, &new_bound, acc);
        }
        Expr::App { func, args, .. } => {
            free_vars(func, bound, acc);
            for a in args {
                free_vars(a, bound, acc);
            }
        }
        Expr::Let {
            name, value, body, ..
        } => {
            free_vars(value, bound, acc);
            let mut new_bound = bound.clone();
            new_bound.insert(name.clone());
            free_vars(body, &new_bound, acc);
        }
        Expr::LetRec {
            name,
            params,
            value,
            body,
            ..
        } => {
            let mut value_bound = bound.clone();
            value_bound.insert(name.clone());
            for (p, _) in params {
                value_bound.insert(p.clone());
            }
            free_vars(value, &value_bound, acc);
            let mut body_bound = bound.clone();
            body_bound.insert(name.clone());
            free_vars(body, &body_bound, acc);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            free_vars(cond, bound, acc);
            free_vars(then_branch, bound, acc);
            if let Some(e) = else_branch {
                free_vars(e, bound, acc);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            free_vars(scrutinee, bound, acc);
            for (pat, guard, arm_expr) in arms {
                let mut arm_bound = bound.clone();
                pattern_bindings(pat, &mut arm_bound);
                if let Some(guard_expr) = guard {
                    free_vars(guard_expr, &arm_bound, acc);
                }
                free_vars(arm_expr, &arm_bound, acc);
            }
        }
        Expr::Block { exprs, .. } | Expr::Tuple(exprs, _) | Expr::Array(exprs, _) => {
            for e in exprs {
                free_vars(e, bound, acc);
            }
        }
        Expr::Record(fields, _) => {
            for (_, e) in fields {
                free_vars(e, bound, acc);
            }
        }
        Expr::FieldAccess { expr, .. } => free_vars(expr, bound, acc),
        Expr::Index { arr, idx, .. } => {
            free_vars(arr, bound, acc);
            free_vars(idx, bound, acc);
        }
        Expr::Binary { left, right, .. } => {
            free_vars(left, bound, acc);
            free_vars(right, bound, acc);
        }
        Expr::Unary { expr, .. } => free_vars(expr, bound, acc),
        Expr::Pipe { left, right, .. } => {
            free_vars(left, bound, acc);
            free_vars(right, bound, acc);
        }
        Expr::Assign { target, value, .. } => {
            free_vars(target, bound, acc);
            free_vars(value, bound, acc);
        }
        Expr::For {
            var,
            iterable,
            body,
            ..
        } => {
            free_vars(iterable, bound, acc);
            let mut new_bound = bound.clone();
            new_bound.insert(var.clone());
            free_vars(body, &new_bound, acc);
        }
        Expr::While { cond, body, .. } => {
            free_vars(cond, bound, acc);
            free_vars(body, bound, acc);
        }
        Expr::Return(e, _) => {
            if let Some(e) = e {
                free_vars(e, bound, acc);
            }
        }
        Expr::TypeAnnotate { expr, .. } | Expr::CapAnnotate { expr, .. } => {
            free_vars(expr, bound, acc)
        }
        Expr::Spawn {
            actor_type, init, ..
        } => {
            free_vars(actor_type, bound, acc);
            for (_, e) in init {
                free_vars(e, bound, acc);
            }
        }
        Expr::Send { actor, args, .. } | Expr::Ask { actor, args, .. } => {
            free_vars(actor, bound, acc);
            for a in args {
                free_vars(a, bound, acc);
            }
        }
        Expr::Emit { args, .. } | Expr::Perform { args, .. } => {
            for a in args {
                free_vars(a, bound, acc);
            }
        }
        Expr::Handle {
            body, handlers, ..
        } => {
            free_vars(body, bound, acc);
            for h in handlers {
                let mut handler_bound = bound.clone();
                for p in &h.params {
                    handler_bound.insert(p.clone());
                }
                free_vars(&h.body, &handler_bound, acc);
            }
        }
        Expr::Receive { arms, after, .. } => {
            for (_, params, arm_expr) in arms {
                let mut arm_bound = bound.clone();
                for p in params {
                    arm_bound.insert(p.clone());
                }
                free_vars(arm_expr, &arm_bound, acc);
            }
            if let Some((ms, timeout_body)) = after {
                free_vars(ms, bound, acc);
                free_vars(timeout_body, bound, acc);
            }
        }
        Expr::Migrate { actor, node, .. } => {
            free_vars(actor, bound, acc);
            free_vars(node, bound, acc);
        }
        _ => {}
    }
}
