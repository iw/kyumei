//! Activities: the deterministic core and explanation validation, run
//! outside workflow code so their results are recorded in history.

#![allow(missing_debug_implementations)]

use std::sync::Arc;

use jiff::Timestamp;
use kyumei_app::{Command, ExplanationStatus, blob::BlobStore, core, simulation::SimulationRun};
use kyumei_diagnostics::{DiagnosticExplanation, ExplanationDraft, explain};
use kyumei_domain::{
    EvidenceId, ExplanationAuthor, ExplanationRecord, Investigation, InvestigationEvent,
    RecordedEvent,
};
use kyumei_knowledge::KnowledgeBase;
use serde::{Deserialize, Serialize};
use temporalio_macros::activities;
use temporalio_sdk::{
    ApplicationFailure,
    activities::{ActivityContext, ActivityError},
};

pub struct InvestigationActivities {
    pub kb: Arc<KnowledgeBase>,
    pub blobs: Arc<BlobStore>,
    /// Run the simulations an assessment says are due.
    pub simulate: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandleInput {
    pub events: Vec<RecordedEvent>,
    pub command: Command,
    pub at: Timestamp,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum HandleOutput {
    Applied {
        events: Vec<(Timestamp, InvestigationEvent)>,
        evidence: Option<EvidenceId>,
    },
    /// The command was invalid. Not an activity failure: retrying would not help.
    Rejected { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimulateInput {
    pub events: Vec<RecordedEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimulateOutput {
    /// The run to record, when a simulation was due.
    pub run: Option<SimulationRun>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrepareInput {
    pub events: Vec<RecordedEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrepareOutput {
    /// The model prompt, or `None` when no diagnostic applies.
    pub prompt: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidateInput {
    pub events: Vec<RecordedEvent>,
    pub seq: u64,
    pub text: Result<String, String>,
    pub provider: String,
    pub run: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidateOutput {
    pub explanation: Option<DiagnosticExplanation>,
    pub status: ExplanationStatus,
    pub record: ExplanationRecord,
}

fn terminal(message: impl Into<String>) -> ActivityError {
    ActivityError::application(ApplicationFailure::non_retryable(message.into()))
}

fn replay(events: &[RecordedEvent]) -> Result<Investigation, ActivityError> {
    Investigation::replay(events)
        .map_err(|e| terminal(format!("investigation log is invalid: {e}")))
}

#[activities]
impl InvestigationActivities {
    /// Apply one command with the deterministic core.
    #[activity]
    pub async fn handle_command(
        self: Arc<Self>,
        _ctx: ActivityContext,
        input: HandleInput,
    ) -> Result<HandleOutput, ActivityError> {
        let investigation = replay(&input.events)?;
        let mut command = input.command;
        if let Err(e) = kyumei_app::blob::resolve(&mut command, &self.blobs) {
            return match e.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData => {
                    Ok(HandleOutput::Rejected {
                        reason: e.to_string(),
                    })
                }
                // Retryable: the content may be readable on the next attempt.
                _ => Err(e.into()),
            };
        }
        match core::handle(&self.kb, &investigation, &command, input.at) {
            Ok(outcome) => {
                for (sha, bytes) in &outcome.blobs {
                    // Retryable: a transient filesystem error should not lose evidence.
                    self.blobs.put(sha, bytes)?;
                }
                Ok(HandleOutput::Applied {
                    events: outcome.events,
                    evidence: outcome.evidence,
                })
            }
            Err(e) => Ok(HandleOutput::Rejected {
                reason: e.to_string(),
            }),
        }
    }

    /// Run the first simulation the latest assessment says is due, if any. A
    /// run that fails comes back as a run with its failure, to be recorded, so
    /// retrying never repeats it.
    #[activity]
    pub async fn simulate(
        self: Arc<Self>,
        _ctx: ActivityContext,
        input: SimulateInput,
    ) -> Result<SimulateOutput, ActivityError> {
        if !self.simulate {
            return Ok(SimulateOutput { run: None });
        }
        let investigation = replay(&input.events)?;
        let assessment = core::current_assessment(&self.kb, &investigation)
            .map_err(|e| terminal(e.to_string()))?;
        let Some(plan) = assessment.simulations.first().cloned() else {
            return Ok(SimulateOutput { run: None });
        };
        let run = tokio::task::spawn_blocking(move || kyumei_app::simulation::run(&plan))
            .await
            .map_err(|e| terminal(format!("the simulation stopped: {e}")))?;
        Ok(SimulateOutput { run: Some(run) })
    }

    /// Build the explanation agent's prompt from the current assessment.
    #[activity]
    pub async fn prepare_explanation(
        self: Arc<Self>,
        _ctx: ActivityContext,
        input: PrepareInput,
    ) -> Result<PrepareOutput, ActivityError> {
        let investigation = replay(&input.events)?;
        Ok(PrepareOutput {
            prompt: prompt(&self.kb, &investigation),
        })
    }

    /// Validate the model's draft against the assessment it explains.
    #[activity]
    pub async fn validate_explanation(
        self: Arc<Self>,
        _ctx: ActivityContext,
        input: ValidateInput,
    ) -> Result<ValidateOutput, ActivityError> {
        let investigation = replay(&input.events)?;
        Ok(validate(&self.kb, &investigation, input))
    }
}

/// The prompt: instructions are on the agent; this is the state to explain.
pub fn prompt(kb: &KnowledgeBase, investigation: &Investigation) -> Option<String> {
    let (_, _, diagnostic) = kyumei_diagnostics::select(kb, investigation).ok()?;
    let d = diagnostic?;
    let assessment = core::current_assessment(kb, investigation).ok()?;
    let context = explain::model_context(d, &assessment, investigation);
    Some(format!(
        "Explain this Kyumei investigation state for the operator. Use only this JSON; reply with only the JSON object described in your instructions.\n\n{}",
        serde_json::to_string_pretty(&context).unwrap_or_default()
    ))
}

/// Merge a model reply into the deterministic explanation, or fall back.
pub fn validate(
    kb: &KnowledgeBase,
    investigation: &Investigation,
    input: ValidateInput,
) -> ValidateOutput {
    let deterministic_record = |note: String| ExplanationRecord {
        author: ExplanationAuthor::Deterministic {
            engine: kyumei_diagnostics::ENGINE.to_owned(),
        },
        digest: String::new(),
        assessment_seq: input.seq,
        note: Some(note),
    };
    let fallback = |reason: String| ValidateOutput {
        explanation: None,
        status: ExplanationStatus::Fallback {
            seq: input.seq,
            reason: reason.clone(),
        },
        record: deterministic_record(reason),
    };
    let text = match &input.text {
        Ok(t) => t,
        Err(e) => return fallback(e.clone()),
    };
    let Ok((_, _, Some(d))) = kyumei_diagnostics::select(kb, investigation) else {
        return fallback("no diagnostic applies".to_owned());
    };
    let Ok(assessment) = core::current_assessment(kb, investigation) else {
        return fallback("the investigation could not be assessed".to_owned());
    };
    let draft: ExplanationDraft =
        match extract_json(text).and_then(|j| serde_json::from_str(j).ok()) {
            Some(d) => d,
            None => return fallback("the model reply was not the requested JSON".to_owned()),
        };
    let base = explain::deterministic(d, &assessment, investigation);
    let author = ExplanationAuthor::Model {
        provider: input.provider.clone(),
        model: None,
        run: input.run.clone(),
    };
    match explain::apply_draft(&base, &draft, d, &assessment, investigation, author.clone()) {
        Ok(explanation) => {
            let digest = kyumei_evidence_digest(&explanation);
            ValidateOutput {
                status: ExplanationStatus::Current { seq: input.seq },
                record: ExplanationRecord {
                    author,
                    digest,
                    assessment_seq: input.seq,
                    note: None,
                },
                explanation: Some(explanation),
            }
        }
        Err(problems) => fallback(format!("rejected: {}", problems.join("; "))),
    }
}

/// The first balanced JSON object in a model reply (tolerating code fences
/// and surrounding prose).
pub fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, ch) in text[start..].char_indices() {
        match ch {
            _ if escaped => escaped = false,
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

fn kyumei_evidence_digest(explanation: &DiagnosticExplanation) -> String {
    kyumei_evidence::sha256_hex(
        serde_json::to_string(explanation)
            .unwrap_or_default()
            .as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_fenced_replies() {
        let reply =
            "Here you go:\n```json\n{\"summary\": \"a {b}\", \"findings\": []}\n```\nThanks";
        assert_eq!(
            extract_json(reply),
            Some("{\"summary\": \"a {b}\", \"findings\": []}")
        );
        assert_eq!(extract_json("no json"), None);
    }
}
