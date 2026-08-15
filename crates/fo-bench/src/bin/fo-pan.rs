#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use clap::{Args, Parser, Subcommand};
use fo_core::{
    CompositeSearchOptions, IndexBuilder, IndexConfig, NormalizationProfile, OriginalByteRange,
    PanAnnotation, PanEvaluationReport, SearchIntent, SearchOptions, normalize_with_provenance,
    pan_evaluate,
};
use serde::Serialize;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-pan",
    version,
    about = "Run and evaluate FrankenOverlap on PAN text-alignment corpora"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Evaluate PAN-format detection XML against PAN-format truth XML.
    Evaluate(EvaluateCommand),
    /// Run FrankenOverlap over a PAN pairs file, write XML detections, and evaluate them.
    Run(RunCommand),
}

#[derive(Debug, Args)]
struct EvaluateCommand {
    truth_dir: PathBuf,
    detections_dir: PathBuf,
    #[arg(long, default_value = "plagiarism")]
    truth_tag: String,
    #[arg(long, default_value = "detected-plagiarism")]
    detection_tag: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct RunCommand {
    /// PAN pairs file: one suspicious and source filename per line.
    pairs: PathBuf,
    source_dir: PathBuf,
    suspicious_dir: PathBuf,
    truth_dir: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, default_value_t = 0.12)]
    minimum_score: f32,
    #[arg(long, default_value_t = 20)]
    minimum_block_tokens: usize,
    #[arg(long, default_value_t = 24)]
    maximum_blocks: usize,
    #[arg(long, default_value_t = 1_024)]
    maximum_candidates: usize,
    #[arg(long, default_value_t = 50_000)]
    maximum_postings: usize,
    #[arg(long, default_value = "plagiarism")]
    truth_tag: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone)]
struct Pair {
    suspicious_reference: String,
    source_reference: String,
}

#[derive(Debug, Serialize)]
struct PanRunReport {
    pairs: usize,
    cases: usize,
    detections: usize,
    elapsed_ms: f64,
    metrics: PanEvaluationReport,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-pan: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Evaluate(command) => run_evaluate(command),
        Command::Run(command) => run_corpus(command),
    }
}

fn run_evaluate(command: EvaluateCommand) -> CliResult<()> {
    validate_tag(&command.truth_tag)?;
    validate_tag(&command.detection_tag)?;
    let cases = read_annotations_tree(&command.truth_dir, &command.truth_tag)?;
    let detections = read_annotations_tree(&command.detections_dir, &command.detection_tag)?;
    let report = pan_evaluate(&cases, &detections)?;
    print_report(&report, command.json)?;
    Ok(())
}

fn run_corpus(command: RunCommand) -> CliResult<()> {
    validate_probability("--minimum-score", command.minimum_score)?;
    if command.minimum_block_tokens == 0
        || command.maximum_blocks == 0
        || command.maximum_candidates == 0
        || command.maximum_postings == 0
    {
        return Err(invalid_input(
            "block, candidate, and posting limits must be positive",
        ));
    }
    validate_tag(&command.truth_tag)?;
    fs::create_dir_all(&command.output)?;
    let pairs = read_pairs(&command.pairs)?;
    let truth_files = xml_files_by_name(&command.truth_dir)?;
    let started = Instant::now();
    let profile = NormalizationProfile::default();
    let mut cases = Vec::<PanAnnotation>::new();
    let mut detections = Vec::<PanAnnotation>::new();

    for pair in &pairs {
        let source_path = safe_join(&command.source_dir, &pair.source_reference)?;
        let suspicious_path = safe_join(&command.suspicious_dir, &pair.suspicious_reference)?;
        let source_text = fs::read_to_string(&source_path)?;
        let suspicious_text = fs::read_to_string(&suspicious_path)?;

        let truth_path = locate_truth_file(&truth_files, pair).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "no truth XML found for {} and {}",
                    pair.suspicious_reference, pair.source_reference
                ),
            )
        })?;
        let pair_cases = parse_pan_xml(&fs::read_to_string(truth_path)?, &command.truth_tag)?
            .into_iter()
            .filter(|case| {
                !case.is_external || case.source_reference == pair.source_reference
            })
            .collect::<Vec<_>>();
        cases.extend(pair_cases);

        let mut builder = IndexBuilder::new(IndexConfig::default())?;
        builder.add_document(pair.source_reference.clone(), &source_text)?;
        let index = builder.build()?;
        let results = index.search_composite(
            &suspicious_text,
            &SearchOptions {
                intent: SearchIntent::AnyPassage,
                max_results: 1,
                max_candidates: command.maximum_candidates,
                max_postings_per_feature: command.maximum_postings,
                minimum_similarity: command.minimum_score,
                minimum_matched_tokens: command.minimum_block_tokens,
                minimum_query_coverage: 0.0,
                minimum_source_coverage: 0.0,
                ..SearchOptions::default()
            },
            CompositeSearchOptions {
                maximum_blocks_per_document: command.maximum_blocks,
                minimum_block_tokens: command.minimum_block_tokens,
                minimum_incremental_query_tokens: command.minimum_block_tokens.min(12).max(1),
                minimum_aggregate_score: command.minimum_score,
                ..CompositeSearchOptions::default()
            },
        )?;

        let suspicious_map = normalize_with_provenance(&suspicious_text, &profile);
        let source_map = normalize_with_provenance(&source_text, &profile);
        let pair_detections = results
            .first()
            .map(|result| {
                result
                    .blocks
                    .iter()
                    .filter_map(|block| {
                        block_to_annotation(
                            block.query_start,
                            block.query_end,
                            block.corpus_start,
                            block.corpus_end,
                            &pair.suspicious_reference,
                            &pair.source_reference,
                            &suspicious_map,
                            &source_map,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let output_name = pair_output_filename(pair)?;
        write_detection_xml(
            &command.output.join(output_name),
            &pair.suspicious_reference,
            &pair_detections,
        )?;
        detections.extend(pair_detections);
    }

    let metrics = pan_evaluate(&cases, &detections)?;
    let report = PanRunReport {
        pairs: pairs.len(),
        cases: cases.len(),
        detections: detections.len(),
        elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
        metrics,
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Pairs:       {}", report.pairs);
        println!("Cases:       {}", report.cases);
        println!("Detections:  {}", report.detections);
        println!("Elapsed ms:  {:.3}", report.elapsed_ms);
        print_human_metrics(&report.metrics);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn block_to_annotation(
    query_start: usize,
    query_end: usize,
    source_start: usize,
    source_end: usize,
    suspicious_reference: &str,
    source_reference: &str,
    suspicious: &fo_core::ProvenanceNormalizedText,
    source: &fo_core::ProvenanceNormalizedText,
) -> Option<PanAnnotation> {
    let this_range = suspicious.original_range_for_tokens(query_start, query_end)?;
    let source_range = source.original_range_for_tokens(source_start, source_end)?;
    let (this_offset, this_length) = character_span(&suspicious.original, this_range)?;
    let (source_offset, source_length) = character_span(&source.original, source_range)?;
    if this_length == 0 || source_length == 0 {
        return None;
    }
    Some(PanAnnotation {
        this_reference: suspicious_reference.to_owned(),
        this_offset,
        this_length,
        source_reference: source_reference.to_owned(),
        source_offset,
        source_length,
        is_external: true,
    })
}

fn character_span(text: &str, range: OriginalByteRange) -> Option<(usize, usize)> {
    if range.start > range.end || text.get(range.start..range.end).is_none() {
        return None;
    }
    let offset = text.get(..range.start)?.chars().count();
    let length = text.get(range.start..range.end)?.chars().count();
    Some((offset, length))
}

fn read_pairs(path: &Path) -> CliResult<Vec<Pair>> {
    let input = fs::read_to_string(path)?;
    let mut pairs = Vec::new();
    for (line_index, line) in input.lines().enumerate() {
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        let fields = value.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 2 {
            return Err(invalid_input(format!(
                "{}:{} must contain exactly two filenames",
                path.display(),
                line_index + 1
            )));
        }
        validate_relative_reference(fields[0])?;
        validate_relative_reference(fields[1])?;
        pairs.push(Pair {
            suspicious_reference: fields[0].to_owned(),
            source_reference: fields[1].to_owned(),
        });
    }
    if pairs.is_empty() {
        return Err(invalid_input(format!(
            "{} contains no document pairs",
            path.display()
        )));
    }
    Ok(pairs)
}

fn read_annotations_tree(directory: &Path, tag: &str) -> CliResult<Vec<PanAnnotation>> {
    let mut files = Vec::new();
    collect_xml_files(directory, &mut files)?;
    files.sort();
    let mut annotations = Vec::new();
    for file in files {
        annotations.extend(parse_pan_xml(&fs::read_to_string(&file)?, tag)?);
    }
    Ok(annotations)
}

fn xml_files_by_name(directory: &Path) -> CliResult<BTreeMap<String, PathBuf>> {
    let mut files = Vec::new();
    collect_xml_files(directory, &mut files)?;
    let mut by_name = BTreeMap::new();
    for file in files {
        let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if by_name.insert(name.to_owned(), file.clone()).is_some() {
            return Err(invalid_input(format!(
                "truth directory contains duplicate XML filename {name}"
            )));
        }
    }
    Ok(by_name)
}

fn collect_xml_files(directory: &Path, output: &mut Vec<PathBuf>) -> io::Result<()> {
    let metadata = fs::symlink_metadata(directory)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_file() {
        if directory.extension().and_then(|extension| extension.to_str()) == Some("xml") {
            output.push(directory.to_owned());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let mut children = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    children.sort();
    for child in children {
        collect_xml_files(&child, output)?;
    }
    Ok(())
}

fn locate_truth_file<'a>(
    files: &'a BTreeMap<String, PathBuf>,
    pair: &Pair,
) -> Option<&'a PathBuf> {
    let suspicious_stem = file_stem(&pair.suspicious_reference)?;
    let source_stem = file_stem(&pair.source_reference)?;
    let paired = format!("{suspicious_stem}-{source_stem}.xml");
    files.get(&paired).or_else(|| {
        let suspicious = format!("{suspicious_stem}.xml");
        files.get(&suspicious)
    })
}

fn pair_output_filename(pair: &Pair) -> CliResult<String> {
    let suspicious = file_stem(&pair.suspicious_reference)
        .ok_or_else(|| invalid_input("suspicious filename has no valid stem"))?;
    let source = file_stem(&pair.source_reference)
        .ok_or_else(|| invalid_input("source filename has no valid stem"))?;
    Ok(format!("{suspicious}-{source}.xml"))
}

fn file_stem(reference: &str) -> Option<&str> {
    Path::new(reference).file_stem()?.to_str()
}

fn parse_pan_xml(xml: &str, tag_name: &str) -> CliResult<Vec<PanAnnotation>> {
    let document_start = xml
        .find("<document")
        .ok_or_else(|| invalid_input("PAN XML has no <document> element"))?;
    let document_end = xml[document_start..]
        .find('>')
        .map(|offset| document_start + offset + 1)
        .ok_or_else(|| invalid_input("PAN XML has an unterminated <document> element"))?;
    let document_attributes = parse_attributes(&xml[document_start..document_end])?;
    let this_reference = document_attributes
        .get("reference")
        .cloned()
        .ok_or_else(|| invalid_input("PAN document element has no reference attribute"))?;

    let mut annotations = Vec::new();
    let mut cursor = document_end;
    while let Some(relative) = xml[cursor..].find("<feature") {
        let start = cursor + relative;
        let end = xml[start..]
            .find('>')
            .map(|offset| start + offset + 1)
            .ok_or_else(|| invalid_input("PAN XML has an unterminated <feature> element"))?;
        let attributes = parse_attributes(&xml[start..end])?;
        cursor = end;
        if !attributes
            .get("name")
            .is_some_and(|name| name.ends_with(tag_name))
        {
            continue;
        }
        let this_offset = parse_required_usize(&attributes, "this_offset")?;
        let this_length = parse_required_usize(&attributes, "this_length")?;
        let external = attributes.contains_key("source_reference")
            && attributes.contains_key("source_offset")
            && attributes.contains_key("source_length");
        let annotation = PanAnnotation {
            this_reference: this_reference.clone(),
            this_offset,
            this_length,
            source_reference: attributes
                .get("source_reference")
                .cloned()
                .unwrap_or_default(),
            source_offset: if external {
                parse_required_usize(&attributes, "source_offset")?
            } else {
                0
            },
            source_length: if external {
                parse_required_usize(&attributes, "source_length")?
            } else {
                0
            },
            is_external: external,
        };
        annotation.validate()?;
        annotations.push(annotation);
    }
    Ok(annotations)
}

fn parse_attributes(tag: &str) -> CliResult<BTreeMap<String, String>> {
    let bytes = tag.as_bytes();
    let mut cursor = 1usize;
    while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() && bytes[cursor] != b'>' {
        cursor += 1;
    }
    let mut attributes = BTreeMap::new();
    while cursor < bytes.len() {
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_whitespace() || bytes[cursor] == b'/')
        {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] == b'>' {
            break;
        }
        let name_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric()
                || matches!(bytes[cursor], b'_' | b'-' | b':' | b'.'))
        {
            cursor += 1;
        }
        if cursor == name_start {
            return Err(invalid_input("invalid XML attribute name"));
        }
        let name = std::str::from_utf8(&bytes[name_start..cursor])?.to_owned();
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            return Err(invalid_input(format!("XML attribute {name} has no '='")));
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let quote = *bytes
            .get(cursor)
            .ok_or_else(|| invalid_input(format!("XML attribute {name} has no value")))?;
        if quote != b'\'' && quote != b'"' {
            return Err(invalid_input(format!(
                "XML attribute {name} is not quoted"
            )));
        }
        cursor += 1;
        let value_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != quote {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            return Err(invalid_input(format!(
                "XML attribute {name} has an unterminated value"
            )));
        }
        let value = std::str::from_utf8(&bytes[value_start..cursor])?;
        attributes.insert(name, xml_unescape(value)?);
        cursor += 1;
    }
    Ok(attributes)
}

fn xml_unescape(value: &str) -> CliResult<String> {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(position) = remaining.find('&') {
        output.push_str(&remaining[..position]);
        remaining = &remaining[position..];
        let Some(end) = remaining.find(';') else {
            return Err(invalid_input("unterminated XML entity"));
        };
        let entity = &remaining[1..end];
        match entity {
            "amp" => output.push('&'),
            "lt" => output.push('<'),
            "gt" => output.push('>'),
            "quot" => output.push('"'),
            "apos" => output.push('\''),
            _ => {
                let number = entity
                    .strip_prefix("#x")
                    .and_then(|digits| u32::from_str_radix(digits, 16).ok())
                    .or_else(|| {
                        entity
                            .strip_prefix('#')
                            .and_then(|digits| digits.parse::<u32>().ok())
                    })
                    .and_then(char::from_u32)
                    .ok_or_else(|| invalid_input(format!("unsupported XML entity &{entity};")))?;
                output.push(number);
            }
        }
        remaining = &remaining[end + 1..];
    }
    output.push_str(remaining);
    Ok(output)
}

fn parse_required_usize(
    attributes: &BTreeMap<String, String>,
    name: &str,
) -> CliResult<usize> {
    attributes
        .get(name)
        .ok_or_else(|| invalid_input(format!("PAN feature has no {name} attribute")))?
        .parse::<usize>()
        .map_err(|error| invalid_input(format!("invalid {name}: {error}")))
}

fn write_detection_xml(
    path: &Path,
    suspicious_reference: &str,
    detections: &[PanAnnotation],
) -> CliResult<()> {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(&format!(
        "<document reference=\"{}\">\n",
        xml_escape(suspicious_reference)
    ));
    for detection in detections {
        xml.push_str(&format!(
            "  <feature name=\"detected-plagiarism\" this_offset=\"{}\" this_length=\"{}\" source_reference=\"{}\" source_offset=\"{}\" source_length=\"{}\" />\n",
            detection.this_offset,
            detection.this_length,
            xml_escape(&detection.source_reference),
            detection.source_offset,
            detection.source_length,
        ));
    }
    xml.push_str("</document>\n");
    fs::write(path, xml)?;
    Ok(())
}

fn xml_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            _ => output.push(ch),
        }
    }
    output
}

fn safe_join(root: &Path, reference: &str) -> CliResult<PathBuf> {
    validate_relative_reference(reference)?;
    Ok(root.join(reference))
}

fn validate_relative_reference(reference: &str) -> CliResult<()> {
    let path = Path::new(reference);
    if reference.is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(invalid_input(format!(
            "unsafe PAN document reference {reference:?}"
        )));
    }
    Ok(())
}

fn validate_probability(name: &str, value: f32) -> CliResult<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(invalid_input(format!("{name} must lie in [0, 1]")));
    }
    Ok(())
}

fn validate_tag(tag: &str) -> CliResult<()> {
    if tag.is_empty()
        || !tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid_input(format!("invalid feature tag {tag:?}")));
    }
    Ok(())
}

fn print_report(report: &PanEvaluationReport, json: bool) -> CliResult<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        print_human_metrics(report);
    }
    Ok(())
}

fn print_human_metrics(report: &PanEvaluationReport) {
    println!("Cases:           {}", report.cases);
    println!("Detections:      {}", report.detections);
    println!("Macro recall:    {:.6}", report.macro_recall);
    println!("Macro precision: {:.6}", report.macro_precision);
    println!("Macro F1:        {:.6}", report.macro_f1);
    println!("Micro recall:    {:.6}", report.micro_recall);
    println!("Micro precision: {:.6}", report.micro_precision);
    println!("Micro F1:        {:.6}", report.micro_f1);
    println!("Granularity:     {:.6}", report.granularity);
    println!("Macro PlagDet:   {:.6}", report.macro_plagdet);
    println!("Micro PlagDet:   {:.6}", report.micro_plagdet);
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

#[cfg(test)]
mod tests {
    use super::{character_span, parse_pan_xml, xml_escape};
    use fo_core::OriginalByteRange;

    #[test]
    fn parses_external_pan_features() {
        let annotations = parse_pan_xml(
            r#"<document reference="suspicious.txt">
                <feature name="plagiarism" this_offset="5" this_length="10"
                  source_reference="source.txt" source_offset="20" source_length="12" />
            </document>"#,
            "plagiarism",
        )
        .expect("parse");
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].this_offset, 5);
        assert_eq!(annotations[0].source_reference, "source.txt");
    }

    #[test]
    fn converts_utf8_bytes_to_pan_character_offsets() {
        let text = "aé中z";
        let range = OriginalByteRange { start: 1, end: 6 };
        assert_eq!(character_span(text, range), Some((1, 2)));
    }

    #[test]
    fn escapes_xml_attributes() {
        assert_eq!(xml_escape("a&\"<b>"), "a&amp;&quot;&lt;b&gt;");
    }
}
