use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::{
    CompositeSearchResult, FoError, Result, SearchResult,
};

pub const LINEAGE_GRAPH_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineageRelation {
    DerivedFrom,
    Reuses,
    NearDuplicate,
    SameFamily,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageSpan {
    pub start: usize,
    pub end: usize,
}

impl LineageSpan {
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.start < self.end
    }

    #[must_use]
    pub const fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageNode {
    pub id: String,
    pub title: String,
    pub observed_at_unix: Option<u64>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl LineageNode {
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(FoError::InvalidConfig(
                "lineage node id must not be empty".to_owned(),
            ));
        }
        if self.metadata.keys().any(|key| key.trim().is_empty()) {
            return Err(FoError::InvalidConfig(format!(
                "lineage node {} contains an empty metadata key",
                self.id
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageEvidence {
    pub method: String,
    pub score: f32,
    pub edit_similarity: f32,
    pub query_coverage: f32,
    pub source_coverage: f32,
    pub matched_tokens: usize,
    pub expected_false_matches: f64,
    #[serde(default)]
    pub source_spans: Vec<LineageSpan>,
    #[serde(default)]
    pub target_spans: Vec<LineageSpan>,
    #[serde(default)]
    pub reordered: bool,
    pub detected_at_unix: u64,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl LineageEvidence {
    pub fn validate(&self) -> Result<()> {
        if self.method.trim().is_empty() {
            return Err(FoError::InvalidConfig(
                "lineage evidence method must not be empty".to_owned(),
            ));
        }
        for (name, value) in [
            ("score", self.score),
            ("edit_similarity", self.edit_similarity),
            ("query_coverage", self.query_coverage),
            ("source_coverage", self.source_coverage),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(FoError::InvalidConfig(format!(
                    "lineage evidence {name} must be finite and lie in [0, 1]"
                )));
            }
        }
        if !self.expected_false_matches.is_finite() || self.expected_false_matches < 0.0 {
            return Err(FoError::InvalidConfig(
                "lineage expected_false_matches must be finite and non-negative".to_owned(),
            ));
        }
        if self.source_spans.iter().any(|span| !span.is_valid())
            || self.target_spans.iter().any(|span| !span.is_valid())
        {
            return Err(FoError::InvalidConfig(
                "lineage evidence contains an invalid span".to_owned(),
            ));
        }
        if self.metadata.keys().any(|key| key.trim().is_empty()) {
            return Err(FoError::InvalidConfig(
                "lineage evidence contains an empty metadata key".to_owned(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn from_search_result(result: &SearchResult, detected_at_unix: u64) -> Self {
        Self {
            method: "franken_overlap".to_owned(),
            score: result.combined_score.clamp(0.0, 1.0),
            edit_similarity: result.edit_similarity.clamp(0.0, 1.0),
            query_coverage: result.query_coverage.clamp(0.0, 1.0),
            source_coverage: result.source_coverage.clamp(0.0, 1.0),
            matched_tokens: result.matched_tokens,
            expected_false_matches: result.estimated_false_matches.max(0.0),
            source_spans: vec![LineageSpan {
                start: result.corpus_start,
                end: result.corpus_end,
            }],
            target_spans: vec![LineageSpan {
                start: result.query_start,
                end: result.query_end,
            }],
            reordered: false,
            detected_at_unix,
            metadata: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn from_composite_result(result: &CompositeSearchResult, detected_at_unix: u64) -> Self {
        Self {
            method: "franken_overlap_composite".to_owned(),
            score: result.aggregate_score.clamp(0.0, 1.0),
            edit_similarity: result.weighted_edit_similarity.clamp(0.0, 1.0),
            query_coverage: result.query_coverage.clamp(0.0, 1.0),
            source_coverage: result.source_coverage.clamp(0.0, 1.0),
            matched_tokens: result.matched_tokens,
            expected_false_matches: result.expected_false_matches.max(0.0),
            source_spans: result
                .blocks
                .iter()
                .map(|block| LineageSpan {
                    start: block.corpus_start,
                    end: block.corpus_end,
                })
                .collect(),
            target_spans: result
                .blocks
                .iter()
                .map(|block| LineageSpan {
                    start: block.query_start,
                    end: block.query_end,
                })
                .collect(),
            reordered: result.reordered_blocks,
            detected_at_unix,
            metadata: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn strength(&self) -> f32 {
        let coverage = harmonic_mean(self.query_coverage, self.source_coverage);
        let false_match_confidence = (1.0 / (1.0 + self.expected_false_matches)) as f32;
        (0.45 * self.score
            + 0.25 * self.edit_similarity
            + 0.20 * coverage
            + 0.10 * false_match_confidence)
            .clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageEdge {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub relation: LineageRelation,
    pub confidence: f32,
    #[serde(default)]
    pub evidence: Vec<LineageEvidence>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl LineageEdge {
    #[must_use]
    pub fn new(
        source_id: impl Into<String>,
        target_id: impl Into<String>,
        relation: LineageRelation,
        evidence: LineageEvidence,
    ) -> Self {
        let source_id = source_id.into();
        let target_id = target_id.into();
        let id = edge_id(&source_id, &target_id, relation);
        let confidence = evidence.strength();
        Self {
            id,
            source_id,
            target_id,
            relation,
            confidence,
            evidence: vec![evidence],
            metadata: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.source_id.trim().is_empty()
            || self.target_id.trim().is_empty()
            || self.source_id == self.target_id
        {
            return Err(FoError::InvalidConfig(
                "lineage edge endpoints must be nonempty and distinct".to_owned(),
            ));
        }
        let expected_id = edge_id(&self.source_id, &self.target_id, self.relation);
        if self.id != expected_id {
            return Err(FoError::InvalidConfig(format!(
                "lineage edge id {} does not match expected {}",
                self.id, expected_id
            )));
        }
        if !self.confidence.is_finite() || !(0.0..=1.0).contains(&self.confidence) {
            return Err(FoError::InvalidConfig(
                "lineage edge confidence must be finite and lie in [0, 1]".to_owned(),
            ));
        }
        if self.evidence.is_empty() {
            return Err(FoError::InvalidConfig(
                "lineage edge must retain at least one evidence record".to_owned(),
            ));
        }
        for evidence in &self.evidence {
            evidence.validate()?;
        }
        if self.metadata.keys().any(|key| key.trim().is_empty()) {
            return Err(FoError::InvalidConfig(
                "lineage edge contains an empty metadata key".to_owned(),
            ));
        }
        Ok(())
    }

    fn merge_evidence(&mut self, evidence: LineageEvidence) -> Result<bool> {
        evidence.validate()?;
        let fingerprint = evidence_fingerprint(&evidence);
        if self
            .evidence
            .iter()
            .any(|existing| evidence_fingerprint(existing) == fingerprint)
        {
            return Ok(false);
        }
        self.evidence.push(evidence);
        self.evidence.sort_unstable_by(|left, right| {
            left.detected_at_unix
                .cmp(&right.detected_at_unix)
                .then_with(|| left.method.cmp(&right.method))
                .then_with(|| right.strength().total_cmp(&left.strength()))
        });
        self.confidence = aggregate_confidence(&self.evidence);
        Ok(true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageGraph {
    pub schema_version: u32,
    #[serde(default)]
    pub nodes: BTreeMap<String, LineageNode>,
    #[serde(default)]
    pub edges: BTreeMap<String, LineageEdge>,
}

impl Default for LineageGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl LineageGraph {
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema_version: LINEAGE_GRAPH_SCHEMA_VERSION,
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != LINEAGE_GRAPH_SCHEMA_VERSION {
            return Err(FoError::InvalidConfig(format!(
                "unsupported lineage graph schema {}",
                self.schema_version
            )));
        }
        for (key, node) in &self.nodes {
            node.validate()?;
            if key != &node.id {
                return Err(FoError::InvalidConfig(format!(
                    "lineage node map key {key} disagrees with node id {}",
                    node.id
                )));
            }
        }
        for (key, edge) in &self.edges {
            edge.validate()?;
            if key != &edge.id {
                return Err(FoError::InvalidConfig(format!(
                    "lineage edge map key {key} disagrees with edge id {}",
                    edge.id
                )));
            }
            if !self.nodes.contains_key(&edge.source_id)
                || !self.nodes.contains_key(&edge.target_id)
            {
                return Err(FoError::InvalidConfig(format!(
                    "lineage edge {} references a missing endpoint",
                    edge.id
                )));
            }
        }
        Ok(())
    }

    pub fn upsert_node(&mut self, node: LineageNode) -> Result<bool> {
        node.validate()?;
        let changed = self.nodes.get(&node.id).is_none_or(|existing| {
            existing.title != node.title
                || existing.observed_at_unix != node.observed_at_unix
                || existing.metadata != node.metadata
        });
        self.nodes.insert(node.id.clone(), node);
        Ok(changed)
    }

    pub fn upsert_edge(&mut self, edge: LineageEdge) -> Result<bool> {
        edge.validate()?;
        if !self.nodes.contains_key(&edge.source_id)
            || !self.nodes.contains_key(&edge.target_id)
        {
            return Err(FoError::InvalidConfig(format!(
                "cannot insert lineage edge {} before both endpoint nodes exist",
                edge.id
            )));
        }
        match self.edges.get_mut(&edge.id) {
            Some(existing) => {
                let mut changed = false;
                for evidence in edge.evidence {
                    changed |= existing.merge_evidence(evidence)?;
                }
                for (key, value) in edge.metadata {
                    if existing.metadata.get(&key) != Some(&value) {
                        existing.metadata.insert(key, value);
                        changed = true;
                    }
                }
                Ok(changed)
            }
            None => {
                self.edges.insert(edge.id.clone(), edge);
                Ok(true)
            }
        }
    }

    pub fn add_evidence(
        &mut self,
        source_id: &str,
        target_id: &str,
        relation: LineageRelation,
        evidence: LineageEvidence,
    ) -> Result<bool> {
        self.upsert_edge(LineageEdge::new(
            source_id,
            target_id,
            relation,
            evidence,
        ))
    }

    pub fn ancestors(&self, node_id: &str, maximum_depth: usize) -> Result<Vec<LineageVisit>> {
        self.traverse(node_id, maximum_depth, TraversalDirection::Ancestors)
    }

    pub fn descendants(&self, node_id: &str, maximum_depth: usize) -> Result<Vec<LineageVisit>> {
        self.traverse(node_id, maximum_depth, TraversalDirection::Descendants)
    }

    pub fn connected_component(
        &self,
        node_id: &str,
        minimum_confidence: f32,
    ) -> Result<Vec<String>> {
        validate_confidence(minimum_confidence)?;
        if !self.nodes.contains_key(node_id) {
            return Err(FoError::InvalidConfig(format!(
                "unknown lineage node {node_id}"
            )));
        }
        let mut visited = BTreeSet::from([node_id.to_owned()]);
        let mut queue = VecDeque::from([node_id.to_owned()]);
        while let Some(current) = queue.pop_front() {
            for edge in self.edges.values().filter(|edge| {
                edge.confidence >= minimum_confidence
                    && (edge.source_id == current || edge.target_id == current)
            }) {
                let other = if edge.source_id == current {
                    &edge.target_id
                } else {
                    &edge.source_id
                };
                if visited.insert(other.clone()) {
                    queue.push_back(other.clone());
                }
            }
        }
        Ok(visited.into_iter().collect())
    }

    pub fn families(&self, minimum_confidence: f32) -> Result<Vec<LineageFamily>> {
        validate_confidence(minimum_confidence)?;
        let mut unvisited = self.nodes.keys().cloned().collect::<BTreeSet<_>>();
        let mut families = Vec::new();
        while let Some(node_id) = unvisited.first().cloned() {
            let members = self.connected_component(&node_id, minimum_confidence)?;
            for member in &members {
                unvisited.remove(member);
            }
            let member_set = members.iter().cloned().collect::<BTreeSet<_>>();
            let edges = self
                .edges
                .values()
                .filter(|edge| {
                    edge.confidence >= minimum_confidence
                        && member_set.contains(&edge.source_id)
                        && member_set.contains(&edge.target_id)
                })
                .count();
            let canonical = self.canonical_origin_from_members(&members)?;
            families.push(LineageFamily {
                canonical_id: canonical.node_id,
                members,
                edges,
            });
        }
        families.sort_unstable_by(|left, right| {
            right
                .members
                .len()
                .cmp(&left.members.len())
                .then_with(|| left.canonical_id.cmp(&right.canonical_id))
        });
        Ok(families)
    }

    pub fn canonical_origin(&self, node_id: &str) -> Result<CanonicalOrigin> {
        let members = self.connected_component(node_id, 0.0)?;
        self.canonical_origin_from_members(&members)
    }

    #[must_use]
    pub fn summary(&self) -> LineageSummary {
        let roots = self
            .nodes
            .keys()
            .filter(|node| !self.edges.values().any(|edge| &edge.target_id == *node))
            .count();
        let leaves = self
            .nodes
            .keys()
            .filter(|node| !self.edges.values().any(|edge| &edge.source_id == *node))
            .count();
        let evidence_records = self
            .edges
            .values()
            .map(|edge| edge.evidence.len())
            .sum();
        LineageSummary {
            schema_version: self.schema_version,
            nodes: self.nodes.len(),
            edges: self.edges.len(),
            evidence_records,
            roots,
            leaves,
        }
    }

    fn canonical_origin_from_members(&self, members: &[String]) -> Result<CanonicalOrigin> {
        let member_set = members.iter().cloned().collect::<BTreeSet<_>>();
        let mut candidates = members
            .iter()
            .map(|node_id| {
                let node = &self.nodes[node_id];
                let incoming = self
                    .edges
                    .values()
                    .filter(|edge| {
                        edge.target_id == *node_id && member_set.contains(&edge.source_id)
                    })
                    .count();
                let descendants = self
                    .edges
                    .values()
                    .filter(|edge| {
                        edge.source_id == *node_id && member_set.contains(&edge.target_id)
                    })
                    .count();
                (node, incoming, descendants)
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable_by(|(left, left_incoming, left_descendants), (right, right_incoming, right_descendants)| {
            left_incoming
                .cmp(right_incoming)
                .then_with(|| {
                    left.observed_at_unix
                        .unwrap_or(u64::MAX)
                        .cmp(&right.observed_at_unix.unwrap_or(u64::MAX))
                })
                .then_with(|| right_descendants.cmp(left_descendants))
                .then_with(|| left.id.cmp(&right.id))
        });
        let Some((node, incoming_edges, direct_descendants)) = candidates.first() else {
            return Err(FoError::InvalidConfig(
                "cannot select a canonical origin from an empty component".to_owned(),
            ));
        };
        Ok(CanonicalOrigin {
            node_id: node.id.clone(),
            observed_at_unix: node.observed_at_unix,
            incoming_edges: *incoming_edges,
            direct_descendants: *direct_descendants,
            component_size: members.len(),
        })
    }

    fn traverse(
        &self,
        node_id: &str,
        maximum_depth: usize,
        direction: TraversalDirection,
    ) -> Result<Vec<LineageVisit>> {
        if !self.nodes.contains_key(node_id) {
            return Err(FoError::InvalidConfig(format!(
                "unknown lineage node {node_id}"
            )));
        }
        if maximum_depth == 0 {
            return Ok(Vec::new());
        }
        let mut output = Vec::new();
        let mut seen = BTreeSet::from([node_id.to_owned()]);
        let mut queue = VecDeque::from([(node_id.to_owned(), 0usize, 1.0f32)]);
        while let Some((current, depth, path_confidence)) = queue.pop_front() {
            if depth >= maximum_depth {
                continue;
            }
            for edge in self.edges.values() {
                let next = match direction {
                    TraversalDirection::Ancestors if edge.target_id == current => {
                        Some(edge.source_id.as_str())
                    }
                    TraversalDirection::Descendants if edge.source_id == current => {
                        Some(edge.target_id.as_str())
                    }
                    _ => None,
                };
                let Some(next) = next else {
                    continue;
                };
                let confidence = path_confidence.min(edge.confidence);
                if seen.insert(next.to_owned()) {
                    let next_depth = depth + 1;
                    output.push(LineageVisit {
                        node_id: next.to_owned(),
                        depth: next_depth,
                        path_confidence: confidence,
                        via_edge_id: edge.id.clone(),
                    });
                    queue.push_back((next.to_owned(), next_depth, confidence));
                }
            }
        }
        output.sort_unstable_by(|left, right| {
            left.depth
                .cmp(&right.depth)
                .then_with(|| right.path_confidence.total_cmp(&left.path_confidence))
                .then_with(|| left.node_id.cmp(&right.node_id))
        });
        Ok(output)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageVisit {
    pub node_id: String,
    pub depth: usize,
    pub path_confidence: f32,
    pub via_edge_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalOrigin {
    pub node_id: String,
    pub observed_at_unix: Option<u64>,
    pub incoming_edges: usize,
    pub direct_descendants: usize,
    pub component_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageFamily {
    pub canonical_id: String,
    pub members: Vec<String>,
    pub edges: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageSummary {
    pub schema_version: u32,
    pub nodes: usize,
    pub edges: usize,
    pub evidence_records: usize,
    pub roots: usize,
    pub leaves: usize,
}

#[derive(Debug, Clone, Copy)]
enum TraversalDirection {
    Ancestors,
    Descendants,
}

#[must_use]
pub fn edge_id(source_id: &str, target_id: &str, relation: LineageRelation) -> String {
    let relation = match relation {
        LineageRelation::DerivedFrom => "derived_from",
        LineageRelation::Reuses => "reuses",
        LineageRelation::NearDuplicate => "near_duplicate",
        LineageRelation::SameFamily => "same_family",
    };
    let mut hash = FNV_OFFSET;
    for value in [source_id, "\0", target_id, "\0", relation] {
        hash = fnv_extend(hash, value.as_bytes());
    }
    format!("edge-{hash:016x}")
}

fn evidence_fingerprint(evidence: &LineageEvidence) -> u64 {
    let mut hash = FNV_OFFSET;
    hash = fnv_extend(hash, evidence.method.as_bytes());
    hash = fnv_extend(hash, &evidence.score.to_bits().to_le_bytes());
    hash = fnv_extend(hash, &evidence.edit_similarity.to_bits().to_le_bytes());
    hash = fnv_extend(hash, &evidence.query_coverage.to_bits().to_le_bytes());
    hash = fnv_extend(hash, &evidence.source_coverage.to_bits().to_le_bytes());
    hash = fnv_extend(hash, &evidence.matched_tokens.to_le_bytes());
    hash = fnv_extend(hash, &evidence.expected_false_matches.to_bits().to_le_bytes());
    hash = fnv_extend(hash, &evidence.detected_at_unix.to_le_bytes());
    for span in evidence.source_spans.iter().chain(&evidence.target_spans) {
        hash = fnv_extend(hash, &span.start.to_le_bytes());
        hash = fnv_extend(hash, &span.end.to_le_bytes());
    }
    hash
}

fn aggregate_confidence(evidence: &[LineageEvidence]) -> f32 {
    let miss_probability = evidence.iter().fold(1.0f64, |accumulator, item| {
        accumulator * (1.0 - f64::from(item.strength()).clamp(0.0, 1.0))
    });
    (1.0 - miss_probability).clamp(0.0, 1.0) as f32
}

fn validate_confidence(value: f32) -> Result<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(FoError::InvalidConfig(
            "minimum lineage confidence must be finite and lie in [0, 1]".to_owned(),
        ));
    }
    Ok(())
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv_extend(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn harmonic_mean(left: f32, right: f32) -> f32 {
    if left <= 0.0 || right <= 0.0 {
        0.0
    } else {
        (2.0 * left * right / (left + right)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        LineageEdge, LineageEvidence, LineageGraph, LineageNode, LineageRelation,
        LineageSpan,
    };

    fn node(id: &str, observed_at_unix: u64) -> LineageNode {
        LineageNode {
            id: id.to_owned(),
            title: id.to_owned(),
            observed_at_unix: Some(observed_at_unix),
            metadata: BTreeMap::new(),
        }
    }

    fn evidence(score: f32, detected_at_unix: u64) -> LineageEvidence {
        LineageEvidence {
            method: "fixture".to_owned(),
            score,
            edit_similarity: score,
            query_coverage: score,
            source_coverage: score,
            matched_tokens: 40,
            expected_false_matches: 0.01,
            source_spans: vec![LineageSpan { start: 10, end: 50 }],
            target_spans: vec![LineageSpan { start: 0, end: 40 }],
            reordered: false,
            detected_at_unix,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn graph_accumulates_distinct_evidence_and_traverses() {
        let mut graph = LineageGraph::new();
        graph.upsert_node(node("a", 1)).expect("node a");
        graph.upsert_node(node("b", 2)).expect("node b");
        graph.upsert_node(node("c", 3)).expect("node c");
        graph
            .upsert_edge(LineageEdge::new(
                "a",
                "b",
                LineageRelation::DerivedFrom,
                evidence(0.8, 10),
            ))
            .expect("edge a-b");
        graph
            .upsert_edge(LineageEdge::new(
                "a",
                "b",
                LineageRelation::DerivedFrom,
                evidence(0.9, 11),
            ))
            .expect("merge a-b");
        graph
            .upsert_edge(LineageEdge::new(
                "b",
                "c",
                LineageRelation::DerivedFrom,
                evidence(0.75, 12),
            ))
            .expect("edge b-c");

        graph.validate().expect("valid graph");
        assert_eq!(graph.edges.len(), 2);
        assert_eq!(graph.edges.values().next().expect("edge").evidence.len(), 2);
        let ancestors = graph.ancestors("c", 4).expect("ancestors");
        assert_eq!(ancestors.len(), 2);
        assert_eq!(ancestors[0].node_id, "b");
        assert_eq!(ancestors[1].node_id, "a");
        assert_eq!(graph.canonical_origin("c").expect("origin").node_id, "a");
    }

    #[test]
    fn families_are_deterministic_and_include_isolates() {
        let mut graph = LineageGraph::new();
        for (id, timestamp) in [("a", 1), ("b", 2), ("z", 3)] {
            graph.upsert_node(node(id, timestamp)).expect("node");
        }
        graph
            .upsert_edge(LineageEdge::new(
                "a",
                "b",
                LineageRelation::Reuses,
                evidence(0.9, 10),
            ))
            .expect("edge");
        let families = graph.families(0.5).expect("families");
        assert_eq!(families.len(), 2);
        assert_eq!(families[0].members, vec!["a", "b"]);
        assert_eq!(families[1].members, vec!["z"]);
    }

    #[test]
    fn duplicate_evidence_is_not_counted_twice() {
        let mut graph = LineageGraph::new();
        graph.upsert_node(node("a", 1)).expect("node a");
        graph.upsert_node(node("b", 2)).expect("node b");
        let edge = LineageEdge::new(
            "a",
            "b",
            LineageRelation::DerivedFrom,
            evidence(0.8, 10),
        );
        assert!(graph.upsert_edge(edge.clone()).expect("first"));
        assert!(!graph.upsert_edge(edge).expect("duplicate"));
        assert_eq!(graph.edges.values().next().expect("edge").evidence.len(), 1);
    }
}
