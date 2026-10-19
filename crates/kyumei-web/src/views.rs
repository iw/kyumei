//! Workspace sections.

use kyumei_app::workspace::{
    CollectorView, DiagramView, FindingView, HypothesisRow, NextEvidence, Situation, TrailRow,
    Workspace,
};
use kyumei_domain::{Effect, EvidenceForm, Statistic, Unit};
use topcoat::{
    Result,
    view::{Unescaped, View, ViewExt, class, component, view},
};

use crate::components::{
    claim_chip, code_block, evidence_ref, panel, reference_chips, reference_list, standing,
    state_chip,
};

#[component]
pub async fn situation(w: &Workspace) -> Result<impl View> {
    let s: &Situation = &w.situation;
    Ok(view! {
        <section class="situation" aria-label="Situation">
            <div class="situation-symptom">
                <div class="eyebrow">"Symptom"
                    if let Some(t) = &s.symptom_title {
                        " · " <span class="muted">(t.clone())</span>
                    }
                </div>
                <blockquote>(s.symptom.clone())</blockquote>
                if let Some(c) = &s.classification {
                    <div class="fine">"Recognised by " (c.clone())</div>
                }
                if !s.references.is_empty() {
                    <details class="refs-box">
                        <summary class="fine">"Runbooks for this symptom (" (s.references.len().to_string()) ")"</summary>
                        reference_list(refs: &s.references)
                    </details>
                }
            </div>
            <dl class="situation-facts">
                for (k, v) in &s.scope {
                    <div><dt>(k.clone())</dt><dd class=(class!("unknown" if v == "unknown"))>(v.clone())</dd></div>
                }
                <div><dt>"Incident window"</dt><dd class=(class!("unknown" if s.window.starts_with("unknown"))) title=(s.window_utc.clone())>(s.window.clone())</dd></div>
                for e in &s.environment {
                    <div class=(class!("span-2" if e.label == "Topology"))>
                        <dt>(e.label.clone())</dt>
                        <dd class=(class!("unknown" if e.status == "unknown"))>
                            (e.value.clone())
                            if e.status != "unknown" {
                                " " <span class=(class!("standing", format!("standing-{}", e.status)))>(e.status)</span>
                            }
                        </dd>
                    </div>
                }
                <div class="span-all">
                    <dt>"Knowledge"</dt>
                    <dd>
                        <a href=(if s.knowledge_pack.is_empty() { "/knowledge".to_owned() } else { format!("/knowledge?pack={}", s.knowledge_pack) })>(s.knowledge.clone())</a>
                        if !s.knowledge_verified {
                            " " <span class="flag">"unverified for this version"</span>
                        }
                    </dd>
                </div>
            </dl>
        </section>
    })
}

#[component]
pub async fn diagnosis(w: &Workspace) -> Result<impl View> {
    let Some(c) = w.assessment.conclusion.clone() else {
        return Ok(view! { "" }.boxed());
    };
    let id = w.id.clone();
    Ok(view! {
        <section class=(class!("panel", "diagnosis", "diagnosis-provisional" if c.provisional)) id="diagnosis">
            <header class="panel-head">
                <h2>if c.provisional { "Diagnosis — supportable" } else { "Diagnosis" }</h2>
                if c.provisional {
                    <span class="panel-note">"The evidence supports it; the path is being completed below."</span>
                }
            </header>
            <p class="diagnosis-title">(c.title.clone())</p>
            <p class="diagnosis-statement">(c.statement.clone())</p>
            if !c.recommendations.is_empty() {
                <h3>"What to do"</h3>
                <ul class="recs">
                    for r in &c.recommendations {
                        <li>
                            (r.text.clone())
                            if let Some(caution) = &r.caution {
                                <div class="caution">"Caution: " (caution.clone())</div>
                            }
                            if !r.claims.is_empty() {
                                <div class="claims">
                                    for cl in &r.claims {
                                        claim_chip(claim: cl)
                                    }
                                </div>
                            }
                        </li>
                    }
                </ul>
            }
            if !c.references.is_empty() {
                <h3>"Runbooks and playbooks"</h3>
                reference_list(refs: &c.references)
            }
            <p class="fine">"Rests on findings "
                for f in &c.findings {
                    <a class="ref" href=(("#", f.clone()))>(f.clone())</a> " "
                }
                "and evidence "
                for e in &c.evidence {
                    evidence_ref(investigation: &id, id: e) " "
                }
            </p>
        </section>
    }
    .boxed())
}

/// The request answered last, and by what: the box's previous question.
#[component]
pub async fn answered_line(w: &Workspace) -> Result<impl View> {
    Ok(view! {
        if let Some(a) = &w.last_answered {
            <p class="answered">
                <span class="answered-mark" aria-hidden="true">"✓"</span>
                " " <code>(a.request.clone())</code> " " (a.title.clone()) " — answered by "
                for e in &a.evidence { evidence_ref(investigation: &w.id, id: e) " " }
                "at " <time title=(a.at_utc.clone())>(a.at.clone()) " " (w.zone.label.clone())</time>
            </p>
        }
    })
}

#[component]
pub async fn next_evidence(
    w: &Workspace,
    error: Option<&str>,
    collector: Option<&str>,
) -> Result<impl View> {
    let Some(n) = w.next.clone() else {
        let concluded = w.status.token == "concluded";
        return Ok(view! {
            <section class="panel next next-none" id="next">
                <header class="panel-head"><h2>"Next evidence"</h2></header>
                answered_line(w: w)
                if concluded {
                    <p>"No further evidence is needed: the diagnosis is supported. You can still add evidence; Kyumei will reassess."</p>
                } else {
                    <p>"Kyumei has no further evidence request for the remaining hypotheses. Their uncertainty is listed under Current assessment."</p>
                }
                evidence_forms(w: w, n: None)
            </section>
        }
        .boxed());
    };
    Ok(view! {
        <section class=(class!("panel", "next", "next-optional" if n.optional)) id="next">
            <header class="panel-head">
                <h2>
                    if n.optional { "Optional evidence" }
                    else if w.assessment.conclusion.is_some() { "Next evidence — completes the diagnosis" }
                    else { "Next evidence required" }
                </h2>
                <span class="panel-note"><code>(n.request.clone())</code></span>
            </header>
            if let Some(e) = error {
                <p class="error" role="alert">(e)</p>
            }
            answered_line(w: w)
            <p class="next-title">(n.title.clone())</p>
            <div class="next-grid">
                <div>
                    <h3>"Why"</h3>
                    <p>(n.why.clone())</p>
                    <p class="fine">(n.rationale.clone())</p>
                </div>
                <dl class="next-meta">
                    <div><dt>"Window"</dt><dd>(n.window.clone())</dd></div>
                    if !n.group_by.is_empty() {
                        <div><dt>"Group by"</dt><dd><code>(n.group_by.join(", "))</code></dd></div>
                    }
                    <div><dt>"Distinguishes"</dt><dd>(n.distinguishes.join("; "))</dd></div>
                    <div><dt>"Informs"</dt><dd>(n.expects.join("; "))</dd></div>
                    if !n.received.is_empty() {
                        <div><dt>"Received"</dt><dd>
                            for r in &n.received { evidence_ref(investigation: &w.id, id: r) " " }
                            if !n.missing.is_empty() {
                                <span class="flag">"still needed: " (n.missing.join("; "))</span>
                            }
                        </dd></div>
                    }
                </dl>
            </div>
            <h3>"Collect"</h3>
            <ol class="collect">
                for (i, c) in n.collect.iter().enumerate() {
                    let cid = format!("collect-{i}");
                    let wid = format!("collect-{i}-witness");
                    <li>
                        <div class="collect-head">
                            <span class="method">(c.method.clone())</span>
                            (c.label.clone())
                            if let Some(a) = &c.applies_to {
                                " " <span class="tag">(a.clone())</span>
                            }
                            if let Some(u) = &c.unit {
                                " " <span class="tag">"unit: " (u.clone())</span>
                            }
                        </div>
                        code_block(id: &cid, body: &c.body)
                        if !c.unfilled.is_empty() {
                            <p class="flag">"Fill in: " (c.unfilled.join(", ")) " — Kyumei does not know these yet."</p>
                        }
                        if !c.claims.is_empty() {
                            <div class="claims">
                                for cl in &c.claims { claim_chip(claim: cl) }
                            </div>
                        }
                        if let Some((witness, because)) = &c.witness {
                            <p class="witness">"With it, read the count of all the same requests. When this query returns no series and that one has some, nothing was counted, and Kyumei reads this one as zero:"</p>
                            code_block(id: &wid, body: witness)
                            <div class="claims">claim_chip(claim: because)</div>
                        }
                    </li>
                }
            </ol>
            if let Some(why) = &n.collect_blocked {
                <p class="flag collect-blocked">"Collect now can't read these queries yet: " (why.clone()) ". "
                    <a href="#edit-context">"Give the incident window a start and an end under Edit context."</a></p>
            }
            if let Some(c) = &n.collector {
                read_queries(investigation: &w.id, request: &n.request, c: c, source: collector)
            }
            if !n.references.is_empty() {
                <details class="refs-box" open="">
                    <summary><h3 class="inline">"Where operators look"</h3></summary>
                    reference_list(refs: &n.references)
                </details>
            }
            evidence_forms(w: w, n: Some(&n))
        </section>
    }
    .boxed())
}

/// The request's queries read in one go: by Kyumei itself when it can reach
/// Prometheus or Grafana (Collect now), or by `kyumei-collect` wherever the
/// metrics can be read, which prints a block to paste back here.
#[component]
pub async fn read_queries(
    investigation: &str,
    request: &str,
    c: &CollectorView,
    source: Option<&str>,
) -> Result<impl View> {
    let queries = match c.queries {
        1 => "the query".to_owned(),
        n => format!("the {n} queries"),
    };
    let missing = c.missing.clone().unwrap_or_default();
    let names = c.unfilled.join(" and ");
    let them = if c.unfilled.len() == 1 { "it" } else { "them" };
    let action = format!("/investigations/{investigation}/requests/{request}/collect");
    Ok(view! {
        <div class="read-queries">
            if let Some(source) = source {
                if c.unfilled.is_empty() {
                    <form method="post" action=(action) class="collect-now">
                        <button type="submit" class="btn btn-primary">"Collect now"</button>
                        <span class="fine">"Reads " (queries.clone()) " from " (source.to_owned()) " over the window, every " (c.step.clone()) ", and adds the samples as evidence."</span>
                    </form>
                } else {
                    <p class="fine">"Collect now can read " (queries.clone()) " from " (source.to_owned()) " once Kyumei knows " (missing.clone()) ": add " (them) " under Edit context."</p>
                }
            }
            <p class="fine">
                if source.is_some() { "Or run" } else { "Run" }
                " this where your metrics can be read, and paste the block it prints. "
                <code>"kyumei-collect"</code> " reads Prometheus or Grafana and nothing else."
            </p>
            code_block(id: "collect-command", body: &c.command)
            if !c.unfilled.is_empty() {
                <p class="flag">"Put the " (names.clone()) " after " if c.unfilled.len() == 1 { "the" } else { "each" } " " <code>"="</code> "."</p>
            }
        </div>
    })
}

/// The ways to add evidence: paste, upload, typed values, an observation;
/// or say the evidence cannot be obtained.
#[component]
pub async fn evidence_forms(w: &Workspace, n: Option<&NextEvidence>) -> Result<impl View> {
    let id = w.id.clone();
    let request = n.map(|n| n.request.clone()).unwrap_or_default();
    let query = n
        .and_then(|n| n.collect.first())
        .map(|c| c.body.clone())
        .unwrap_or_default();
    let expected: Vec<String> = n.map(|n| n.expects.clone()).unwrap_or_default();
    let action = format!("/investigations/{id}/evidence");
    let upload = format!("/investigations/{id}/evidence/upload");
    let units = [
        Unit::Seconds,
        Unit::Milliseconds,
        Unit::Count,
        Unit::PerSecond,
        Unit::Ratio,
    ];
    Ok(view! {
        <div class="add-evidence">
            <h3>"Add evidence"</h3>
            <div class="tabs" role="tablist">
                <details class="tab" open="" name="add">
                    <summary>"Paste"</summary>
                    <form method="post" action=(action.clone()) class="evidence-form">
                        <input type="hidden" name="mode" value="paste">
                        <input type="hidden" name="request" value=(request.clone())>
                        <label>"Output, export or text"
                            <textarea name="text" rows="9" required="" spellcheck="false"
                                placeholder="Prometheus JSON (/api/v1/query_range), exposition text, CSV, timestamp-value lines, temporal CLI or grpcurl JSON, dynamic config YAML, logs…"></textarea>
                        </label>
                        <div class="row">
                            <label class="grow">"Query or command used"
                                <input type="text" name="query" value=(query.clone()) spellcheck="false">
                            </label>
                        </div>
                        evidence_context_fields(zone: &w.zone.label)
                        <div class="actions">
                            <button type="submit" class="btn btn-primary">"Add evidence"</button>
                        </div>
                    </form>
                </details>
                <details class="tab" name="add">
                    <summary>"Upload"</summary>
                    <form method="post" action=(upload) enctype="multipart/form-data" class="evidence-form">
                        <input type="hidden" name="request" value=(request.clone())>
                        <label>"File (exports, logs, configuration, screenshots)"
                            <input type="file" name="file" required="">
                        </label>
                        <label>"Query or command used"
                            <input type="text" name="query" value=(query.clone()) spellcheck="false">
                        </label>
                        evidence_context_fields(zone: &w.zone.label)
                        <div class="actions">
                            <button type="submit" class="btn btn-primary">"Upload evidence"</button>
                        </div>
                    </form>
                </details>
                <details class="tab" name="add">
                    <summary>"Enter values"</summary>
                    <form method="post" action=(action.clone()) class="evidence-form">
                        <input type="hidden" name="mode" value="structured">
                        <input type="hidden" name="request" value=(request.clone())>
                        <p class="fine">"Values you type are recorded as your assertion, not as observed telemetry."</p>
                        <div class="row">
                            <label class="grow">"Signal"
                                <select name="signal" required="">
                                    for (sid, title) in &w.signals {
                                        <option value=(sid.clone()) selected=(expected.contains(title))>(title.clone())</option>
                                    }
                                </select>
                            </label>
                            <label>"Statistic"
                                <select name="statistic">
                                    for s in [Statistic::Min, Statistic::Max, Statistic::Mean, Statistic::Last, Statistic::Typical] {
                                        <option value=(s.label())>(s.label())</option>
                                    }
                                </select>
                            </label>
                            <label>"Value"<input type="text" name="value" required="" inputmode="decimal"></label>
                            <label>"Unit"
                                <select name="unit">
                                    <option value="">"signal's unit"</option>
                                    for u in units {
                                        <option value=(u.to_string())>(u.to_string())</option>
                                    }
                                </select>
                            </label>
                        </div>
                        <div class="row">
                            <label>"From (" (w.zone.label.clone()) ")"<input type="datetime-local" name="window_start"></label>
                            <label>"To (" (w.zone.label.clone()) ")"<input type="datetime-local" name="window_end"></label>
                            <label class="grow">"Where it came from"<input type="text" name="note" placeholder="e.g. Grafana panel ‘WFT slots’"></label>
                        </div>
                        <div class="actions"><button type="submit" class="btn btn-primary">"Add values"</button></div>
                    </form>
                </details>
                <details class="tab" name="add">
                    <summary>"Observation"</summary>
                    <form method="post" action=(action.clone()) class="evidence-form">
                        <input type="hidden" name="mode" value="observation">
                        <input type="hidden" name="request" value=(request.clone())>
                        <label>"What you observed"
                            <textarea name="text" rows="4" required="" placeholder="e.g. Grafana shows the backlog flat at ~60k from 13:40"></textarea>
                        </label>
                        <div class="actions"><button type="submit" class="btn btn-primary">"Record observation"</button></div>
                    </form>
                </details>
                if n.is_some() {
                    <details class="tab tab-decline" name="add">
                        <summary>"Can’t get this"</summary>
                        <form method="post" action=(format!("/investigations/{id}/requests/{request}/decline")) class="evidence-form">
                            <label>"Why not?"
                                <input type="text" name="reason" required="" placeholder="e.g. no access to server metrics">
                            </label>
                            <p class="fine">"Kyumei keeps the affected hypotheses open and looks for other evidence."</p>
                            <div class="actions"><button type="submit" class="btn btn-secondary">"Mark unavailable"</button></div>
                        </form>
                    </details>
                }
            </div>
        </div>
    })
}

#[component]
pub async fn evidence_context_fields(zone: &str) -> Result<impl View> {
    let forms = EvidenceForm::ALL;
    let units = [
        Unit::Seconds,
        Unit::Milliseconds,
        Unit::Count,
        Unit::PerSecond,
        Unit::Ratio,
    ];
    Ok(view! {
        <details class="more">
            <summary>"Describe it (optional)"</summary>
            <div class="row">
                <label>"Form"
                    <select name="form">
                        <option value="">"detect"</option>
                        for f in forms {
                            <option value=(f.token())>(f.label())</option>
                        }
                    </select>
                </label>
                <label>"Unit of values"
                    <select name="unit">
                        <option value="">"detect"</option>
                        for u in units {
                            <option value=(u.to_string())>(u.to_string())</option>
                        }
                    </select>
                </label>
                <label>"Captured from (" (zone.to_owned()) ")"<input type="datetime-local" name="window_start"></label>
                <label>"to (" (zone.to_owned()) ")"<input type="datetime-local" name="window_end"></label>
            </div>
            <label>"Note"<input type="text" name="note"></label>
        </details>
    })
}

#[component]
pub async fn assessment(w: &Workspace) -> Result<impl View> {
    let a = &w.assessment;
    let id = w.id.clone();
    Ok(view! {
        panel(title: "Current assessment", id: Some("assessment"), note: Some(w.explanation.status.as_str()),
            <p class="summary">(a.summary.clone())</p>
            <div class="assess-grid">
                <div>
                    <h3>"What Kyumei knows"</h3>
                    if a.knows.is_empty() {
                        <p class="muted">"Nothing measured yet."</p>
                    }
                    <table class="facts">
                        <tbody>
                            for f in &a.knows {
                                <tr>
                                    <th scope="row">(f.title.clone())</th>
                                    <td class="num">(f.summary.clone())</td>
                                    <td>standing(status: f.status)</td>
                                    <td class="refs">for e in &f.evidence { evidence_ref(investigation: &id, id: e) " " }</td>
                                </tr>
                            }
                        </tbody>
                    </table>
                </div>
                <div>
                    hypothesis_list(title: "Leading", rows: &a.leading, empty: "No hypothesis is supported yet.")
                    hypothesis_list(title: "Open", rows: &a.open, empty: "None open.")
                    hypothesis_list(title: "Ruled out", rows: &a.eliminated, empty: "None ruled out yet.")
                </div>
            </div>
            if !a.uncertainties.is_empty() {
                <details class="uncertainty" open=(a.conclusion.is_none())>
                    <summary>"Unresolved uncertainty (" (a.uncertainties.len()) ")"</summary>
                    <ul>
                        for u in &a.uncertainties {
                            <li><span class="tag">(u.label)</span> " " (u.text.clone())</li>
                        }
                    </ul>
                </details>
            }
        )
    })
}

#[component]
pub async fn hypothesis_list(
    title: &str,
    rows: &[HypothesisRow],
    empty: &str,
) -> Result<impl View> {
    Ok(view! {
        <h3>(title)</h3>
        if rows.is_empty() {
            <p class="muted fine">(empty)</p>
        } else {
            <ul class="hyps">
                for h in rows {
                    <li>
                        state_chip(state: h.state, contested: h.contested)
                        " " <span class="hyp-title" title=(h.statement.clone())>(h.title.clone())</span>
                        " " <span class="fine muted">(h.stage.clone())</span>
                        if !h.findings.is_empty() {
                            <span class="fine"> " — "
                                for f in &h.findings { <a class="ref" href=(("#", f.clone()))>(f.clone())</a> " " }
                            </span>
                        }
                        if !h.references.is_empty() {
                            <span class="fine">reference_chips(refs: &h.references)</span>
                        }
                    </li>
                }
            </ul>
        }
    })
}

#[component]
pub async fn findings(w: &Workspace) -> Result<impl View> {
    let id = w.id.clone();
    Ok(view! {
        panel(title: "Findings", id: Some("findings"), note: Some("Each finding shows the evidence behind it"),
            if w.findings.is_empty() {
                <p class="muted">"No rule has fired yet. Findings appear as evidence arrives."</p>
            }
            <ol class="findings">
                for f in &w.findings {
                    finding(f: f, investigation: &id)
                }
            </ol>
        )
    })
}

#[component]
pub async fn finding(f: &FindingView, investigation: &str) -> Result<impl View> {
    let effect = match f.effect {
        Effect::Supports => "supports",
        Effect::Weakens => "weakens",
        Effect::Eliminates => "eliminates",
        Effect::Establishes => "establishes",
    };
    Ok(view! {
        <li class=(class!("finding", format!("effect-{effect}"))) id=(f.id.clone())>
            <div class="finding-head">
                <span class="effect">(effect)</span>
                " " <span class="hyp-title">(f.hypothesis.clone())</span>
                " " <code class="fine muted">(f.id.clone())</code>
                if f.rests_on_assertion {
                    " " <span class="flag">"rests partly on an operator assertion"</span>
                }
                if f.rests_on_simulation {
                    " " <span class="tag">"rests on a simulation"</span>
                }
            </div>
            <p>(f.statement.clone())</p>
            <details class="evidence-affordance">
                <summary>"Evidence"</summary>
                <table class="basis">
                    <thead><tr><th>"Signal"</th><th>"Statistic"</th><th>"Value"</th><th>"Standing"</th><th>"Source"</th></tr></thead>
                    <tbody>
                        for b in &f.basis {
                            <tr>
                                <td>(b.signal.clone())</td>
                                <td>(b.statistic.clone())</td>
                                <td class="num">(b.value.clone())</td>
                                <td>standing(status: b.status)</td>
                                <td>
                                    for o in &b.observations { evidence_ref(investigation: investigation, id: o) " " }
                                    <div class="fine muted">(b.sources.join("; "))</div>
                                </td>
                            </tr>
                        }
                    </tbody>
                </table>
                <p class="fine">"Rule " <code>(f.rule.clone())</code> " fired because " <code>(f.condition.clone())</code></p>
                if !f.claims.is_empty() {
                    <div class="claims">"Relies on: " for c in &f.claims { claim_chip(claim: c) }</div>
                }
            </details>
        </li>
    })
}

#[component]
pub async fn diagrams(w: &Workspace) -> Result<impl View> {
    Ok(view! {
        panel(title: "Diagrams", id: Some("diagrams"), note: Some("Derived from the same assessment as the text"),
            <div class="diagrams">
                for d in &w.diagrams {
                    diagram(d: d)
                }
            </div>
        )
    })
}

#[component]
pub async fn diagram(d: &DiagramView) -> Result<impl View> {
    Ok(view! {
        <figure class=(class!("diagram", format!("diagram-{}", d.kind.token())))>
            <figcaption>(d.title.clone())</figcaption>
            <div class="diagram-svg">(Unescaped::new_unchecked(d.svg.clone()))</div>
            <details class="diagram-text">
                <summary>"Text version"</summary>
                <pre>(d.text.clone())</pre>
            </details>
        </figure>
    })
}

#[component]
pub async fn trail(rows: &[TrailRow], investigation: &str, zone: &str) -> Result<impl View> {
    let note = format!("times in {zone}");
    Ok(view! {
        panel(title: "Investigation trail", id: Some("trail"), note: Some(note.as_str()),
            <ol class="trail">
                for t in rows {
                    <li class=(class!("trail-row", format!("trail-{}", t.kind)))>
                        <time title=(t.utc.clone())>(t.time.clone())</time>
                        <span class="trail-text">(t.text.clone())</span>
                        <span class="trail-refs">
                            for r in &t.refs {
                                if r.starts_with('E') && r.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) {
                                    evidence_ref(investigation: investigation, id: r) " "
                                } else if r.starts_with("F-") {
                                    <a class="ref" href=(("#", r.clone()))>(r.clone())</a> " "
                                } else {
                                    <span class="ref-plain">(r.clone())</span> " "
                                }
                            }
                        </span>
                    </li>
                }
            </ol>
        )
    })
}

#[component]
pub async fn evidence_list(w: &Workspace) -> Result<impl View> {
    let id = w.id.clone();
    Ok(view! {
        panel(title: "Evidence", id: Some("evidence"),
            <table class="evidence-table">
                <tbody>
                    for e in &w.evidence {
                        <tr>
                            <td>evidence_ref(investigation: &id, id: &e.id)</td>
                            <td>
                                <div>(e.form.clone()) if let Some(r) = &e.responds_to { <span class="fine muted">" → " (r.clone())</span> }</div>
                                <div class="fine muted"><time title=(e.submitted_utc.clone())>(e.submitted.clone())</time> " · " (e.origin.clone())</div>
                            </td>
                            <td class="fine">
                                if e.bound.is_empty() { <span class="muted">"informs no signal"</span> } else { (e.bound.join(", ")) }
                                if e.cautions > 0 { " " <span class="flag">(e.cautions) " to check"</span> }
                                if e.corrected { " " <span class="tag">"corrected"</span> }
                            </td>
                        </tr>
                    }
                </tbody>
            </table>
        )
    })
}

#[component]
pub async fn requests(w: &Workspace) -> Result<impl View> {
    Ok(view! {
        panel(title: "Requests", id: Some("requests"),
            <table class="requests-table">
                <tbody>
                    for r in &w.requests {
                        <tr>
                            <td><code>(r.id.clone())</code></td>
                            <td>(r.title.clone())<div class="fine muted">"issued " <time title=(r.issued_utc.clone())>(r.issued.clone())</time></div></td>
                            <td class="fine">(r.status.clone())</td>
                        </tr>
                    }
                </tbody>
            </table>
        )
    })
}
