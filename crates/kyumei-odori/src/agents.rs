//! The explanation agent and its providers.

use std::sync::Arc;

use async_trait::async_trait;
use kyumei_diagnostics::explain;
use odori::{
    Agent, AgentRegistry, Providers, RunBudget,
    agents::provider::{
        Provider, TurnError, TurnEvent, TurnEventSink, TurnOutcome, TurnRequest, TurnUsage,
    },
    providers::{ClaudeProvider, CodexProvider},
};
use serde_json::{Value, json};

/// The agent name the workflow starts.
pub const EXPLAINER: &str = "kyumei-explainer";

/// Which model writes explanations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Explainer {
    /// Claude Code, through the authenticated `claude` CLI.
    Claude,
    /// Codex, through the authenticated `codex` CLI.
    Codex,
    /// A scripted provider for tests: deterministic, schema-valid replies.
    Scripted,
}

impl Explainer {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Scripted => "scripted test provider",
        }
    }

    fn provider_name(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Scripted => "scripted",
        }
    }

    /// Check the provider can run before relying on it.
    pub async fn preflight(&self) -> Result<(), String> {
        match self {
            Self::Claude => ClaudeProvider::new()
                .ensure_version()
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
            Self::Codex | Self::Scripted => Ok(()),
        }
    }

    pub fn registry(&self) -> AgentRegistry {
        let agent = Agent::new(EXPLAINER, explain::INSTRUCTIONS)
            .with_provider(self.provider_name())
            .with_budget(RunBudget::unlimited().with_max_turns(1))
            .with_output_schema(explain::draft_schema());
        let mut registry = AgentRegistry::new();
        registry.register(agent);
        registry
    }

    pub fn providers(&self) -> Providers {
        match self {
            Self::Claude => Providers::new(Arc::new(ClaudeProvider::new())),
            Self::Codex => Providers::new(Arc::new(CodexProvider::new())),
            Self::Scripted => Providers::new(Arc::new(ScriptedExplainer)),
        }
    }
}

/// A deterministic provider that answers like a careful model: it restates
/// the conclusion (or the leading findings) using only values present in the
/// prompt, so its drafts pass validation.
#[derive(Debug)]
pub struct ScriptedExplainer;

#[async_trait]
impl Provider for ScriptedExplainer {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn execute_turn(
        &self,
        request: TurnRequest,
        events: TurnEventSink,
    ) -> Result<TurnOutcome, TurnError> {
        let session = format!(
            "scripted-{}-{}",
            request.identity.run_id, request.identity.turn
        );
        events.emit(TurnEvent::SessionStarted {
            session_id: session.clone(),
        });
        let state: Value = request
            .input
            .find('{')
            .and_then(|i| serde_json::from_str(&request.input[i..]).ok())
            .unwrap_or(Value::Null);
        let summary = state["conclusion"]["statement"]
            .as_str()
            .map(|s| format!("In short: {s}"))
            .or_else(|| {
                state["findings"]
                    .as_array()
                    .and_then(|f| f.first())
                    .and_then(|f| f["statement"].as_str())
                    .map(|s| format!("So far: {s}"))
            })
            .unwrap_or_else(|| "No evidence has moved a hypothesis yet.".to_owned());
        let findings: Vec<Value> = state["findings"]
            .as_array()
            .map(|all| {
                all.iter()
                    .take(2)
                    .filter_map(|f| {
                        Some(json!({
                            "finding": f["id"].as_str()?,
                            "text": format!("Put simply: {}", f["statement"].as_str()?),
                        }))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let next = state["next_request"]["title"]
            .as_str()
            .map(|t| format!("Next, Kyumei needs: {t}."));
        let text = json!({
            "summary": summary,
            "findings": findings,
            "next_action": next,
            "diagram": { "kind": "diagnostic_path", "focus": [] },
        })
        .to_string();
        let mut usage = TurnUsage::default();
        usage.input_tokens = Some(request.input.len() as u64 / 4);
        usage.output_tokens = Some(text.len() as u64 / 4);
        usage.total_cost_usd = Some(0.0);
        let mut outcome = TurnOutcome::new(session, text);
        outcome.usage = usage;
        Ok(outcome)
    }
}
