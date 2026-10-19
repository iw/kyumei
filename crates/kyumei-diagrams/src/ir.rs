//! The diagram intermediate representation.

use std::fmt;

use kyumei_domain::DomainRef;
use serde::{Deserialize, Serialize};

/// A diagnostic diagram: semantic nodes and relationships, never drawing
/// instructions. Kyumei lays it out and renders it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagram {
    pub id: String,
    pub title: String,
    pub kind: DiagramKind,
    /// A one-paragraph description for assistive technology.
    pub description: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub annotations: Vec<Annotation>,
}

/// The approved diagram types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagramKind {
    /// The reasoning path through the hypotheses.
    DiagnosticPath,
    /// Where in the request path the suspected problem sits.
    SystemPath,
    /// How observations bear on hypotheses.
    EvidenceRelationship,
}

impl DiagramKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::DiagnosticPath => "Diagnostic path",
            Self::SystemPath => "System path",
            Self::EvidenceRelationship => "Evidence relationships",
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::DiagnosticPath => "diagnostic-path",
            Self::SystemPath => "system-path",
            Self::EvidenceRelationship => "evidence-relationship",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub label: String,
    /// A short second line (a value, a state, a question).
    pub detail: Option<String>,
    pub role: NodeRole,
    pub status: NodeStatus,
    /// The domain objects this node depicts.
    pub refers_to: Vec<DomainRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    Symptom,
    /// A question on the diagnostic path, answered by a hypothesis state.
    Question,
    Hypothesis,
    /// A part of the Temporal request path.
    Component,
    Observation,
    Conclusion,
}

/// Node status. Every status has a glyph and a word, so diagrams never
/// depend on colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Neutral,
    Open,
    Supported,
    Weakened,
    Eliminated,
    Established,
    /// An observation from telemetry.
    Observed,
    /// An observation the operator asserted.
    Asserted,
    /// A value a simulation produced.
    Simulated,
    /// A component where evidence places the problem.
    Suspected,
    /// A component evidence clears.
    Cleared,
}

impl NodeStatus {
    pub fn word(self) -> &'static str {
        match self {
            Self::Neutral => "",
            Self::Open => "open",
            Self::Supported => "supported",
            Self::Weakened => "weakened",
            Self::Eliminated => "eliminated",
            Self::Established => "established",
            Self::Observed => "observed",
            Self::Asserted => "asserted",
            Self::Simulated => "simulated",
            Self::Suspected => "suspected constraint",
            Self::Cleared => "cleared",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Neutral => "",
            Self::Open => "?",
            Self::Supported => "+",
            Self::Weakened => "−",
            Self::Eliminated => "✕",
            Self::Established => "✓",
            Self::Observed => "●",
            Self::Asserted => "○",
            Self::Simulated => "◌",
            Self::Suspected => "◆",
            // Not ✓, which means "established" on the diagnostic path.
            Self::Cleared => "◇",
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Open => "open",
            Self::Supported => "supported",
            Self::Weakened => "weakened",
            Self::Eliminated => "eliminated",
            Self::Established => "established",
            Self::Observed => "observed",
            Self::Asserted => "asserted",
            Self::Simulated => "simulated",
            Self::Suspected => "suspected",
            Self::Cleared => "cleared",
        }
    }

    /// The short answer shown on a diagnostic path edge.
    pub fn answer(self) -> &'static str {
        match self {
            Self::Eliminated => "NO",
            Self::Weakened => "UNLIKELY",
            Self::Supported => "LIKELY",
            Self::Established => "YES",
            _ => "OPEN",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub relation: Relation,
    pub label: Option<String>,
    pub refers_to: Vec<DomainRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// Next step on the diagnostic path.
    LeadsTo,
    /// Request flow between components.
    FlowsTo,
    Supports,
    Weakens,
    Eliminates,
    Establishes,
}

impl Relation {
    pub fn word(self) -> &'static str {
        match self {
            Self::LeadsTo => "leads to",
            Self::FlowsTo => "flows to",
            Self::Supports => "supports",
            Self::Weakens => "weakens",
            Self::Eliminates => "eliminates",
            Self::Establishes => "establishes",
        }
    }

    pub fn is_evidential(self) -> bool {
        matches!(
            self,
            Self::Supports | Self::Weakens | Self::Eliminates | Self::Establishes
        )
    }
}

/// A note attached to a node ("capacity available", "suspected constraint").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub target: NodeId,
    pub text: String,
    pub refers_to: Vec<DomainRef>,
}

impl Diagram {
    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| &n.id == id)
    }

    /// Every domain object the diagram refers to.
    pub fn references(&self) -> Vec<DomainRef> {
        let mut refs: Vec<DomainRef> = self
            .nodes
            .iter()
            .flat_map(|n| n.refers_to.iter().cloned())
            .chain(self.edges.iter().flat_map(|e| e.refers_to.iter().cloned()))
            .chain(
                self.annotations
                    .iter()
                    .flat_map(|a| a.refers_to.iter().cloned()),
            )
            .collect();
        refs.sort();
        refs.dedup();
        refs
    }
}
