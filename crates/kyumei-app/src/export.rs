//! Markdown export, rendered from the same workspace model as the interface.

use std::fmt::Write as _;

use crate::workspace::{ClaimView, HypothesisRow, Workspace};

/// References as a list of links, each with why it applies and where it is cited from.
fn references(o: &mut String, refs: &[crate::dashboards::ReferenceView]) {
    for r in refs {
        let title = match &r.url {
            Some(url) => format!("[{}]({url})", r.title),
            None => r.title.clone(),
        };
        let detail = match (&r.panel, &r.version) {
            (Some(p), _) => format!(" (panel {} of {})", p.id, p.dashboard_title),
            (None, Some(v)) => format!(" ({v})"),
            (None, None) => String::new(),
        };
        let _ = writeln!(o, "- {}: {title}{detail}. {} _{}_", r.label, r.why, r.cited);
    }
    let _ = writeln!(o);
}

pub fn markdown(w: &Workspace) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "# {}\n", w.title);
    let _ = writeln!(
        o,
        "_Kyumei investigation `{}` · {}_\n",
        w.id, w.status.label
    );

    let _ = writeln!(o, "## Situation\n");
    let _ = writeln!(o, "> {}\n", w.situation.symptom.replace('\n', "\n> "));
    if let Some(t) = &w.situation.symptom_title {
        let _ = writeln!(
            o,
            "- **Symptom:** {t}{}",
            w.situation
                .classification
                .as_ref()
                .map(|c| format!(" ({c})"))
                .unwrap_or_default()
        );
    }
    for (k, v) in &w.situation.scope {
        let _ = writeln!(o, "- **{k}:** {v}");
    }
    let _ = writeln!(o, "- **Incident window:** {}", w.situation.window);
    for e in &w.situation.environment {
        let _ = writeln!(o, "- **{}:** {} _({})_", e.label, e.value, e.status);
    }
    let _ = writeln!(o, "- **Knowledge:** {}\n", w.situation.knowledge);

    if let Some(c) = &w.assessment.conclusion {
        let _ = writeln!(
            o,
            "## Diagnosis{}\n",
            if c.provisional {
                " (supportable; path still being completed)"
            } else {
                ""
            }
        );
        let _ = writeln!(o, "**{}**\n", c.title);
        let _ = writeln!(o, "{}\n", c.statement);
        if !c.recommendations.is_empty() {
            let _ = writeln!(o, "### Recommendations\n");
            for r in &c.recommendations {
                let _ = writeln!(o, "- {}", r.text);
                if let Some(caution) = &r.caution {
                    let _ = writeln!(o, "  - _Caution:_ {caution}");
                }
                if !r.claims.is_empty() {
                    let _ = writeln!(o, "  - _Relies on:_ {}", claims(&r.claims));
                }
            }
            let _ = writeln!(o);
        }
        if !c.references.is_empty() {
            let _ = writeln!(o, "### Runbooks and playbooks\n");
            references(&mut o, &c.references);
        }
        let _ = writeln!(
            o,
            "Supported by findings {} and evidence {}.\n",
            c.findings.join(", "),
            c.evidence.join(", ")
        );
    }

    let _ = writeln!(o, "## Current assessment\n");
    let _ = writeln!(o, "{}\n", w.assessment.summary);
    let _ = writeln!(o, "### What Kyumei knows\n");
    let _ = writeln!(
        o,
        "| Signal | Value | Standing | Evidence |\n|---|---|---|---|"
    );
    for f in &w.assessment.knows {
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} |",
            f.title,
            f.summary,
            f.status.label(),
            f.evidence.join(", ")
        );
    }
    let _ = writeln!(o);
    hypotheses(&mut o, "Leading", &w.assessment.leading);
    hypotheses(&mut o, "Ruled out", &w.assessment.eliminated);
    hypotheses(&mut o, "Open", &w.assessment.open);
    if !w.assessment.uncertainties.is_empty() {
        let _ = writeln!(o, "### Uncertainty\n");
        for u in &w.assessment.uncertainties {
            let _ = writeln!(o, "- **{}:** {}", u.label, u.text);
        }
        let _ = writeln!(o);
    }

    if let Some(n) = &w.next {
        let _ = writeln!(
            o,
            "## Next evidence{}\n",
            if n.optional { " (optional)" } else { "" }
        );
        let _ = writeln!(o, "**{}** ({})\n", n.title, n.request);
        let _ = writeln!(o, "{}\n", n.why);
        for c in &n.collect {
            let _ = writeln!(
                o,
                "- {} — {}{}",
                c.method,
                c.label,
                c.applies_to
                    .as_ref()
                    .map(|a| format!(" ({a})"))
                    .unwrap_or_default()
            );
            let _ = writeln!(o, "\n  ```\n  {}\n  ```", c.body);
        }
        let _ = writeln!(o, "\n- **Window:** {}", n.window);
        if !n.group_by.is_empty() {
            let _ = writeln!(o, "- **Group by:** {}", n.group_by.join(", "));
        }
        let _ = writeln!(o, "- **Distinguishes:** {}\n", n.distinguishes.join("; "));
        if !n.references.is_empty() {
            let _ = writeln!(o, "Where operators look:\n");
            references(&mut o, &n.references);
        }
    }

    if !w.findings.is_empty() {
        let _ = writeln!(o, "## Findings\n");
        for f in &w.findings {
            let _ = writeln!(
                o,
                "### {} — {} “{}”\n",
                f.id,
                f.effect.label(),
                f.hypothesis
            );
            let _ = writeln!(o, "{}\n", f.statement);
            for b in &f.basis {
                let _ = writeln!(
                    o,
                    "- {} {}: {} _({})_ — {}",
                    b.signal,
                    b.statistic,
                    b.value,
                    b.status.label(),
                    b.sources.join("; ")
                );
            }
            if !f.claims.is_empty() {
                let _ = writeln!(o, "- _Knowledge:_ {}", claims(&f.claims));
            }
            let _ = writeln!(o, "- _Rule:_ `{}` when {}\n", f.rule, f.condition);
        }
    }

    if !w.diagrams.is_empty() {
        let _ = writeln!(o, "## Diagrams\n");
        for d in &w.diagrams {
            let _ = writeln!(o, "### {}\n\n```text\n{}```\n", d.title, d.text);
        }
    }

    let _ = writeln!(o, "## Evidence\n");
    let _ = writeln!(
        o,
        "| ID | Submitted ({}) | Form | Origin | Answers | Informs |\n|---|---|---|---|---|---|",
        w.zone.label
    );
    for e in &w.evidence {
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} | {} | {} |",
            e.id,
            e.submitted,
            e.form,
            e.origin,
            e.responds_to.clone().unwrap_or_else(|| "—".to_owned()),
            if e.bound.is_empty() {
                "—".to_owned()
            } else {
                e.bound.join(", ")
            }
        );
    }
    let _ = writeln!(o, "\n## Trail\n");
    let _ = writeln!(
        o,
        "| Time ({}) | Event | Refs |\n|---|---|---|",
        w.zone.label
    );
    for t in &w.trail {
        let _ = writeln!(o, "| {} | {} | {} |", t.time, t.text, t.refs.join(", "));
    }
    let _ = writeln!(
        o,
        "\n---\n_Explanation: {} — {}_",
        w.explanation.author, w.explanation.status
    );
    o
}

fn hypotheses(o: &mut String, heading: &str, rows: &[HypothesisRow]) {
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(o, "### {heading}\n");
    for h in rows {
        let state = if h.contested {
            format!("{} (contested)", h.state.label())
        } else {
            h.state.label().to_owned()
        };
        let findings = if h.findings.is_empty() {
            String::new()
        } else {
            format!(" — {}", h.findings.join(", "))
        };
        let _ = writeln!(
            o,
            "- **{}** _{}_: {}{}",
            h.title, state, h.statement, findings
        );
    }
    let _ = writeln!(o);
}

fn claims(c: &[ClaimView]) -> String {
    c.iter()
        .map(|c| format!("`{}` ({}, {})", c.name, c.category, c.applicability))
        .collect::<Vec<_>>()
        .join("; ")
}
