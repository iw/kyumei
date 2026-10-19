//! The durable investigation workflow.
//!
//! One workflow execution per investigation. Its history *is* the
//! investigation's durability: the event log lives in workflow state, every
//! operator command arrives as a workflow update and is applied by an
//! activity running the deterministic core, and a model explanation for the
//! latest assessment is produced by Odori's `AgentRun` as a child workflow.
//!
//! The workflow orchestrates; it never diagnoses. All diagnosis happens in
//! `kyumei-app`'s core, inside activities, so a knowledge or engine upgrade
//! can never make workflow replay non-deterministic.

#![allow(missing_debug_implementations)]

use std::time::Duration;

use jiff::Timestamp;
use kyumei_app::{Command, ExplanationStatus};
use kyumei_diagnostics::DiagnosticExplanation;
use kyumei_domain::{EvidenceId, InvestigationEvent, RecordedEvent};
use odori::{
    RunConfig, RunEnd, RunOutput,
    agents::{AgentRun, run::RunInput},
};
use serde::{Deserialize, Serialize};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{
    ActivityOptions, ChildWorkflowOptions, SyncWorkflowContext, WorkflowContext,
    WorkflowContextView, WorkflowResult,
};

use crate::activities::{
    HandleInput, HandleOutput, InvestigationActivities, PrepareInput, PrepareOutput, SimulateInput,
    SimulateOutput, ValidateInput, ValidateOutput,
};

/// How explanations are produced, when a model is configured.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplainConfig {
    /// The Odori agent that writes explanations.
    pub agent: String,
    /// The task queue Odori's worker polls.
    pub agent_queue: String,
    /// Human description, e.g. "Claude Code".
    pub provider: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvestigationInput {
    /// The investigation so far: at least the `opened` event and the first
    /// assessment, or a complete log when rehydrating.
    pub events: Vec<RecordedEvent>,
    pub explain: Option<ExplainConfig>,
}

/// What the workflow reports to queries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowView {
    pub events: Vec<RecordedEvent>,
    pub explanation: Option<DiagnosticExplanation>,
    pub explanation_status: ExplanationStatus,
    pub commands: u64,
    pub closed: bool,
}

/// A command with the id it is submitted under. Sending it again under the same
/// id returns the reply it already had, rather than applying it twice.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Submission {
    pub id: String,
    pub command: Command,
}

/// How many submissions' replies the workflow remembers: enough for any resend.
const REMEMBERED_REPLIES: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum CommandReply {
    Applied {
        events: usize,
        evidence: Option<EvidenceId>,
    },
    Rejected {
        reason: String,
    },
}

#[workflow]
#[derive(Default)]
pub struct InvestigationWorkflow {
    events: Vec<RecordedEvent>,
    /// A command or explanation is being applied; others wait.
    busy: bool,
    closed: bool,
    explain: Option<ExplainConfig>,
    /// The assessment the latest explanation attempt covered.
    explained_seq: Option<u64>,
    /// The assessment the latest simulation check covered.
    simulated_seq: Option<u64>,
    explanation: Option<DiagnosticExplanation>,
    explanation_status: Option<ExplanationStatus>,
    commands: u64,
    /// The replies to the latest submissions, by submission id.
    replies: Vec<(String, CommandReply)>,
}

impl InvestigationWorkflow {
    fn reply_to(&self, submission: &str) -> Option<CommandReply> {
        self.replies
            .iter()
            .find(|(id, _)| id == submission)
            .map(|(_, r)| r.clone())
    }

    fn remember(&mut self, submission: String, reply: CommandReply) {
        self.replies.push((submission, reply));
        let excess = self.replies.len().saturating_sub(REMEMBERED_REPLIES);
        self.replies.drain(..excess);
    }

    fn latest_assessment_seq(&self) -> Option<u64> {
        self.events
            .iter()
            .rev()
            .find(|e| matches!(e.event, InvestigationEvent::Assessed(_)))
            .map(|e| e.seq)
    }

    fn wants_simulation(&self) -> bool {
        !self.busy
            && self.latest_assessment_seq().is_some()
            && self.simulated_seq != self.latest_assessment_seq()
    }

    fn wants_explanation(&self) -> bool {
        self.explain.is_some()
            && !self.busy
            && self.latest_assessment_seq().is_some()
            && self.explained_seq != self.latest_assessment_seq()
    }

    fn append(&mut self, new: Vec<(Timestamp, InvestigationEvent)>) -> usize {
        let n = new.len();
        for (at, event) in new {
            let seq = self.events.len() as u64;
            self.events.push(RecordedEvent { seq, at, event });
        }
        n
    }

    fn to_view(&self) -> WorkflowView {
        WorkflowView {
            events: self.events.clone(),
            explanation: self.explanation.clone(),
            explanation_status: self
                .explanation_status
                .clone()
                .unwrap_or(ExplanationStatus::Deterministic),
            commands: self.commands,
            closed: self.closed,
        }
    }
}

fn now(ctx: &WorkflowContext<InvestigationWorkflow>) -> Timestamp {
    ctx.workflow_time()
        .and_then(|t| Timestamp::try_from(t).ok())
        .unwrap_or_else(|| ctx.state(|s| s.events.last().map(|e| e.at).unwrap_or_default()))
}

fn activity_options(seconds: u64) -> ActivityOptions {
    ActivityOptions::with_start_to_close_timeout(Duration::from_secs(seconds)).build()
}

#[workflow_methods]
impl InvestigationWorkflow {
    #[run]
    pub async fn run(
        ctx: &mut WorkflowContext<Self>,
        input: InvestigationInput,
    ) -> WorkflowResult<WorkflowView> {
        ctx.state_mut(|s| {
            s.events = input.events.clone();
            s.explain = input.explain.clone();
        });
        loop {
            ctx.wait_condition(|s| s.closed || s.wants_simulation() || s.wants_explanation())
                .await?;
            if ctx.state(|s| s.closed) {
                break;
            }
            // Simulations first, so that the explanation covers what they found.
            if ctx.state(|s| s.wants_simulation()) {
                let seq = ctx.state(|s| s.latest_assessment_seq().unwrap_or_default());
                ctx.state_mut(|s| s.simulated_seq = Some(seq));
                // Histories from before simulations replay without them.
                if ctx.patched(SIMULATION_PATCH) {
                    simulate(ctx, seq).await;
                    continue;
                }
            }
            if !ctx.state(|s| s.wants_explanation()) {
                continue;
            }
            // One explanation round for the latest assessment. Commands keep
            // being processed by the update handler meanwhile.
            let Some(config) = ctx.state(|s| s.explain.clone()) else {
                continue;
            };
            let (events, seq) = ctx.state(|s| {
                (
                    s.events.clone(),
                    s.latest_assessment_seq().unwrap_or_default(),
                )
            });
            ctx.state_mut(|s| {
                s.explained_seq = Some(seq);
                s.explanation_status = Some(ExplanationStatus::Pending { seq });
            });
            let prepared: PrepareOutput = ctx
                .execute_activity(
                    InvestigationActivities::prepare_explanation,
                    PrepareInput {
                        events: events.clone(),
                    },
                    activity_options(60),
                )
                .await?;
            let text = match prepared.prompt {
                None => Err("no diagnostic applies, so there is nothing to explain".to_owned()),
                Some(prompt) => {
                    let child_id = format!("{}-explain-{seq}", ctx.workflow_id());
                    let started = ctx
                        .start_child_workflow(
                            AgentRun::run,
                            RunInput {
                                agent: config.agent.clone(),
                                prompt,
                                config: RunConfig::default()
                                    .with_turn_max_attempts(2)
                                    .with_turn_timeout(Duration::from_secs(300)),
                                handoff: None,
                            },
                            ChildWorkflowOptions::builder()
                                .workflow_id(child_id)
                                .task_queue(config.agent_queue.clone())
                                .build(),
                        )
                        .await;
                    match started {
                        Err(e) => Err(format!(
                            "could not start the explanation agent: {}",
                            chain(&e)
                        )),
                        Ok(child) => match child.result().await {
                            Err(e) => Err(format!("the explanation agent failed: {}", chain(&e))),
                            Ok(output) => text_of(output),
                        },
                    }
                }
            };
            let validated: ValidateOutput = ctx
                .execute_activity(
                    InvestigationActivities::validate_explanation,
                    ValidateInput {
                        events,
                        seq,
                        text,
                        provider: config.provider.clone(),
                        run: format!("{}-explain-{seq}", ctx.workflow_id()),
                    },
                    activity_options(60),
                )
                .await?;
            // Record the explanation in the log, serialised with commands.
            ctx.wait_condition(|s| !s.busy).await?;
            let at = now(ctx);
            ctx.state_mut(|s| {
                if let Some(explanation) = validated.explanation {
                    s.explanation = Some(explanation);
                }
                s.explanation_status = Some(validated.status);
                s.append(vec![(
                    at,
                    InvestigationEvent::ExplanationRecorded(validated.record),
                )]);
            });
        }
        Ok(ctx.state(|s| s.to_view()))
    }

    /// Apply an operator command once and return what happened: sent again
    /// under the same id, as after the engine reported it unprocessed, it gets
    /// the reply it already had.
    #[update]
    pub async fn submit_once(
        ctx: &mut WorkflowContext<Self>,
        submission: Submission,
    ) -> CommandReply {
        apply(ctx, submission).await
    }

    /// Stop the workflow. The investigation remains queryable until then.
    #[update]
    pub fn close(&mut self, _ctx: &mut SyncWorkflowContext<Self>, _reason: String) -> bool {
        self.closed = true;
        true
    }

    #[query]
    pub fn view(&self, _ctx: &WorkflowContextView) -> WorkflowView {
        self.to_view()
    }
}

/// The patch that added simulations to the workflow.
const SIMULATION_PATCH: &str = "simulation";

/// Run the simulation the assessment at `seq` says is due, if any, and record
/// it as a command. A simulation that can't run leaves the investigation as it
/// was.
async fn simulate(ctx: &mut WorkflowContext<InvestigationWorkflow>, seq: u64) {
    let events = ctx.state(|s| s.events.clone());
    let output = ctx
        .execute_activity(
            InvestigationActivities::simulate,
            SimulateInput { events },
            activity_options(300),
        )
        .await;
    if let Ok(SimulateOutput { run: Some(run) }) = output {
        apply(
            ctx,
            Submission {
                id: format!("simulation-{seq}"),
                command: Command::RecordSimulation(Box::new(run)),
            },
        )
        .await;
    }
}

/// Apply a submission's command, once per submission id.
async fn apply(
    ctx: &mut WorkflowContext<InvestigationWorkflow>,
    submission: Submission,
) -> CommandReply {
    let Submission { id, command } = submission;
    if ctx.state(|s| s.closed) {
        return CommandReply::Rejected {
            reason: "the investigation is closed".to_owned(),
        };
    }
    if ctx.wait_condition(|s| !s.busy).await.is_err() {
        return CommandReply::Rejected {
            reason: "the investigation was cancelled".to_owned(),
        };
    }
    // Checked once no other command is being applied, so a repeat that
    // waited on the first finds its reply.
    if let Some(reply) = ctx.state(|s| s.reply_to(&id)) {
        return reply;
    }
    ctx.state_mut(|s| s.busy = true);
    let at = now(ctx);
    let events = ctx.state(|s| s.events.clone());
    let result = ctx
        .execute_activity(
            InvestigationActivities::handle_command,
            HandleInput {
                events,
                command,
                at,
            },
            activity_options(120),
        )
        .await;
    let reply = match result {
        Ok(HandleOutput::Applied { events, evidence }) => {
            let n = ctx.state_mut(|s| {
                s.commands += 1;
                s.append(events)
            });
            CommandReply::Applied {
                events: n,
                evidence,
            }
        }
        Ok(HandleOutput::Rejected { reason }) => CommandReply::Rejected { reason },
        Err(e) => CommandReply::Rejected {
            reason: format!("the command could not be applied: {e}"),
        },
    };
    ctx.state_mut(|s| {
        s.remember(id, reply.clone());
        s.busy = false;
    });
    reply
}

/// An error with its causes, plus a hint for the common setup failures.
fn chain(e: &dyn std::error::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !parts.contains(&text) {
            parts.push(text);
        }
        source = s.source();
    }
    let debug = format!("{e:?}");
    let mut message = parts.join(": ");
    for (needle, hint) in [
        (
            "authenticate",
            "the model CLI is not logged in (run `claude login` or `codex login`)",
        ),
        (
            "usage limit",
            "the model subscription's usage limit was reached",
        ),
        (
            "No such file",
            "the model CLI is not installed or not on PATH",
        ),
    ] {
        if (message.contains(needle) || debug.contains(needle))
            && !message.contains("login")
            && !message.contains(hint)
        {
            message.push_str(&format!(" — {hint}"));
        }
    }
    if parts.len() == 1 {
        // The SDK's Display hides the activity failure; its Debug form keeps it.
        if let Some(i) = debug.find("message: \"") {
            let rest = &debug[i + 10..];
            if let Some(end) = rest.find('"') {
                message.push_str(&format!(" ({})", &rest[..end]));
            }
        }
    }
    message
}

fn text_of(output: RunOutput) -> Result<String, String> {
    match output.end {
        RunEnd::Completed | RunEnd::ConversationEnded => Ok(output.text),
        RunEnd::BudgetExceeded { cap } => Err(format!("budget exceeded: {cap:?}")),
        RunEnd::GuardrailBlocked { guardrail, reason } => {
            Err(format!("guardrail {guardrail}: {reason}"))
        }
        other => Err(format!("run ended without an answer: {other:?}")),
    }
}
