//! Structured explanations.
//!
//! An explanation is not free Markdown: it is a summary, one entry per
//! finding, the remaining uncertainty, the next action, evidence references
//! and diagrams. The engine always produces a deterministic explanation. A
//! model may propose better prose ([`ExplanationDraft`]); the draft is merged
//! only if it passes validation against the assessment, so model prose can
//! never introduce numbers, findings or hypothesis states the evidence does
//! not support.

use std::{collections::BTreeSet, sync::LazyLock};

use kyumei_diagrams::{Diagram, DiagramKind};
use kyumei_domain::{
    Effect, EvidenceId, ExplanationAuthor, FindingId, HypothesisId, HypothesisState, Investigation,
    RequestDefinitionId, Uncertainty, format_number,
};
use kyumei_knowledge::DiagnosticDef;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{Assessment, ENGINE, diagrams};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticExplanation {
    pub author: ExplanationAuthor,
    pub summary: String,
    pub findings: Vec<ExplainedFinding>,
    pub uncertainties: Vec<Uncertainty>,
    pub next_action: Option<NextAction>,
    pub evidence_refs: Vec<EvidenceId>,
    pub diagrams: Vec<Diagram>,
    /// Notes about how the explanation was produced (e.g. a rejected draft).
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExplainedFinding {
    pub finding: FindingId,
    pub hypothesis: HypothesisId,
    pub effect: Effect,
    pub text: String,
    pub evidence: Vec<EvidenceId>,
    pub rests_on_assertion: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NextAction {
    pub request: RequestDefinitionId,
    pub title: String,
    pub text: String,
    /// True once a conclusion exists: further evidence would only sharpen it.
    pub optional: bool,
}

/// The deterministic explanation of an assessment.
pub fn deterministic(
    d: &DiagnosticDef,
    a: &Assessment,
    investigation: &Investigation,
) -> DiagnosticExplanation {
    let findings: Vec<ExplainedFinding> = a
        .findings
        .iter()
        .map(|f| ExplainedFinding {
            finding: f.id.clone(),
            hypothesis: f.hypothesis.clone(),
            effect: f.effect,
            text: f.statement.clone(),
            evidence: f.evidence(),
            rests_on_assertion: f.rests_on_assertion(),
        })
        .collect();
    let mut evidence_refs: Vec<EvidenceId> =
        findings.iter().flat_map(|f| f.evidence.clone()).collect();
    evidence_refs.sort();
    evidence_refs.dedup();
    let next_action = a.next().and_then(|p| {
        d.request(&p.definition).map(|r| NextAction {
            request: r.id.clone(),
            title: r.title.clone(),
            text: p.rationale.clone(),
            optional: a.conclusion.is_some(),
        })
    });
    DiagnosticExplanation {
        author: ExplanationAuthor::Deterministic {
            engine: ENGINE.to_owned(),
        },
        summary: summary(d, a),
        findings,
        uncertainties: a.uncertainties.clone(),
        next_action,
        evidence_refs,
        diagrams: diagrams::all(d, a, investigation),
        notes: Vec::new(),
    }
}

fn summary(d: &DiagnosticDef, a: &Assessment) -> String {
    if let Some(c) = &a.conclusion {
        return c.statement.clone();
    }
    let title = |id: &HypothesisId| d.hypothesis(id).map_or(id.to_string(), |h| h.title.clone());
    let list = |state: HypothesisState| -> Vec<String> {
        a.hypotheses
            .iter()
            .filter(|h| h.state == state && !h.contested)
            .map(|h| title(&h.id))
            .collect()
    };
    let mut parts = Vec::new();
    let established = list(HypothesisState::Established);
    let supported = list(HypothesisState::Supported);
    let eliminated = list(HypothesisState::Eliminated);
    let weakened = list(HypothesisState::Weakened);
    if !established.is_empty() {
        parts.push(format!("Established: {}.", established.join("; ")));
    }
    if !supported.is_empty() {
        parts.push(format!(
            "Evidence currently supports: {}.",
            supported.join("; ")
        ));
    }
    if !eliminated.is_empty() {
        parts.push(format!("Ruled out: {}.", eliminated.join("; ")));
    }
    if !weakened.is_empty() {
        parts.push(format!("Unlikely: {}.", weakened.join("; ")));
    }
    if parts.is_empty() {
        parts.push("No hypothesis has been moved by evidence yet.".to_owned());
    }
    if let Some(next) = a.next().and_then(|p| d.request(&p.definition)) {
        parts.push(format!("Next: {}.", next.title));
    }
    parts.join(" ")
}

// ---------------------------------------------------------------------------
// Model drafts
// ---------------------------------------------------------------------------

/// What a model may propose. Everything else comes from the engine.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExplanationDraft {
    /// Two to four sentences for an operator.
    pub summary: String,
    /// Clearer wording for findings, by finding ID.
    #[serde(default)]
    pub findings: Vec<DraftFinding>,
    /// Why the next evidence matters, in plain words.
    #[serde(default)]
    pub next_action: Option<String>,
    /// An approved diagram worth showing, with the hypotheses to focus on.
    #[serde(default)]
    pub diagram: Option<DiagramProposal>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DraftFinding {
    pub finding: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagramProposal {
    pub kind: DiagramKind,
    #[serde(default)]
    pub focus: Vec<String>,
}

/// JSON Schema for [`ExplanationDraft`] (strict-mode friendly).
pub fn draft_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "findings", "next_action", "diagram"],
        "properties": {
            "summary": {"type": "string", "maxLength": 900},
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["finding", "text"],
                    "properties": {
                        "finding": {"type": "string"},
                        "text": {"type": "string", "maxLength": 500}
                    }
                }
            },
            "next_action": {"type": ["string", "null"], "maxLength": 500},
            "diagram": {
                "type": ["object", "null"],
                "additionalProperties": false,
                "required": ["kind", "focus"],
                "properties": {
                    "kind": {"type": "string", "enum": ["diagnostic_path", "system_path", "evidence_relationship"]},
                    "focus": {"type": "array", "items": {"type": "string"}}
                }
            }
        }
    })
}

/// The structured state a model explains. It contains only what the engine
/// established; the model is told to use nothing else.
pub fn model_context(
    d: &DiagnosticDef,
    a: &Assessment,
    investigation: &Investigation,
) -> serde_json::Value {
    let hypotheses: Vec<serde_json::Value> = a
        .hypotheses
        .iter()
        .filter_map(|h| {
            d.hypothesis(&h.id).map(|def| {
                json!({
                    "id": h.id, "title": def.title, "statement": def.statement,
                    "state": h.state.label(), "contested": h.contested,
                    "findings": h.findings,
                })
            })
        })
        .collect();
    let facts: Vec<serde_json::Value> = a
        .facts
        .facts
        .values()
        .map(|f| {
            let values: serde_json::Map<String, serde_json::Value> = f
                .values
                .iter()
                .map(|(s, v)| {
                    (
                        s.label().to_owned(),
                        json!(s.unit_of(f.unit).format_value(*v)),
                    )
                })
                .collect();
            json!({
                "signal": f.signal,
                "title": d.signal(&f.signal).map(|s| s.title.clone()),
                "status": f.status.label(),
                "values": values,
                "categorical": f.categorical,
                "evidence": f.evidence,
            })
        })
        .collect();
    let findings: Vec<serde_json::Value> = a
        .findings
        .iter()
        .map(|f| json!({"id": f.id, "hypothesis": f.hypothesis, "effect": f.effect.label(), "statement": f.statement, "evidence": f.evidence(), "rests_on_simulation": f.rests_on_simulation()}))
        .collect();
    json!({
        "symptom": investigation.symptom.description,
        "server_version": investigation.environment.server_version.as_ref().map(|v| v.value.to_string()),
        "scope": investigation.scope,
        "incident_window": investigation.window.display(),
        "hypotheses": hypotheses,
        "facts": facts,
        "findings": findings,
        "uncertainties": a.uncertainties.iter().map(|u| u.text.clone()).collect::<Vec<_>>(),
        "next_request": a.next().and_then(|p| d.request(&p.definition)).map(|r| json!({"id": r.id, "title": r.title, "why": r.why})),
        "conclusion": a.conclusion.as_ref().map(|c| json!({"title": c.title, "statement": c.statement})),
    })
}

/// Instructions for the explanation agent.
pub const INSTRUCTIONS: &str = "You explain a Temporal diagnostic investigation to an on-call engineer. \
Use ONLY the JSON state you are given. Do not introduce any number, metric, configuration key, \
cause or recommendation that is not in the state. Do not change any hypothesis state: say \
'ruled out' only for eliminated hypotheses and 'established' only for established ones. \
A finding that rests on a simulation shows a cause is consistent with the symptom, not that it \
caused it: present it as a simulation, never as an observation. \
Be concise and specific. Do not use tools. Reply with ONLY a JSON object with keys \
summary (2-4 sentences), findings (array of {finding, text} using finding ids from the state), \
next_action (string or null) and diagram (null or {kind, focus}) where kind is one of \
diagnostic_path, system_path, evidence_relationship and focus lists hypothesis ids.";

/// Merge a model draft into the deterministic explanation, or say why not.
pub fn apply_draft(
    base: &DiagnosticExplanation,
    draft: &ExplanationDraft,
    d: &DiagnosticDef,
    a: &Assessment,
    investigation: &Investigation,
    author: ExplanationAuthor,
) -> Result<DiagnosticExplanation, Vec<String>> {
    let mut problems = Vec::new();
    let allowed = allowed_numbers(a, investigation);
    let check_text = |label: &str, text: &str, problems: &mut Vec<String>| {
        if text.trim().is_empty() {
            problems.push(format!("{label} is empty"));
        }
        for n in numbers_in(text) {
            if !allowed.iter().any(|a| close(*a, n))
                && !(n.fract() == 0.0 && (0.0..=10.0).contains(&n))
            {
                problems.push(format!(
                    "{label} introduces the number {} which is not in the evidence",
                    format_number(n)
                ));
            }
        }
        problems.extend(contradictions(label, text, d, a));
    };
    check_text("summary", &draft.summary, &mut problems);
    if draft.summary.chars().count() > 900 {
        problems.push("summary is too long".to_owned());
    }
    let mut merged = base.clone();
    for f in &draft.findings {
        let Some(target) = merged
            .findings
            .iter_mut()
            .find(|x| x.finding.as_str() == f.finding)
        else {
            problems.push(format!("draft refers to unknown finding `{}`", f.finding));
            continue;
        };
        check_text(&format!("finding {}", f.finding), &f.text, &mut problems);
        target.text = f.text.clone();
    }
    if let Some(next) = &draft.next_action {
        check_text("next action", next, &mut problems);
        if let Some(n) = merged.next_action.as_mut() {
            n.text = next.clone();
        }
    }
    if let Some(proposal) = &draft.diagram {
        let focus: Vec<HypothesisId> = proposal.focus.iter().map(HypothesisId::new).collect();
        if let Some(unknown) = focus.iter().find(|h| a.hypothesis(h).is_none()) {
            problems.push(format!(
                "diagram focus names unknown hypothesis `{unknown}`"
            ));
        } else if proposal.kind == DiagramKind::EvidenceRelationship && !focus.is_empty() {
            let focused = diagrams::evidence_relationship(d, a, Some(&focus));
            match kyumei_diagrams::validate(&focused) {
                Ok(()) if !focused.edges.is_empty() => {
                    merged
                        .diagrams
                        .retain(|x| x.kind != DiagramKind::EvidenceRelationship);
                    merged.diagrams.insert(0, focused);
                }
                Ok(()) => problems.push("proposed diagram focus has no findings".to_owned()),
                Err(e) => problems.push(e.to_string()),
            }
        } else if let Some(pos) = merged.diagrams.iter().position(|x| x.kind == proposal.kind) {
            let chosen = merged.diagrams.remove(pos);
            merged.diagrams.insert(0, chosen);
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    merged.summary = draft.summary.trim().to_owned();
    merged.author = author;
    Ok(merged)
}

static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d[\d,]*(?:\.\d+)?").expect("number pattern"));

fn numbers_in(text: &str) -> Vec<f64> {
    NUMBER
        .find_iter(text)
        .filter_map(|m| m.as_str().replace(',', "").parse::<f64>().ok())
        .collect()
}

fn close(a: f64, b: f64) -> bool {
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() <= scale * 0.051 || (a.round() - b.round()).abs() < 0.5
}

/// Every number the explanation may legitimately mention.
fn allowed_numbers(a: &Assessment, investigation: &Investigation) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    for f in a.facts.facts.values() {
        out.extend(f.values.values().copied());
        // Durations may be quoted in either unit.
        if f.unit == kyumei_domain::Unit::Seconds {
            out.extend(f.values.values().map(|v| v * 1000.0));
        }
        if let Some(c) = &f.categorical {
            out.extend(numbers_in(c));
        }
    }
    let mut texts: Vec<String> = a.findings.iter().map(|f| f.statement.clone()).collect();
    texts.extend(a.uncertainties.iter().map(|u| u.text.clone()));
    if let Some(c) = &a.conclusion {
        texts.push(c.statement.clone());
        texts.extend(c.recommendations.iter().map(|r| r.text.clone()));
    }
    texts.push(investigation.symptom.description.clone());
    texts.push(investigation.window.display());
    texts.extend(investigation.scope.task_queues.clone());
    texts.extend(investigation.scope.namespaces.clone());
    if let Some(v) = &investigation.environment.server_version {
        texts.push(v.value.to_string());
    }
    for s in &investigation.environment.sdks {
        texts.push(s.value.to_string());
    }
    for r in &investigation.requests {
        texts.push(r.window.text.clone());
        texts.push(r.id.to_string());
    }
    for e in &investigation.evidence {
        texts.push(e.id.to_string());
        for o in &e.interpretation.observations {
            texts.push(o.id.to_string());
        }
    }
    texts.extend(a.findings.iter().map(|f| f.id.to_string()));
    for t in texts {
        out.extend(numbers_in(&t));
    }
    out
}

static SENTENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^.!?]+[.!?]?").expect("sentence pattern"));

/// Sentences that claim a hypothesis state the assessment does not hold.
fn contradictions(label: &str, text: &str, d: &DiagnosticDef, a: &Assessment) -> Vec<String> {
    let mut problems = BTreeSet::new();
    let lowered = text.to_lowercase();
    for sentence in SENTENCE.find_iter(&lowered).map(|m| m.as_str()) {
        for h in &a.hypotheses {
            let Some(def) = d.hypothesis(&h.id) else {
                continue;
            };
            let title = def.title.to_lowercase();
            if !sentence.contains(&title) {
                continue;
            }
            let negated = sentence.contains(" not ") || sentence.contains("n't");
            let says_eliminated = ["ruled out", "eliminated", "excluded"]
                .iter()
                .any(|w| sentence.contains(w));
            let says_established = ["established", "confirmed", "root cause"]
                .iter()
                .any(|w| sentence.contains(w));
            if says_eliminated && !negated && h.state != HypothesisState::Eliminated {
                problems.insert(format!(
                    "{label} says “{}” is ruled out, but it is {}",
                    def.title,
                    h.state.label()
                ));
            }
            if says_established && !negated && h.state != HypothesisState::Established {
                problems.insert(format!(
                    "{label} says “{}” is established, but it is {}",
                    def.title,
                    h.state.label()
                ));
            }
        }
    }
    problems.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_numbers() {
        assert_eq!(
            numbers_in("dispatch 1,995/s vs add 2,100/s at 31.4 s"),
            vec![1995.0, 2100.0, 31.4]
        );
        assert!(close(31.4, 31.0));
        assert!(!close(31.4, 45.0));
    }
}
