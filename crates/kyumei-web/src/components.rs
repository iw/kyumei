//! Kyumei's interface components.
//!
//! These follow Topcoat UI's conventions (`#[default] mut attrs: Attributes`
//! forwarding, `class!` composition, variants as enums) but are Kyumei's own
//! code with Kyumei's own stylesheet. The upstream registry components assume
//! Tailwind and the Topcoat browser runtime, which this application does not
//! use, so they are re-expressed here rather than installed with the CLI.

use kyumei_domain::{EpistemicStatus, HypothesisState};
use topcoat::{
    Result,
    view::{Attributes, Child, View, class, component, view},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    #[default]
    Primary,
    Secondary,
    Quiet,
}

impl ButtonVariant {
    fn class(self) -> &'static str {
        match self {
            Self::Primary => "btn btn-primary",
            Self::Secondary => "btn btn-secondary",
            Self::Quiet => "btn btn-quiet",
        }
    }
}

#[component]
pub async fn button(
    #[default] variant: ButtonVariant,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <button class=(class!(variant.class(), attrs.remove("class"))) (attrs)>(child)</button>
    })
}

/// A titled section of the workspace.
#[component]
pub async fn panel(
    title: &str,
    #[default] id: Option<&str>,
    #[default] note: Option<&str>,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <section class=(class!("panel", attrs.remove("class"))) id=(id) (attrs)>
            <header class="panel-head">
                <h2>(title)</h2>
                if let Some(note) = note {
                    <span class="panel-note">(note)</span>
                }
            </header>
            (child)
        </section>
    })
}

/// A hypothesis state: glyph and word, never colour alone.
#[component]
pub async fn state_chip(state: HypothesisState, #[default] contested: bool) -> Result<impl View> {
    let label = if contested && !state.is_resolved() {
        "contested".to_owned()
    } else if contested {
        format!("{} · contested", state.label())
    } else {
        state.label().to_owned()
    };
    Ok(view! {
        <span class=(class!("chip", format!("chip-{}", state.label()))) title=(state_help(state))>
            <span class="chip-glyph" aria-hidden="true">(state.glyph())</span>
            (label)
        </span>
    })
}

fn state_help(state: HypothesisState) -> &'static str {
    match state {
        HypothesisState::Candidate => "Under consideration; no evidence has moved it yet",
        HypothesisState::Supported => "Evidence points toward it; not yet established",
        HypothesisState::Weakened => "Evidence points away from it; not ruled out",
        HypothesisState::Eliminated => "Evidence rules it out",
        HypothesisState::Established => "Evidence establishes it",
    }
}

/// The epistemic standing of a statement.
#[component]
pub async fn standing(status: EpistemicStatus) -> Result<impl View> {
    Ok(view! {
        <span class=(class!("standing", format!("standing-{}", status.token()))) title=(status.label())>
            (status.token())
        </span>
    })
}

/// A reference to an evidence record.
#[component]
pub async fn evidence_ref(investigation: &str, id: &str) -> Result<impl View> {
    let (evidence, observation) = match id.split_once('.') {
        Some((e, _)) => (e.to_owned(), Some(id.to_owned())),
        None => (id.to_owned(), None),
    };
    let href = match &observation {
        Some(o) => format!("/investigations/{investigation}/evidence/{evidence}#{o}"),
        None => format!("/investigations/{investigation}/evidence/{evidence}"),
    };
    Ok(view! {
        <a class="ref" href=(href)>(id)</a>
    })
}

/// A knowledge claim with its verification standing.
#[component]
pub async fn claim_chip(claim: &kyumei_app::workspace::ClaimView) -> Result<impl View> {
    let mark = if claim.verified {
        "verified"
    } else {
        "unverified"
    };
    Ok(view! {
        <a class=(class!("claim", "claim-unverified" if !claim.verified)) href=(format!("/knowledge/{}?pack={}", claim.id, claim.pack))
            title=(format!("{} — {}", claim.category, claim.applicability))>
            <code>(claim.name.clone())</code>
            <span class="claim-meta">(claim.category.clone()) " · " (mark)</span>
        </a>
    })
}

/// Copyable code block.
#[component]
pub async fn code_block(id: &str, body: &str) -> Result<impl View> {
    Ok(view! {
        <div class="code">
            <pre id=(id)><code>(body)</code></pre>
            <button type="button" class="btn btn-quiet btn-copy" data-copy=(id)>"Copy"</button>
        </div>
    })
}

/// Runbooks, playbook sections and dashboard panels, each linked at its source's
/// pinned commit, with why it bears on what it is listed under.
#[component]
pub async fn reference_list(refs: &[kyumei_app::dashboards::ReferenceView]) -> Result<impl View> {
    Ok(view! {
        <ul class="refs">
            for r in refs {
                <li class="ref-item">
                    <span class="ref-kind">(r.label.clone())</span> " "
                    match (&r.panel, &r.url) {
                        (Some(p), _) => { <a href=(p.open_url.clone())>(r.title.clone())</a> },
                        (None, Some(url)) => { <a href=(url.clone()) rel="noreferrer">(r.title.clone())</a> },
                        (None, None) => { (r.title.clone()) },
                    }
                    if let (None, Some(v)) = (&r.panel, &r.version) {
                        <span class="fine muted">" · " (v.clone())</span>
                    }
                    if let Some(p) = &r.panel {
                        <span class="fine muted">" · panel " (p.id.to_string()) " of " (p.dashboard_title.clone())</span>
                        " " <a class="fine open-panel" href=(p.open_url.clone())>"open with live data"</a>
                        if let Some(g) = &p.grafana_url {
                            " · " <a class="fine" href=(g.clone()) rel="noreferrer">"in Grafana"</a>
                        }
                    }
                    <div class="fine ref-why">(r.why.clone()) " "
                        match (&r.panel, &r.url) {
                            (Some(_), Some(url)) => { <a class="muted ref-cite" href=(url.clone()) rel="noreferrer">(r.cited.clone())</a> },
                            _ => { <span class="muted ref-cite">(r.cited.clone())</span> },
                        }
                    </div>
                </li>
            }
        </ul>
    })
}

/// A hypothesis's runbooks and playbook sections, as compact links.
#[component]
pub async fn reference_chips(refs: &[kyumei_app::dashboards::ReferenceView]) -> Result<impl View> {
    Ok(view! {
        for r in refs {
            " "
            if let Some(url) = &r.url {
                <a class="ref-chip" href=(url.clone()) title=(match &r.version { Some(v) => format!("{} ({v}; {})", r.why, r.cited), None => format!("{} ({})", r.why, r.cited) }) rel="noreferrer">(r.label.clone()) ": " (r.title.clone())</a>
            }
        }
    })
}

/// Text with `backticked` names set as code.
#[component]
pub async fn inline_code(text: &str) -> Result<impl View> {
    let parts: Vec<(bool, String)> = text
        .split('`')
        .enumerate()
        .map(|(i, part)| (i % 2 == 1, part.to_owned()))
        .collect();
    Ok(view! {
        for (code, part) in &parts {
            if *code { <code>(part.clone())</code> } else { (part.clone()) }
        }
    })
}
