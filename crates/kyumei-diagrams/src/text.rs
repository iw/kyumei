//! Plain-text rendering: the accessible fallback and the Markdown export.

use std::fmt::Write as _;

use crate::ir::{Diagram, DiagramKind, NodeRole, Relation};

pub fn render_text(d: &Diagram) -> String {
    match d.kind {
        DiagramKind::DiagnosticPath | DiagramKind::SystemPath => path_text(d),
        DiagramKind::EvidenceRelationship => relationship_text(d),
    }
}

fn path_text(d: &Diagram) -> String {
    let mut out = String::new();
    let order = order(d);
    for (i, node) in order.iter().enumerate() {
        if i > 0 {
            let edge = d.edges.iter().find(|e| e.to == node.id);
            let _ = writeln!(out, "    │");
            if let Some(label) = edge.and_then(|e| e.label.as_deref()) {
                let _ = writeln!(out, "    │ {label}");
            }
            let _ = writeln!(out, "    ▼");
        }
        let glyph = node.status.glyph();
        let status = node.status.word();
        let mut line = String::new();
        if !glyph.is_empty() {
            let _ = write!(line, "[{glyph}] ");
        }
        line.push_str(&node.label);
        if !status.is_empty()
            && d.kind == DiagramKind::DiagnosticPath
            && node.role != NodeRole::Symptom
        {
            let _ = write!(line, " — {status}");
        }
        for a in d.annotations.iter().filter(|a| a.target == node.id) {
            let _ = write!(line, "   ← {}", a.text);
        }
        let _ = writeln!(out, "{line}");
        if let Some(detail) = &node.detail {
            let _ = writeln!(out, "    {detail}");
        }
    }
    out
}

fn relationship_text(d: &Diagram) -> String {
    let mut out = String::new();
    for h in d.nodes.iter().filter(|n| n.role == NodeRole::Hypothesis) {
        let _ = writeln!(
            out,
            "[{}] {} — {}",
            h.status.glyph(),
            h.label,
            h.status.word()
        );
        for e in d.edges.iter().filter(|e| e.to == h.id) {
            let Some(from) = d.node(&e.from) else {
                continue;
            };
            let arrow = match e.relation {
                Relation::Establishes => "══►",
                Relation::Supports => "──►",
                Relation::Weakens => "- ►",
                Relation::Eliminates => "─✕►",
                _ => "──►",
            };
            let _ = writeln!(out, "    {} {arrow} {}", from.label, e.relation.word());
        }
    }
    out
}

fn order(d: &Diagram) -> Vec<&crate::ir::Node> {
    let targets: std::collections::BTreeSet<_> = d.edges.iter().map(|e| &e.to).collect();
    let mut order = Vec::new();
    let mut current = d.nodes.iter().find(|n| !targets.contains(&n.id));
    while let Some(node) = current {
        if order.iter().any(|n: &&crate::ir::Node| n.id == node.id) {
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
