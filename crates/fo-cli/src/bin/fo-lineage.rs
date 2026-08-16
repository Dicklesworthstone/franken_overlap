#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use fo_core::{
    CompositeSearchResult, HybridOverlapEvidence, HybridSearchReport, LineageEdge,
    LineageEvidence, LineageGraph, LineageNode, LineageRelation, SearchResult,
};
use serde::{Deserialize, Serialize};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-lineage",
    version,
    about = "Build and inspect durable textual provenance and document-lineage graphs"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a new empty lineage graph.
    Init(InitCommand),
    /// Add or replace one node and its metadata.
    Node(NodeCommand),
    /// Ingest ordinary, composite, or hybrid FrankenOverlap JSON results.
    Ingest(IngestCommand),
    /// Merge another lineage graph into this graph.
    Merge(MergeCommand),
    /// List ancestors of a node.
    Ancestors(TraversalCommand),
    /// List descendants of a node.
    Descendants(TraversalCommand),
    /// Select a canonical origin for one connected component.
    Origin(NodeQueryCommand),
    /// List connected textual families.
    Families(FamiliesCommand),
    /// Print graph statistics.
    Summary(GraphCommand),
    /// Validate graph schema, endpoints, spans, scores, and deterministic edge IDs.
    Verify(GraphCommand),
    /// Export a Graphviz DOT representation.
    Dot(DotCommand),
}

#[derive(Debug, Args)]
struct InitCommand {
    graph: PathBuf,
    #[arg(long)]
    replace: bool,
}

#[derive(Debug, Args)]
struct NodeCommand {
    graph: PathBuf,
    #[arg(long)]
    id: String,
    #[arg(long, default_value = "")]
    title: String,
    #[arg(long)]
    observed_at_unix: Option<u64>,
    #[arg(long = "metadata")]
    metadata: Vec<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct IngestCommand {
    graph: PathBuf,
    results: PathBuf,
    #[arg(long)]
    target_id: String,
    #[arg(long, default_value = "")]
    target_title: String,
    #[arg(long)]
    target_observed_at_unix: Option<u64>,
    #[arg(long, value_enum, default_value = "derived-from")]
    relation: RelationArg,
    #[arg(long, default_value_t = 0)]
    detected_at_unix: u64,
    #[arg(long, default_value_t = 0.0)]
    minimum_score: f32,
    #[arg(long, default_value_t = 0.0)]
    minimum_query_coverage: f32,
    #[arg(long, default_value_t = 0)]
    minimum_matched_tokens: usize,
    #[arg(long = "target-metadata")]
    target_metadata: Vec<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct MergeCommand {
    graph: PathBuf,
    other: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct TraversalCommand {
    graph: PathBuf,
    node_id: String,
    #[arg(long, default_value_t = 32)]
    maximum_depth: usize,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct NodeQueryCommand {
    graph: PathBuf,
    node_id: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct FamiliesCommand {
    graph: PathBuf,
    #[arg(long, default_value_t = 0.50)]
    minimum_confidence: f32,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct GraphCommand {
    graph: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct DotCommand {
    graph: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long, default_value_t = 0.0)]
    minimum_confidence: f32,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RelationArg {
    DerivedFrom,
    Reuses,
    NearDuplicate,
    SameFamily,
}

impl From<RelationArg> for LineageRelation {
    fn from(value: RelationArg) -> Self {
        match value {
            RelationArg::DerivedFrom => Self::DerivedFrom,
            RelationArg::Reuses => Self::Reuses,
            RelationArg::NearDuplicate => Self::NearDuplicate,
            RelationArg::SameFamily => Self::SameFamily,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ResultInput {
    Raw(Vec<SearchResult>),
    Composite(Vec<CompositeSearchResult>),
    Hybrid(HybridSearchReport),
    RawLines(Vec<RawResultEnvelope>),
}

#[derive(Debug, Deserialize)]
struct RawResultEnvelope {
    result: SearchResult,
}

#[derive(Debug, Serialize)]
struct IngestReport {
    graph: String,
    target_id: String,
    parsed_results: usize,
    retained_results: usize,
    inserted_nodes: usize,
    changed_edges: usize,
    skipped_without_overlap_evidence: usize,
    summary: fo_core::LineageSummary,
}

#[derive(Debug, Serialize)]
struct MergeReport {
    graph: String,
    source_graph: String,
    changed_nodes: usize,
    changed_edges: usize,
    summary: fo_core::LineageSummary,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-lineage: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Init(command) => run_init(command),
        Command::Node(command) => run_node(command),
        Command::Ingest(command) => run_ingest(command),
        Command::Merge(command) => run_merge(command),
        Command::Ancestors(command) => run_traversal(command, true),
        Command::Descendants(command) => run_traversal(command, false),
        Command::Origin(command) => run_origin(command),
        Command::Families(command) => run_families(command),
        Command::Summary(command) => run_summary(command),
        Command::Verify(command) => run_verify(command),
        Command::Dot(command) => run_dot(command),
    }
}

fn run_init(command: InitCommand) -> CliResult<()> {
    if command.graph.exists() && !command.replace {
        return Err(invalid(format!(
            "{} already exists; pass --replace to reset it",
            command.graph.display()
        )));
    }
    save_graph(&command.graph, &LineageGraph::new())?;
    println!("Created {}", command.graph.display());
    Ok(())
}

fn run_node(command: NodeCommand) -> CliResult<()> {
    let mut graph = load_or_new_graph(&command.graph)?;
    let changed = graph.upsert_node(LineageNode {
        id: command.id.clone(),
        title: if command.title.is_empty() {
            command.id.clone()
        } else {
            command.title
        },
        observed_at_unix: command.observed_at_unix,
        metadata: parse_metadata(&command.metadata)?,
    })?;
    graph.validate()?;
    save_graph(&command.graph, &graph)?;
    if command.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "changed": changed,
                "node": graph.nodes[&command.id],
                "summary": graph.summary(),
            }))?
        );
    } else {
        println!("{} node {}", if changed { "Updated" } else { "Retained" }, command.id);
    }
    Ok(())
}

fn run_ingest(command: IngestCommand) -> CliResult<()> {
    validate_threshold("minimum_score", command.minimum_score)?;
    validate_threshold(
        "minimum_query_coverage",
        command.minimum_query_coverage,
    )?;
    let mut graph = load_or_new_graph(&command.graph)?;
    let target_metadata = parse_metadata(&command.target_metadata)?;
    let target_changed = graph.upsert_node(LineageNode {
        id: command.target_id.clone(),
        title: if command.target_title.is_empty() {
            command.target_id.clone()
        } else {
            command.target_title
        },
        observed_at_unix: command.target_observed_at_unix,
        metadata: target_metadata,
    })?;
    let input = read_result_input(&command.results)?;
    let relation = command.relation.into();
    let mut parsed_results = 0usize;
    let mut retained_results = 0usize;
    let mut inserted_nodes = usize::from(target_changed);
    let mut changed_edges = 0usize;
    let mut skipped_without_overlap_evidence = 0usize;

    match input {
        ResultInput::Raw(results) => {
            parsed_results = results.len();
            for result in results {
                if !retain_raw(&result, &command) {
                    continue;
                }
                retained_results += 1;
                inserted_nodes += usize::from(ensure_source_node(
                    &mut graph,
                    &result.path,
                    &result.path,
                    None,
                    BTreeMap::new(),
                )?);
                changed_edges += usize::from(graph.add_evidence(
                    &result.path,
                    &command.target_id,
                    relation,
                    LineageEvidence::from_search_result(&result, command.detected_at_unix),
                )?);
            }
        }
        ResultInput::RawLines(results) => {
            parsed_results = results.len();
            for envelope in results {
                let result = envelope.result;
                if !retain_raw(&result, &command) {
                    continue;
                }
                retained_results += 1;
                inserted_nodes += usize::from(ensure_source_node(
                    &mut graph,
                    &result.path,
                    &result.path,
                    None,
                    BTreeMap::new(),
                )?);
                changed_edges += usize::from(graph.add_evidence(
                    &result.path,
                    &command.target_id,
                    relation,
                    LineageEvidence::from_search_result(&result, command.detected_at_unix),
                )?);
            }
        }
        ResultInput::Composite(results) => {
            parsed_results = results.len();
            for result in results {
                if result.aggregate_score < command.minimum_score
                    || result.query_coverage < command.minimum_query_coverage
                    || result.matched_tokens < command.minimum_matched_tokens
                {
                    continue;
                }
                retained_results += 1;
                inserted_nodes += usize::from(ensure_source_node(
                    &mut graph,
                    &result.path,
                    &result.path,
                    None,
                    BTreeMap::new(),
                )?);
                changed_edges += usize::from(graph.add_evidence(
                    &result.path,
                    &command.target_id,
                    relation,
                    LineageEvidence::from_composite_result(&result, command.detected_at_unix),
                )?);
            }
        }
        ResultInput::Hybrid(report) => {
            parsed_results = report.results.len();
            for result in report.results {
                let Some(overlap) = result.overlap else {
                    skipped_without_overlap_evidence += 1;
                    continue;
                };
                let (score, coverage, matched_tokens, evidence) = match overlap {
                    HybridOverlapEvidence::Passage(ref passage) => (
                        passage.combined_score,
                        passage.query_coverage,
                        passage.matched_tokens,
                        LineageEvidence::from_search_result(
                            passage,
                            command.detected_at_unix,
                        ),
                    ),
                    HybridOverlapEvidence::Composite(ref composite) => (
                        composite.aggregate_score,
                        composite.query_coverage,
                        composite.matched_tokens,
                        LineageEvidence::from_composite_result(
                            composite,
                            command.detected_at_unix,
                        ),
                    ),
                };
                if score < command.minimum_score
                    || coverage < command.minimum_query_coverage
                    || matched_tokens < command.minimum_matched_tokens
                {
                    continue;
                }
                retained_results += 1;
                inserted_nodes += usize::from(ensure_source_node(
                    &mut graph,
                    &result.external_id,
                    &result.title,
                    metadata_timestamp(&result.metadata),
                    result.metadata,
                )?);
                changed_edges += usize::from(graph.add_evidence(
                    &result.external_id,
                    &command.target_id,
                    relation,
                    evidence,
                )?);
            }
        }
    }

    graph.validate()?;
    save_graph(&command.graph, &graph)?;
    let report = IngestReport {
        graph: command.graph.display().to_string(),
        target_id: command.target_id,
        parsed_results,
        retained_results,
        inserted_nodes,
        changed_edges,
        skipped_without_overlap_evidence,
        summary: graph.summary(),
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Graph:                    {}", report.graph);
        println!("Target:                   {}", report.target_id);
        println!("Parsed results:           {}", report.parsed_results);
        println!("Retained results:         {}", report.retained_results);
        println!("Changed nodes:            {}", report.inserted_nodes);
        println!("Changed edges:            {}", report.changed_edges);
        println!(
            "Skipped lexical-only hits: {}",
            report.skipped_without_overlap_evidence
        );
        println!("Total nodes / edges:      {} / {}", report.summary.nodes, report.summary.edges);
    }
    Ok(())
}

fn run_merge(command: MergeCommand) -> CliResult<()> {
    let mut graph = load_or_new_graph(&command.graph)?;
    let other = load_graph(&command.other)?;
    let mut changed_nodes = 0usize;
    let mut changed_edges = 0usize;
    for node in other.nodes.into_values() {
        changed_nodes += usize::from(graph.upsert_node(node)?);
    }
    for edge in other.edges.into_values() {
        changed_edges += usize::from(graph.upsert_edge(edge)?);
    }
    graph.validate()?;
    save_graph(&command.graph, &graph)?;
    let report = MergeReport {
        graph: command.graph.display().to_string(),
        source_graph: command.other.display().to_string(),
        changed_nodes,
        changed_edges,
        summary: graph.summary(),
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Merged {} into {}", report.source_graph, report.graph);
        println!("Changed nodes / edges: {} / {}", changed_nodes, changed_edges);
    }
    Ok(())
}

fn run_traversal(command: TraversalCommand, ancestors: bool) -> CliResult<()> {
    let graph = load_graph(&command.graph)?;
    let visits = if ancestors {
        graph.ancestors(&command.node_id, command.maximum_depth)?
    } else {
        graph.descendants(&command.node_id, command.maximum_depth)?
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&visits)?);
    } else {
        for visit in visits {
            println!(
                "depth={} confidence={:.4} {} via {}",
                visit.depth, visit.path_confidence, visit.node_id, visit.via_edge_id
            );
        }
    }
    Ok(())
}

fn run_origin(command: NodeQueryCommand) -> CliResult<()> {
    let graph = load_graph(&command.graph)?;
    let origin = graph.canonical_origin(&command.node_id)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&origin)?);
    } else {
        println!("Canonical origin:     {}", origin.node_id);
        println!("Observed at:          {:?}", origin.observed_at_unix);
        println!("Component size:       {}", origin.component_size);
        println!("Incoming edges:       {}", origin.incoming_edges);
        println!("Direct descendants:   {}", origin.direct_descendants);
    }
    Ok(())
}

fn run_families(command: FamiliesCommand) -> CliResult<()> {
    let graph = load_graph(&command.graph)?;
    let families = graph.families(command.minimum_confidence)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&families)?);
    } else {
        for (index, family) in families.iter().enumerate() {
            println!(
                "{}. canonical={} members={} edges={}",
                index + 1,
                family.canonical_id,
                family.members.len(),
                family.edges
            );
            for member in &family.members {
                println!("   {member}");
            }
        }
    }
    Ok(())
}

fn run_summary(command: GraphCommand) -> CliResult<()> {
    let graph = load_graph(&command.graph)?;
    let summary = graph.summary();
    if command.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("Nodes:             {}", summary.nodes);
        println!("Edges:             {}", summary.edges);
        println!("Evidence records:  {}", summary.evidence_records);
        println!("Roots:             {}", summary.roots);
        println!("Leaves:            {}", summary.leaves);
    }
    Ok(())
}

fn run_verify(command: GraphCommand) -> CliResult<()> {
    let graph = load_graph(&command.graph)?;
    graph.validate()?;
    if command.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "valid": true,
                "summary": graph.summary(),
            }))?
        );
    } else {
        println!("{} is valid", command.graph.display());
    }
    Ok(())
}

fn run_dot(command: DotCommand) -> CliResult<()> {
    validate_threshold("minimum_confidence", command.minimum_confidence)?;
    let graph = load_graph(&command.graph)?;
    let dot = render_dot(&graph, command.minimum_confidence);
    if let Some(path) = command.output {
        atomic_write(&path, dot.as_bytes())?;
        println!("Wrote {}", path.display());
    } else {
        print!("{dot}");
    }
    Ok(())
}

fn read_result_input(path: &Path) -> CliResult<ResultInput> {
    let bytes = fs::read(path)?;
    if let Ok(input) = serde_json::from_slice::<ResultInput>(&bytes) {
        return Ok(input);
    }
    let file = fs::File::open(path)?;
    let mut envelopes = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        let result = serde_json::from_str::<SearchResult>(value).map_err(|error| {
            invalid(format!("{}:{}: {error}", path.display(), line_index + 1))
        })?;
        envelopes.push(RawResultEnvelope { result });
    }
    if envelopes.is_empty() {
        return Err(invalid(format!(
            "{} contains no recognized search results",
            path.display()
        )));
    }
    Ok(ResultInput::RawLines(envelopes))
}

fn retain_raw(result: &SearchResult, command: &IngestCommand) -> bool {
    result.combined_score >= command.minimum_score
        && result.query_coverage >= command.minimum_query_coverage
        && result.matched_tokens >= command.minimum_matched_tokens
}

fn ensure_source_node(
    graph: &mut LineageGraph,
    id: &str,
    title: &str,
    observed_at_unix: Option<u64>,
    metadata: BTreeMap<String, String>,
) -> CliResult<bool> {
    Ok(graph.upsert_node(LineageNode {
        id: id.to_owned(),
        title: if title.trim().is_empty() {
            id.to_owned()
        } else {
            title.to_owned()
        },
        observed_at_unix,
        metadata,
    })?)
}

fn metadata_timestamp(metadata: &BTreeMap<String, String>) -> Option<u64> {
    for key in [
        "observed_at_unix",
        "filed_at_unix",
        "published_at_unix",
        "timestamp",
    ] {
        if let Some(value) = metadata.get(key).and_then(|value| value.parse().ok()) {
            return Some(value);
        }
    }
    None
}

fn load_or_new_graph(path: &Path) -> CliResult<LineageGraph> {
    if path.exists() {
        load_graph(path)
    } else {
        Ok(LineageGraph::new())
    }
}

fn load_graph(path: &Path) -> CliResult<LineageGraph> {
    let graph = serde_json::from_slice::<LineageGraph>(&fs::read(path)?)?;
    graph.validate()?;
    Ok(graph)
}

fn save_graph(path: &Path, graph: &LineageGraph) -> CliResult<()> {
    graph.validate()?;
    atomic_write(path, &serde_json::to_vec_pretty(graph)?)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> CliResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "{}.tmp-{}",
        path.extension().and_then(|value| value.to_str()).unwrap_or("json"),
        std::process::id()
    ));
    {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

fn parse_metadata(values: &[String]) -> CliResult<BTreeMap<String, String>> {
    let mut metadata = BTreeMap::new();
    for value in values {
        let Some((key, value)) = value.split_once('=') else {
            return Err(invalid(format!("metadata must use KEY=VALUE: {value:?}")));
        };
        if key.trim().is_empty() {
            return Err(invalid("metadata keys must not be empty"));
        }
        metadata.insert(key.trim().to_owned(), value.to_owned());
    }
    Ok(metadata)
}

fn validate_threshold(name: &str, value: f32) -> CliResult<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(invalid(format!("{name} must lie in [0, 1]")));
    }
    Ok(())
}

fn render_dot(graph: &LineageGraph, minimum_confidence: f32) -> String {
    let mut output = String::from("digraph franken_overlap_lineage {\n  rankdir=LR;\n");
    for node in graph.nodes.values() {
        output.push_str(&format!(
            "  {} [label=\"{}\"];\n",
            dot_quote(&node.id),
            dot_escape(&node.title)
        ));
    }
    for edge in graph
        .edges
        .values()
        .filter(|edge| edge.confidence >= minimum_confidence)
    {
        output.push_str(&format!(
            "  {} -> {} [label=\"{:?} {:.3}\"];\n",
            dot_quote(&edge.source_id),
            dot_quote(&edge.target_id),
            edge.relation,
            edge.confidence
        ));
    }
    output.push_str("}\n");
    output
}

fn dot_quote(value: &str) -> String {
    format!("\"{}\"", dot_escape(value))
}

fn dot_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
