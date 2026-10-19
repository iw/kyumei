//! Deterministic layout and SVG rendering.
//!
//! Layouts are simple and fixed: vertical paths and two-column relationship
//! graphs. Meaning is carried by glyphs, words and line styles; colour only
//! reinforces it. Every SVG has a `<title>` and `<desc>` and is safe to embed
//! (all text is XML-escaped).

use std::{collections::BTreeMap, fmt::Write as _};

use crate::ir::{Diagram, DiagramKind, Edge, Node, NodeId, NodeRole, NodeStatus, Relation};

const FONT: f64 = 13.0;
const SMALL: f64 = 11.0;
const LINE: f64 = 17.0;
const CHAR_W: f64 = 7.1;
const PAD: f64 = 10.0;

/// Render a validated diagram as a standalone SVG document fragment.
pub fn render_svg(d: &Diagram) -> String {
    match d.kind {
        DiagramKind::DiagnosticPath => vertical(d, 560.0, 480.0, false),
        DiagramKind::SystemPath => vertical(d, 640.0, 200.0, true),
        DiagramKind::EvidenceRelationship => relationships(d),
    }
}

struct Canvas {
    body: String,
    id: String,
}

impl Canvas {
    fn new(d: &Diagram) -> Self {
        Self {
            body: String::new(),
            id: sanitize_id(&d.id),
        }
    }

    fn finish(self, d: &Diagram, width: f64, height: f64) -> String {
        let title_id = format!("{}-title", self.id);
        let desc_id = format!("{}-desc", self.id);
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" class="kd kd-{kind}" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" role="img" aria-labelledby="{title_id} {desc_id}"><title id="{title_id}">{title}</title><desc id="{desc_id}">{desc}</desc>{style}<defs><marker id="{id}-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="8" markerHeight="8" markerUnits="userSpaceOnUse" orient="auto-start-reverse"><path d="M0 0 L8 4 L0 8 z" class="kd-arrowhead"/></marker></defs>{body}</svg>"#,
            kind = d.kind.token(),
            w = width,
            h = height,
            title = esc(&d.title),
            desc = esc(&d.description),
            style = STYLE,
            id = self.id,
            body = self.body,
        )
    }

    fn node(
        &mut self,
        n: &Node,
        x: f64,
        y: f64,
        w: f64,
        lines: &[String],
        detail: &[String],
    ) -> f64 {
        let h = box_height(lines, detail);
        let class = format!(
            "kd-node kd-role-{} kd-status-{}",
            role_token(n.role),
            n.status.token()
        );
        let _ = write!(
            self.body,
            r#"<g class="{class}" data-node="{nid}">"#,
            nid = esc(&n.id.0)
        );
        let (dash, stroke) = status_stroke(n.status);
        let _ = write!(
            self.body,
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="4" class="kd-box" stroke-width="{stroke}"{dash}/>"#,
            dash = dash
                .map(|d| format!(r#" stroke-dasharray="{d}""#))
                .unwrap_or_default()
        );
        if n.status == NodeStatus::Established || n.status == NodeStatus::Suspected {
            // A second, inset outline: established/suspected read without colour.
            let _ = write!(
                self.body,
                r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="3" class="kd-box-inner"/>"#,
                x + 3.0,
                y + 3.0,
                w - 6.0,
                h - 6.0
            );
        }
        let glyph = n.status.glyph();
        let text_x = if glyph.is_empty() {
            x + PAD
        } else {
            x + PAD + 16.0
        };
        if !glyph.is_empty() {
            let _ = write!(
                self.body,
                r#"<text x="{:.1}" y="{:.1}" class="kd-glyph" aria-hidden="true">{}</text>"#,
                x + PAD,
                y + PAD + FONT,
                esc(glyph)
            );
        }
        let mut ty = y + PAD + FONT;
        for line in lines {
            let _ = write!(
                self.body,
                r#"<text x="{text_x:.1}" y="{ty:.1}" class="kd-label">{}</text>"#,
                esc(line)
            );
            ty += LINE;
        }
        for line in detail {
            let _ = write!(
                self.body,
                r#"<text x="{text_x:.1}" y="{:.1}" class="kd-detail">{}</text>"#,
                ty - 2.0,
                esc(line)
            );
            ty += SMALL + 4.0;
        }
        self.body.push_str("</g>");
        h
    }

    fn edge_path(&mut self, path: &str, relation: Relation, label: Option<(&str, f64, f64, &str)>) {
        let (dash, width) = relation_stroke(relation);
        let _ = write!(
            self.body,
            r#"<path d="{path}" class="kd-edge kd-rel-{rel}" stroke-width="{width}"{dash} marker-end="url(#{id}-arrow)"/>"#,
            rel = relation_token(relation),
            dash = dash
                .map(|d| format!(r#" stroke-dasharray="{d}""#))
                .unwrap_or_default(),
            id = self.id,
        );
        if let Some((text, x, y, anchor)) = label {
            let _ = write!(
                self.body,
                r#"<text x="{x:.1}" y="{y:.1}" text-anchor="{anchor}" class="kd-edge-label kd-rel-{rel}">{}</text>"#,
                esc(text),
                rel = relation_token(relation),
            );
        }
    }

    fn annotation(&mut self, x1: f64, y: f64, x2: f64, text: &str, max_chars: usize) {
        let lines = wrap(text, max_chars);
        let top = y + 4.0 - (lines.len() as f64 - 1.0) * 7.0;
        let _ = write!(
            self.body,
            r#"<g class="kd-annotation"><line x1="{x1:.1}" y1="{y:.1}" x2="{x2:.1}" y2="{y:.1}" class="kd-annotation-line"/><text x="{:.1}" y="{top:.1}" class="kd-annotation-text">"#,
            x2 + 6.0,
        );
        for (i, line) in lines.iter().enumerate() {
            let prefix = if i == 0 { "← " } else { "  " };
            let _ = write!(
                self.body,
                r#"<tspan x="{:.1}" dy="{}">{prefix}{}</tspan>"#,
                x2 + 6.0,
                if i == 0 { 0.0 } else { 14.0 },
                esc(line)
            );
        }
        self.body.push_str("</text></g>");
    }
}

/// Vertical chain: diagnostic and system paths.
fn vertical(d: &Diagram, width: f64, box_w: f64, annotate: bool) -> String {
    let mut c = Canvas::new(d);
    let order = chain_order(d);
    let x = if annotate {
        16.0
    } else {
        (width - box_w) / 2.0
    };
    let mut y = 16.0;
    let mut placed: BTreeMap<&NodeId, (f64, f64)> = BTreeMap::new();
    let incoming: BTreeMap<&NodeId, &Edge> = d.edges.iter().map(|e| (&e.to, e)).collect();
    let chars = ((box_w - 2.0 * PAD - 16.0) / CHAR_W) as usize;
    for (i, node) in order.iter().enumerate() {
        if i > 0 {
            let top = y;
            y += 44.0;
            let cx = x + box_w / 2.0;
            let label = incoming.get(&node.id).and_then(|e| e.label.clone());
            let relation = incoming
                .get(&node.id)
                .map_or(Relation::LeadsTo, |e| e.relation);
            let label_ref = label
                .as_deref()
                .map(|l| (l, cx + 10.0, top + 26.0, "start"));
            c.edge_path(
                &format!("M{cx:.1} {top:.1} L{cx:.1} {:.1}", y - 2.0),
                relation,
                label_ref,
            );
        }
        let lines = wrap(&node.label, chars);
        let detail = node
            .detail
            .as_deref()
            .map(|t| wrap(t, chars + 4))
            .unwrap_or_default();
        let h = c.node(node, x, y, box_w, &lines, &detail);
        placed.insert(&node.id, (y, h));
        y += h;
    }
    if annotate {
        let x2 = x + box_w + 30.0;
        let chars = ((width - x2 - 16.0) / 6.6) as usize;
        for a in &d.annotations {
            if let Some((ay, ah)) = placed.get(&a.target) {
                c.annotation(x + box_w + 4.0, ay + ah / 2.0, x2, &a.text, chars);
            }
        }
    }
    c.finish(d, width, y + 16.0)
}

/// Two columns: observations on the left, hypotheses on the right.
fn relationships(d: &Diagram) -> String {
    let mut c = Canvas::new(d);
    let width = 760.0;
    let col_w = 270.0;
    let left_x = 16.0;
    let right_x = width - col_w - 16.0;
    let chars = ((col_w - 2.0 * PAD - 16.0) / CHAR_W) as usize;

    let hypotheses: Vec<&Node> = d
        .nodes
        .iter()
        .filter(|n| n.role == NodeRole::Hypothesis)
        .collect();
    let mut observations: Vec<&Node> = d
        .nodes
        .iter()
        .filter(|n| n.role != NodeRole::Hypothesis)
        .collect();
    // Order observations by the first hypothesis they bear on, keeping lines short.
    let rank = |n: &Node| {
        d.edges
            .iter()
            .filter(|e| e.from == n.id)
            .filter_map(|e| hypotheses.iter().position(|h| h.id == e.to))
            .min()
            .unwrap_or(usize::MAX)
    };
    observations.sort_by_key(|n| rank(n));

    let mut positions: BTreeMap<&NodeId, (f64, f64, f64)> = BTreeMap::new();
    let mut ly = 44.0;
    for n in &observations {
        let lines = wrap(&n.label, chars);
        let detail = n
            .detail
            .as_deref()
            .map(|t| wrap(t, chars + 4))
            .unwrap_or_default();
        let h = c.node(n, left_x, ly, col_w, &lines, &detail);
        positions.insert(&n.id, (left_x + col_w, ly + h / 2.0, h));
        ly += h + 14.0;
    }
    // Spread hypotheses over the height of the observations column.
    let heights: Vec<f64> = hypotheses
        .iter()
        .map(|n| {
            let lines = wrap(&n.label, chars);
            let detail = n
                .detail
                .as_deref()
                .map(|t| wrap(t, chars + 4))
                .unwrap_or_default();
            box_height(&lines, &detail)
        })
        .collect();
    let total: f64 = heights.iter().sum();
    let gap = if hypotheses.len() > 1 {
        ((ly - 44.0 - total) / (hypotheses.len() as f64 - 1.0)).clamp(14.0, 60.0)
    } else {
        14.0
    };
    let mut ry = 44.0;
    for n in &hypotheses {
        let lines = wrap(&n.label, chars);
        let detail = n
            .detail
            .as_deref()
            .map(|t| wrap(t, chars + 4))
            .unwrap_or_default();
        let h = c.node(n, right_x, ry, col_w, &lines, &detail);
        positions.insert(&n.id, (right_x, ry + h / 2.0, h));
        ry += h + gap;
    }
    let _ = write!(
        c.body,
        r#"<text x="{left_x:.1}" y="34" class="kd-heading">Observations</text><text x="{right_x:.1}" y="34" class="kd-heading">Hypotheses</text>"#
    );
    // Line style carries the relation (see the legend); the text version
    // and each finding's evidence spell it out.
    let mut per_target: BTreeMap<&NodeId, usize> = BTreeMap::new();
    for e in &d.edges {
        let (Some(&(x1, y1, _)), Some(&(x2, y2, h2))) =
            (positions.get(&e.from), positions.get(&e.to))
        else {
            continue;
        };
        let k = per_target.entry(&e.to).or_default();
        let offset = (*k as f64 - 1.5) * 5.0;
        *k += 1;
        let ty = (y2 + offset).clamp(y2 - h2 / 2.0 + 6.0, y2 + h2 / 2.0 - 6.0);
        let mid = (x1 + x2) / 2.0;
        let path = format!(
            "M{x1:.1} {y1:.1} C{mid:.1} {y1:.1} {mid:.1} {ty:.1} {:.1} {ty:.1}",
            x2 - 2.0
        );
        c.edge_path(&path, e.relation, None);
    }
    legend(&mut c, left_x + col_w + 24.0, 12.0);
    let height = ly.max(ry) + 8.0;
    c.finish(d, width, height)
}

/// A legend for relation line styles, so the diagram reads without colour.
fn legend(c: &mut Canvas, x: f64, y: f64) {
    let items = [
        (Relation::Establishes, "establishes"),
        (Relation::Supports, "supports"),
        (Relation::Weakens, "weakens"),
        (Relation::Eliminates, "eliminates"),
    ];
    let mut lx = x;
    for (relation, word) in items {
        let (dash, width) = relation_stroke(relation);
        let _ = write!(
            c.body,
            r#"<line x1="{lx:.1}" y1="{y:.1}" x2="{:.1}" y2="{y:.1}" class="kd-edge kd-rel-{}" stroke-width="{width}"{}/><text x="{:.1}" y="{:.1}" class="kd-detail">{word}</text>"#,
            lx + 22.0,
            relation_token(relation),
            dash.map(|d| format!(r#" stroke-dasharray="{d}""#))
                .unwrap_or_default(),
            lx + 26.0,
            y + 4.0,
        );
        lx += 34.0 + word.len() as f64 * 6.2;
    }
}

/// Nodes in path order: from the root along `leads_to`/`flows_to` edges,
/// then any unreached nodes in declaration order.
fn chain_order(d: &Diagram) -> Vec<&Node> {
    let targets: std::collections::BTreeSet<&NodeId> = d.edges.iter().map(|e| &e.to).collect();
    let mut order: Vec<&Node> = Vec::new();
    let mut current = d.nodes.iter().find(|n| !targets.contains(&n.id));
    while let Some(node) = current {
        if order.iter().any(|n| n.id == node.id) {
            break;
        }
        order.push(node);
        current = d
            .edges
            .iter()
            .find(|e| e.from == node.id)
            .and_then(|e| d.node(&e.to));
    }
    for n in &d.nodes {
        if !order.iter().any(|o| o.id == n.id) {
            order.push(n);
        }
    }
    order
}

fn box_height(lines: &[String], detail: &[String]) -> f64 {
    2.0 * PAD + lines.len() as f64 * LINE + detail.len() as f64 * (SMALL + 4.0) - 2.0
}

fn status_stroke(status: NodeStatus) -> (Option<&'static str>, f64) {
    match status {
        NodeStatus::Open => (Some("6 4"), 1.2),
        NodeStatus::Weakened => (Some("2 3"), 1.2),
        NodeStatus::Eliminated => (Some("1 3"), 1.0),
        NodeStatus::Asserted => (Some("5 3"), 1.2),
        NodeStatus::Simulated => (Some("2 2"), 1.2),
        NodeStatus::Established | NodeStatus::Suspected => (None, 2.0),
        NodeStatus::Supported => (None, 1.6),
        _ => (None, 1.2),
    }
}

fn relation_stroke(relation: Relation) -> (Option<&'static str>, f64) {
    match relation {
        Relation::Weakens => (Some("5 4"), 1.3),
        Relation::Eliminates => (Some("1 3"), 1.6),
        Relation::Establishes => (None, 2.4),
        Relation::Supports => (None, 1.4),
        Relation::LeadsTo | Relation::FlowsTo => (None, 1.3),
    }
}

fn relation_token(relation: Relation) -> &'static str {
    match relation {
        Relation::LeadsTo => "leads-to",
        Relation::FlowsTo => "flows-to",
        Relation::Supports => "supports",
        Relation::Weakens => "weakens",
        Relation::Eliminates => "eliminates",
        Relation::Establishes => "establishes",
    }
}

fn role_token(role: NodeRole) -> &'static str {
    match role {
        NodeRole::Symptom => "symptom",
        NodeRole::Question => "question",
        NodeRole::Hypothesis => "hypothesis",
        NodeRole::Component => "component",
        NodeRole::Observation => "observation",
        NodeRole::Conclusion => "conclusion",
    }
}

/// Greedy word wrap by estimated character width.
pub(crate) fn wrap(text: &str, max_chars: usize) -> Vec<String> {
    let max = max_chars.max(8);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if candidate.chars().count() > max && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            line = word.to_owned();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub(crate) fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn sanitize_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("kd-{cleaned}")
}

/// Defaults so an exported SVG is legible on its own; the web UI overrides
/// these through CSS custom properties.
const STYLE: &str = r#"<style>
.kd{font-family:var(--kd-font,ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif);font-size:13px}
.kd .kd-box{fill:var(--kd-surface,#fff);stroke:var(--kd-line,#3a3f47)}
.kd .kd-box-inner{fill:none;stroke:var(--kd-line,#3a3f47);stroke-width:1}
.kd .kd-label{fill:var(--kd-ink,#1b1e23);font-size:13px}
.kd .kd-detail{fill:var(--kd-muted,#5b616b);font-size:11px}
.kd .kd-glyph{fill:var(--kd-ink,#1b1e23);font-size:13px;font-weight:600}
.kd .kd-heading{fill:var(--kd-muted,#5b616b);font-size:11px;letter-spacing:.06em;text-transform:uppercase}
.kd .kd-edge{fill:none;stroke:var(--kd-line,#3a3f47)}
.kd .kd-arrowhead{fill:var(--kd-line,#3a3f47)}
.kd .kd-edge-label{fill:var(--kd-muted,#5b616b);font-size:11px;paint-order:stroke;stroke:var(--kd-surface,#fff);stroke-width:4px}
.kd .kd-annotation-line{stroke:var(--kd-muted,#5b616b);stroke-width:1}
.kd .kd-annotation-text{fill:var(--kd-ink,#1b1e23);font-size:12px}
.kd .kd-status-established .kd-box,.kd .kd-status-suspected .kd-box{stroke:var(--kd-accent,#9a4b00)}
.kd .kd-status-eliminated .kd-label{fill:var(--kd-muted,#5b616b)}
.kd .kd-rel-establishes{stroke:var(--kd-accent,#9a4b00)}
</style>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words() {
        assert_eq!(
            wrap("Worker execution capacity exhausted?", 20),
            vec!["Worker execution", "capacity exhausted?"]
        );
    }

    #[test]
    fn escapes_xml() {
        assert_eq!(esc(r#"<a & "b">"#), "&lt;a &amp; &quot;b&quot;&gt;");
    }
}
