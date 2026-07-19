use std::marker::PhantomData;
use std::sync::Arc;

use anyhow::Result as AnyResult;
use serde::{Deserialize, Serialize};
use tower_lsp::async_trait;
use tower_lsp::jsonrpc;
use tower_lsp::lsp_types::{
    ExecuteCommandOptions, ExecuteCommandParams, InitializeParams, InitializeResult,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
};
use tower_lsp::{Client, LanguageServer};

/// LSP command exposed by `arc-lsp` for semantic-aware gutters and timeline UIs.
pub const COMMAND_GET_SEMANTIC_DIFF: &str = "arc/getSemanticDiff";

/// Query envelope for MCP tooling reads against the Intent Graph.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntentGraphQuery {
    /// Optional base revision selector.
    pub from: Option<String>,
    /// Optional target revision selector.
    pub to: Option<String>,
    /// Optional path filter.
    pub path: Option<String>,
}

/// Result envelope returned by the MCP tooling bridge.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntentGraphSnapshot {
    /// Human-readable summary of the queried slice.
    pub summary: String,
    /// Candidate intent links used by ranking/inspection UIs.
    pub links: Vec<String>,
}

/// MCP tooling boundary for agent access to repository intent data.
#[async_trait]
pub trait McpTooling: Send + Sync {
    /// Query intent-graph metadata for semantic ranking and explanation.
    async fn query_intent_graph(&self, query: IntentGraphQuery) -> AnyResult<IntentGraphSnapshot>;
}

/// Default no-op MCP bridge used by the scaffold.
pub struct NullMcpTooling;

#[async_trait]
impl McpTooling for NullMcpTooling {
    async fn query_intent_graph(&self, _query: IntentGraphQuery) -> AnyResult<IntentGraphSnapshot> {
        Ok(IntentGraphSnapshot {
            summary: "Intent graph bridge scaffold is active".to_string(),
            links: Vec::new(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SemanticDiffParams {
    from: Option<String>,
    to: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SemanticAtomDelta {
    atom_kind: String,
    target: String,
    summary: String,
}

#[derive(Debug, Clone, Serialize)]
struct SemanticDiffResult {
    from: Option<String>,
    to: Option<String>,
    deltas: Vec<SemanticAtomDelta>,
    scaffold: bool,
    intent_graph: IntentGraphSnapshot,
}

/// Semantic bridge server connecting LSP clients to arc change algebra and MCP tooling.
pub struct ArcLanguageServer {
    client: Client,
    mcp_tooling: Arc<dyn McpTooling>,
    _atom_marker: PhantomData<arc_core::algebra::Atom>,
}

fn internal_error(message: impl Into<String>) -> jsonrpc::Error {
    jsonrpc::Error {
        code: jsonrpc::ErrorCode::InternalError,
        message: message.into().into(),
        data: None,
    }
}

impl ArcLanguageServer {
    /// Build a new language server with an MCP tooling adapter.
    pub fn new(client: Client, mcp_tooling: Arc<dyn McpTooling>) -> Self {
        Self { client, mcp_tooling, _atom_marker: PhantomData }
    }

    async fn handle_get_semantic_diff(
        &self,
        params: ExecuteCommandParams,
    ) -> jsonrpc::Result<Option<serde_json::Value>> {
        let command_params = params
            .arguments
            .into_iter()
            .next()
            .map(serde_json::from_value::<SemanticDiffParams>)
            .transpose()
            .map_err(|err| jsonrpc::Error::invalid_params(format!("invalid params: {err}")))?
            .unwrap_or_default();

        let query = IntentGraphQuery {
            from: command_params.from.clone(),
            to: command_params.to.clone(),
            path: command_params.path.clone(),
        };

        let intent_graph = self
            .mcp_tooling
            .query_intent_graph(query)
            .await
            .map_err(|_err| internal_error("mcp query failed"))?;

        let result = SemanticDiffResult {
            from: command_params.from,
            to: command_params.to,
            deltas: Vec::new(),
            scaffold: true,
            intent_graph,
        };

        let value =
            serde_json::to_value(result).map_err(|_err| internal_error("serialization failed"))?;
        Ok(Some(value))
    }
}

#[async_trait]
impl LanguageServer for ArcLanguageServer {
    async fn initialize(&self, _: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![COMMAND_GET_SEMANTIC_DIFF.to_string()],
                    ..ExecuteCommandOptions::default()
                }),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: tower_lsp::lsp_types::InitializedParams) {
        self.client
            .log_message(
                tower_lsp::lsp_types::MessageType::INFO,
                "arc-lsp initialized: semantic diff and MCP tooling bridge online",
            )
            .await;
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> jsonrpc::Result<Option<serde_json::Value>> {
        match params.command.as_str() {
            COMMAND_GET_SEMANTIC_DIFF => self.handle_get_semantic_diff(params).await,
            _ => Err(jsonrpc::Error::method_not_found()),
        }
    }
}
