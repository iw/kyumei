//! What a collector reads for an evidence request: the request's PromQL
//! queries over its window, as a request code for `kyumei-collect`, or for
//! Kyumei's own Collect now.

use jiff::SignedDuration;
use kyumei_diagnostics::select::range_step;
use kyumei_domain::{
    CollectionMethod, EvidenceRequest, EvidenceRequestId, InvestigationId, TimeWindow,
};
use kyumei_samples::{RequestCode, Window, placeholders};

/// The companion's name, as the command line uses it.
pub const COMPANION: &str = "kyumei-collect";

/// The queries a request asks for, with the window and step to read them over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionPlan {
    /// PromQL range queries; `<name>` stands for a value Kyumei does not know.
    pub queries: Vec<String>,
    pub window: TimeWindow,
    pub step: SignedDuration,
    /// The `<name>` placeholders in the queries, in first-seen order.
    pub unfilled: Vec<String>,
}

impl CollectionPlan {
    /// The plan for a request with PromQL instructions and a known window. Each
    /// query's witness, when it has one, is read too, so that the query's empty
    /// result can read as zero.
    pub fn for_request(r: &EvidenceRequest) -> Option<Self> {
        let window = r.window.window?;
        let mut queries: Vec<String> = Vec::new();
        for c in r
            .collect
            .iter()
            .filter(|c| c.method == CollectionMethod::Promql)
        {
            let witness = c.absent_reads_zero.as_ref().map(|z| &z.witness);
            for q in std::iter::once(&c.body).chain(witness) {
                if !queries.contains(q) {
                    queries.push(q.clone());
                }
            }
        }
        if queries.is_empty() {
            return None;
        }
        let mut unfilled: Vec<String> = Vec::new();
        for q in &queries {
            for name in placeholders(q) {
                if !unfilled.contains(&name) {
                    unfilled.push(name);
                }
            }
        }
        Some(Self {
            queries,
            window,
            step: range_step(window.duration()),
            unfilled,
        })
    }

    /// The values the queries still need, as the context form names them:
    /// "the namespace and the Task Queue". `None` when nothing is missing.
    pub fn missing(&self) -> Option<String> {
        let names: Vec<String> = self
            .unfilled
            .iter()
            .map(|n| match n.as_str() {
                "namespace" => "the namespace".to_owned(),
                "task_queue" => "the Task Queue".to_owned(),
                other => format!("<{other}>"),
            })
            .collect();
        match names.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            [rest @ .., last] => Some(format!("{} and {last}", rest.join(", "))),
        }
    }

    /// The step as PromQL writes a duration: `15s`, `1m`.
    pub fn step_text(&self) -> String {
        match self.step.as_secs() {
            s if s > 0 && s % 60 == 0 => format!("{}m", s / 60),
            s => format!("{s}s"),
        }
    }

    /// The range in the form collectors take.
    pub fn sample_window(&self) -> Window {
        Window {
            start_ms: self.window.start.as_millisecond(),
            end_ms: self.window.end.as_millisecond(),
            step_ms: u64::try_from(self.step.as_millis()).unwrap_or(15_000),
        }
    }

    pub fn code(
        &self,
        investigation: &InvestigationId,
        request: &EvidenceRequestId,
    ) -> RequestCode {
        let w = self.sample_window();
        RequestCode {
            investigation: investigation.to_string(),
            request: request.to_string(),
            queries: self.queries.clone(),
            start_ms: w.start_ms,
            end_ms: w.end_ms,
            step_ms: w.step_ms,
        }
    }

    /// The command to copy: the code, and a `--set` to fill for each value
    /// Kyumei does not know.
    pub fn command(&self, investigation: &InvestigationId, request: &EvidenceRequestId) -> String {
        let mut command = format!(
            "{COMPANION} --code {}",
            self.code(investigation, request).encode()
        );
        for name in &self.unfilled {
            command.push_str(&format!(" --set {name}="));
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use kyumei_domain::{CollectionInstruction, RequestStatus, RequestedWindow};

    use super::*;

    fn request(bodies: &[&str], window: Option<TimeWindow>) -> EvidenceRequest {
        EvidenceRequest {
            id: "R2".into(),
            definition: "workflow-worker-slots".into(),
            issued_at: "2026-10-13T15:31:38Z".parse().expect("ts"),
            title: "t".into(),
            why: "w".into(),
            collect: bodies
                .iter()
                .map(|b| CollectionInstruction {
                    method: CollectionMethod::Promql,
                    label: "l".into(),
                    body: (*b).to_owned(),
                    applies_to: None,
                    unit: None,
                    claims: vec![],
                    unfilled: vec![],
                    absent_reads_zero: None,
                })
                .collect(),
            window: RequestedWindow {
                window,
                text: String::new(),
            },
            group_by: vec![],
            distinguishes: vec![],
            expected_signals: vec![],
            responses: vec![],
            status: RequestStatus::Open,
        }
    }

    fn window(minutes: i64) -> TimeWindow {
        let start: jiff::Timestamp = "2026-10-13T11:32:00Z".parse().expect("ts");
        TimeWindow::new(start, start + SignedDuration::from_mins(minutes)).expect("window")
    }

    #[test]
    fn a_plan_carries_the_queries_window_and_unknown_values() {
        let r = request(
            &[
                r#"sum by (task_queue) (temporal_worker_task_slots_available{namespace="<namespace>", task_queue="<task_queue>"})"#,
            ],
            Some(window(31)),
        );
        let plan = CollectionPlan::for_request(&r).expect("plan");
        assert_eq!(plan.unfilled, vec!["namespace", "task_queue"]);
        assert_eq!(
            plan.missing().as_deref(),
            Some("the namespace and the Task Queue")
        );
        assert_eq!(plan.step, SignedDuration::from_secs(15));
        assert_eq!(plan.step_text(), "15s");
        let command = plan.command(&"inv-1".into(), &r.id);
        assert!(
            command.starts_with("kyumei-collect --code kyq1."),
            "{command}"
        );
        assert!(
            command.ends_with(" --set namespace= --set task_queue="),
            "{command}"
        );
        let code =
            RequestCode::decode(command.split_whitespace().nth(2).expect("code")).expect("decode");
        assert_eq!(code.request, "R2");
        assert_eq!(code.investigation, "inv-1");
        assert_eq!(code.queries, plan.queries);
        assert_eq!(code.end_ms - code.start_ms, 31 * 60_000);
    }

    #[test]
    fn long_windows_take_longer_steps_and_requests_without_promql_or_a_window_have_none() {
        let plan = CollectionPlan::for_request(&request(&["up"], Some(window(240)))).expect("plan");
        assert_eq!(plan.missing(), None);
        assert_eq!(plan.step, SignedDuration::from_mins(1));
        assert_eq!(plan.step_text(), "1m");
        assert!(CollectionPlan::for_request(&request(&["up"], None)).is_none());
        assert!(CollectionPlan::for_request(&request(&[], Some(window(30)))).is_none());
    }
}
