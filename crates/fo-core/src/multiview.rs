use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::{
    FoError, Index, IndexBuilder, IndexConfig, IndexStats, NormalizationProfile, Result,
    SearchIntent, SearchOptions, SearchResult,
};

const MULTIVIEW_FORMAT_VERSION: u32 = 1;
const MANIFEST_FILE: &str = "manifest.json";
static TEMPORARY_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureViewConfig {
    pub name: String,
    pub qgram_size: usize,
    pub winnow_window: usize,
    pub weight: f32,
    pub normalization: NormalizationProfile,
}

impl FeatureViewConfig {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        qgram_size: usize,
        winnow_window: usize,
        weight: f32,
        normalization: NormalizationProfile,
    ) -> Self {
        Self {
            name: name.into(),
            qgram_size,
            winnow_window,
            weight,
            normalization,
        }
    }

    fn index_config(&self) -> IndexConfig {
        IndexConfig {
            normalization: self.normalization.clone(),
            qgram_size: self.qgram_size,
            winnow_window: self.winnow_window,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || self.name.len() > 128 {
            return Err(FoError::InvalidConfig(
                "feature-view names must contain between 1 and 128 bytes".to_owned(),
            ));
        }
        if !self.weight.is_finite() || self.weight <= 0.0 || self.weight > 100.0 {
            return Err(FoError::InvalidConfig(format!(
                "feature view {} has invalid weight {}",
                self.name, self.weight
            )));
        }
        self.index_config().validate()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiViewConfig {
    pub views: Vec<FeatureViewConfig>,
    pub minimum_view_support: usize,
    pub consensus_overlap_fraction: f32,
    pub per_view_candidate_multiplier: usize,
}

impl Default for MultiViewConfig {
    fn default() -> Self {
        Self::balanced()
    }
}

impl MultiViewConfig {
    #[must_use]
    pub fn balanced() -> Self {
        let normalization = NormalizationProfile::default();
        Self {
            views: vec![
                FeatureViewConfig::new("char-5", 5, 8, 0.85, normalization.clone()),
                FeatureViewConfig::new("char-7", 7, 12, 1.00, normalization.clone()),
                FeatureViewConfig::new("char-11", 11, 16, 1.15, normalization),
            ],
            minimum_view_support: 2,
            consensus_overlap_fraction: 0.55,
            per_view_candidate_multiplier: 8,
        }
    }

    #[must_use]
    pub fn high_recall() -> Self {
        let normalization = NormalizationProfile::default();
        Self {
            views: vec![
                FeatureViewConfig::new("char-4", 4, 6, 0.80, normalization.clone()),
                FeatureViewConfig::new("char-6", 6, 9, 1.00, normalization.clone()),
                FeatureViewConfig::new("char-8", 8, 12, 1.10, normalization),
            ],
            minimum_view_support: 1,
            consensus_overlap_fraction: 0.45,
            per_view_candidate_multiplier: 12,
        }
    }

    #[must_use]
    pub fn high_precision() -> Self {
        let normalization = NormalizationProfile::default();
        Self {
            views: vec![
                FeatureViewConfig::new("char-7", 7, 12, 0.90, normalization.clone()),
                FeatureViewConfig::new("char-11", 11, 16, 1.05, normalization.clone()),
                FeatureViewConfig::new("char-15", 15, 20, 1.20, normalization),
            ],
            minimum_view_support: 2,
            consensus_overlap_fraction: 0.65,
            per_view_candidate_multiplier: 6,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.views.is_empty() || self.views.len() > 32 {
            return Err(FoError::InvalidConfig(
                "multi-view indexes require between 1 and 32 views".to_owned(),
            ));
        }
        if self.minimum_view_support == 0 || self.minimum_view_support > self.views.len() {
            return Err(FoError::InvalidConfig(format!(
                "minimum_view_support must lie in 1..={}",
                self.views.len()
            )));
        }
        if !self.consensus_overlap_fraction.is_finite()
            || !(0.0..=1.0).contains(&self.consensus_overlap_fraction)
        {
            return Err(FoError::InvalidConfig(
                "consensus_overlap_fraction must be finite and lie in [0, 1]".to_owned(),
            ));
        }
        if self.per_view_candidate_multiplier == 0 || self.per_view_candidate_multiplier > 1024 {
            return Err(FoError::InvalidConfig(
                "per_view_candidate_multiplier must be between 1 and 1024".to_owned(),
            ));
        }
        let normalization = &self.views[0].normalization;
        let mut names = HashSet::with_capacity(self.views.len());
        for view in &self.views {
            view.validate()?;
            if &view.normalization != normalization {
                return Err(FoError::InvalidConfig(
                    "all multi-view feature scales must share one normalization profile so spans remain comparable"
                        .to_owned(),
                ));
            }
            if !names.insert(view.name.as_str()) {
                return Err(FoError::InvalidConfig(format!(
                    "duplicate feature-view name {}",
                    view.name
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewEvidence {
    pub view_name: String,
    pub view_weight: f32,
    pub raw_score: f32,
    pub edit_similarity: f32,
    pub query_coverage: f32,
    pub source_coverage: f32,
    pub chain_consistency: f32,
    pub matched_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiViewSearchResult {
    pub representative: SearchResult,
    pub fused_score: f32,
    pub view_support: usize,
    pub total_views: usize,
    pub support_ratio: f32,
    pub score_disagreement: f32,
    pub weighted_edit_similarity: f32,
    pub weighted_query_coverage: f32,
    pub weighted_source_coverage: f32,
    pub matched_tokens: usize,
    pub evidence: Vec<ViewEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiViewViewStats {
    pub name: String,
    pub weight: f32,
    pub config: IndexConfig,
    pub stats: IndexStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiViewStats {
    pub views: usize,
    pub documents: usize,
    pub total_postings: usize,
    pub view_stats: Vec<MultiViewViewStats>,
}

#[derive(Debug, Clone)]
struct NamedIndex {
    config: FeatureViewConfig,
    index: Index,
}

#[derive(Debug, Clone)]
pub struct MultiViewIndex {
    config: MultiViewConfig,
    views: Vec<NamedIndex>,
}

impl MultiViewIndex {
    #[must_use]
    pub fn config(&self) -> &MultiViewConfig {
        &self.config
    }

    #[must_use]
    pub fn stats(&self) -> MultiViewStats {
        let view_stats = self
            .views
            .iter()
            .map(|view| MultiViewViewStats {
                name: view.config.name.clone(),
                weight: view.config.weight,
                config: view.index.config.clone(),
                stats: view.index.stats(),
            })
            .collect::<Vec<_>>();
        MultiViewStats {
            views: view_stats.len(),
            documents: view_stats.first().map_or(0, |view| view.stats.documents),
            total_postings: view_stats.iter().map(|view| view.stats.postings).sum(),
            view_stats,
        }
    }

    pub fn save(&self, directory: impl AsRef<Path>) -> Result<()> {
        self.config.validate()?;
        let directory = directory.as_ref();
        fs::create_dir_all(directory).map_err(|error| FoError::io(directory, error))?;
        let mut files = Vec::with_capacity(self.views.len());
        for (view_index, view) in self.views.iter().enumerate() {
            let filename = format!("view-{view_index:03}.foidx");
            view.index.save(directory.join(&filename))?;
            files.push(filename);
        }
        let manifest = MultiViewManifest {
            format_version: MULTIVIEW_FORMAT_VERSION,
            config: self.config.clone(),
            files,
        };
        let bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| {
            FoError::InvalidConfig(format!("could not serialize multi-view manifest: {error}"))
        })?;
        atomic_write(&directory.join(MANIFEST_FILE), &bytes)
    }

    pub fn load(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref();
        let manifest_path = directory.join(MANIFEST_FILE);
        let bytes = fs::read(&manifest_path).map_err(|error| FoError::io(&manifest_path, error))?;
        let manifest = serde_json::from_slice::<MultiViewManifest>(&bytes).map_err(|error| {
            FoError::InvalidIndex(format!("invalid multi-view manifest: {error}"))
        })?;
        if manifest.format_version != MULTIVIEW_FORMAT_VERSION {
            return Err(FoError::InvalidIndex(format!(
                "unsupported multi-view format version {}",
                manifest.format_version
            )));
        }
        manifest.config.validate()?;
        if manifest.files.len() != manifest.config.views.len() {
            return Err(FoError::InvalidIndex(format!(
                "manifest has {} files for {} configured views",
                manifest.files.len(),
                manifest.config.views.len()
            )));
        }
        let mut views = Vec::with_capacity(manifest.files.len());
        for (view_config, filename) in manifest.config.views.iter().zip(&manifest.files) {
            if filename.contains('/')
                || filename.contains('\\')
                || filename == "."
                || filename == ".."
            {
                return Err(FoError::InvalidIndex(format!(
                    "invalid multi-view filename {filename:?}"
                )));
            }
            let index = Index::load(directory.join(filename))?;
            if index.config != view_config.index_config() {
                return Err(FoError::InvalidIndex(format!(
                    "view {} configuration disagrees with its index file",
                    view_config.name
                )));
            }
            views.push(NamedIndex {
                config: view_config.clone(),
                index,
            });
        }
        validate_document_identity(&views)?;
        Ok(Self {
            config: manifest.config,
            views,
        })
    }

    pub fn search(
        &self,
        specimen: &str,
        options: &SearchOptions,
    ) -> Result<Vec<MultiViewSearchResult>> {
        options.validate()?;
        self.config.validate()?;
        let mut clusters = HashMap::<u32, Vec<HitCluster>>::new();
        for view in &self.views {
            let mut view_options = options.clone();
            view_options.intent = SearchIntent::AnyPassage;
            view_options.max_results = options
                .max_candidates
                .max(
                    options
                        .max_results
                        .saturating_mul(self.config.per_view_candidate_multiplier),
                );
            view_options.max_candidates = view_options.max_candidates.max(view_options.max_results);
            view_options.minimum_similarity = options.minimum_similarity.min(0.15);
            view_options.minimum_matched_tokens = options
                .minimum_matched_tokens
                .min(view.config.qgram_size.saturating_mul(2))
                .max(view.config.qgram_size);
            view_options.minimum_query_coverage = 0.0;
            view_options.minimum_source_coverage = 0.0;

            for hit in view.index.search(specimen, &view_options)? {
                add_hit_to_clusters(
                    &mut clusters,
                    hit,
                    &view.config,
                    self.config.consensus_overlap_fraction,
                );
            }
        }

        let mut results = Vec::new();
        for document_clusters in clusters.into_values() {
            for cluster in document_clusters {
                if let Some(result) = fuse_cluster(cluster, &self.config, options) {
                    results.push(result);
                }
            }
        }
        results.sort_unstable_by(|left, right| {
            right
                .fused_score
                .total_cmp(&left.fused_score)
                .then_with(|| right.view_support.cmp(&left.view_support))
                .then_with(|| {
                    right
                        .weighted_query_coverage
                        .total_cmp(&left.weighted_query_coverage)
                })
                .then_with(|| {
                    left.representative
                        .document_id
                        .cmp(&right.representative.document_id)
                })
                .then_with(|| {
                    left.representative
                        .corpus_start
                        .cmp(&right.representative.corpus_start)
                })
        });
        results.truncate(options.max_results);
        Ok(results)
    }
}

#[derive(Debug)]
pub struct MultiViewIndexBuilder {
    config: MultiViewConfig,
    documents: Vec<(String, String)>,
}

impl MultiViewIndexBuilder {
    pub fn new(config: MultiViewConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            documents: Vec::new(),
        })
    }

    pub fn add_document(
        &mut self,
        path: impl Into<String>,
        contents: impl Into<String>,
    ) -> Result<u32> {
        if self.documents.len() >= u32::MAX as usize {
            return Err(FoError::TooManyDocuments);
        }
        let id = self.documents.len() as u32;
        self.documents.push((path.into(), contents.into()));
        Ok(id)
    }

    pub fn build(self) -> Result<MultiViewIndex> {
        let mut views = Vec::with_capacity(self.config.views.len());
        for view_config in &self.config.views {
            let mut builder = IndexBuilder::new(view_config.index_config())?;
            for (path, contents) in &self.documents {
                builder.add_document(path.clone(), contents)?;
            }
            views.push(NamedIndex {
                config: view_config.clone(),
                index: builder.build()?,
            });
        }
        validate_document_identity(&views)?;
        Ok(MultiViewIndex {
            config: self.config,
            views,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MultiViewManifest {
    format_version: u32,
    config: MultiViewConfig,
    files: Vec<String>,
}

#[derive(Debug)]
struct HitCluster {
    representative: SearchResult,
    evidence: Vec<ViewEvidence>,
}

fn add_hit_to_clusters(
    clusters: &mut HashMap<u32, Vec<HitCluster>>,
    hit: SearchResult,
    view: &FeatureViewConfig,
    overlap_threshold: f32,
) {
    let document_clusters = clusters.entry(hit.document_id).or_default();
    let matching = document_clusters.iter().position(|cluster| {
        overlap_fraction(
            (hit.query_start, hit.query_end),
            (
                cluster.representative.query_start,
                cluster.representative.query_end,
            ),
        ) >= overlap_threshold
            && overlap_fraction(
                (hit.corpus_start, hit.corpus_end),
                (
                    cluster.representative.corpus_start,
                    cluster.representative.corpus_end,
                ),
            ) >= overlap_threshold
    });
    let evidence = ViewEvidence {
        view_name: view.name.clone(),
        view_weight: view.weight,
        raw_score: hit.combined_score,
        edit_similarity: hit.edit_similarity,
        query_coverage: hit.query_coverage,
        source_coverage: hit.source_coverage,
        chain_consistency: hit.chain_consistency,
        matched_tokens: hit.matched_tokens,
    };
    if let Some(cluster_index) = matching {
        let cluster = &mut document_clusters[cluster_index];
        if let Some(existing) = cluster
            .evidence
            .iter_mut()
            .find(|existing| existing.view_name == view.name)
        {
            if evidence.raw_score > existing.raw_score {
                *existing = evidence;
            }
        } else {
            cluster.evidence.push(evidence);
        }
        if hit.combined_score > cluster.representative.combined_score {
            cluster.representative = hit;
        }
    } else {
        document_clusters.push(HitCluster {
            representative: hit,
            evidence: vec![evidence],
        });
    }
}

fn fuse_cluster(
    mut cluster: HitCluster,
    config: &MultiViewConfig,
    options: &SearchOptions,
) -> Option<MultiViewSearchResult> {
    cluster
        .evidence
        .sort_unstable_by(|left, right| left.view_name.cmp(&right.view_name));
    let view_support = cluster.evidence.len();
    if view_support < config.minimum_view_support {
        return None;
    }
    let total_weight = cluster
        .evidence
        .iter()
        .map(|evidence| evidence.view_weight)
        .sum::<f32>();
    if !total_weight.is_finite() || total_weight <= 0.0 {
        return None;
    }
    let weighted = |selector: fn(&ViewEvidence) -> f32| {
        cluster
            .evidence
            .iter()
            .map(|evidence| evidence.view_weight * selector(evidence))
            .sum::<f32>()
            / total_weight
    };
    let weighted_raw_score = weighted(|evidence| evidence.raw_score);
    let weighted_edit_similarity = weighted(|evidence| evidence.edit_similarity);
    let weighted_query_coverage = weighted(|evidence| evidence.query_coverage);
    let weighted_source_coverage = weighted(|evidence| evidence.source_coverage);
    let weighted_chain_consistency = weighted(|evidence| evidence.chain_consistency);
    let matched_tokens = cluster
        .evidence
        .iter()
        .map(|evidence| evidence.matched_tokens)
        .max()
        .unwrap_or(0);
    let variance = cluster
        .evidence
        .iter()
        .map(|evidence| {
            evidence.view_weight * (evidence.raw_score - weighted_raw_score).powi(2)
        })
        .sum::<f32>()
        / total_weight;
    let score_disagreement = variance.max(0.0).sqrt().clamp(0.0, 1.0);
    let consensus = (1.0 - 2.0 * score_disagreement).clamp(0.0, 1.0);
    let support_ratio = view_support as f32 / config.views.len() as f32;
    let evidence_confidence = (1.0
        / (1.0 + cluster.representative.estimated_false_matches.max(0.0)))
        as f32;
    let base = 0.55 * weighted_raw_score
        + 0.16 * weighted_edit_similarity
        + 0.14 * weighted_query_coverage
        + 0.08 * weighted_chain_consistency
        + 0.07 * evidence_confidence;
    let fused_score =
        (base * (0.68 + 0.22 * support_ratio + 0.10 * consensus)).clamp(0.0, 1.0);
    if fused_score < options.minimum_similarity {
        return None;
    }
    let representative_query_span = cluster
        .representative
        .query_end
        .saturating_sub(cluster.representative.query_start)
        .max(1);
    if matched_tokens < options.minimum_matched_tokens.min(representative_query_span) {
        return None;
    }
    match options.intent {
        SearchIntent::AnyPassage => {}
        SearchIntent::SourceAttribution => {
            if weighted_query_coverage < options.minimum_query_coverage {
                return None;
            }
        }
        SearchIntent::NearDuplicate => {
            if weighted_query_coverage < options.minimum_query_coverage
                || weighted_source_coverage < options.minimum_source_coverage
            {
                return None;
            }
        }
    }
    cluster.representative.intent = options.intent;
    Some(MultiViewSearchResult {
        representative: cluster.representative,
        fused_score,
        view_support,
        total_views: config.views.len(),
        support_ratio,
        score_disagreement,
        weighted_edit_similarity,
        weighted_query_coverage,
        weighted_source_coverage,
        matched_tokens,
        evidence: cluster.evidence,
    })
}

fn overlap_fraction(left: (usize, usize), right: (usize, usize)) -> f32 {
    let intersection = left.1.min(right.1).saturating_sub(left.0.max(right.0));
    let shorter = left
        .1
        .saturating_sub(left.0)
        .min(right.1.saturating_sub(right.0));
    if shorter == 0 {
        0.0
    } else {
        intersection as f32 / shorter as f32
    }
}

fn validate_document_identity(views: &[NamedIndex]) -> Result<()> {
    let Some(first) = views.first() else {
        return Err(FoError::InvalidIndex(
            "multi-view index contains no views".to_owned(),
        ));
    };
    for view in views.iter().skip(1) {
        if view.index.documents().len() != first.index.documents().len() {
            return Err(FoError::InvalidIndex(format!(
                "view {} has a different document count",
                view.config.name
            )));
        }
        for (left, right) in first.index.documents().iter().zip(view.index.documents()) {
            if left.id != right.id
                || left.path != right.path
                || left.normalized.tokens.len() != right.normalized.tokens.len()
            {
                return Err(FoError::InvalidIndex(format!(
                    "view {} has inconsistent document identity or normalized coordinates",
                    view.config.name
                )));
            }
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = temporary_path(path);
    fs::write(&temporary, bytes).map_err(|error| FoError::io(&temporary, error))?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() => {
            fs::remove_file(path).map_err(|remove_error| FoError::io(path, remove_error))?;
            fs::rename(&temporary, path).map_err(|rename_error| FoError::io(path, rename_error))
        }
        Err(error) => Err(FoError::io(path, error)),
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut filename = path
        .file_name()
        .map_or_else(|| "manifest".into(), |name| name.to_os_string());
    let sequence = TEMPORARY_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    filename.push(format!(".tmp-{}-{sequence}", std::process::id()));
    path.with_file_name(filename)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{MultiViewConfig, MultiViewIndex, MultiViewIndexBuilder};
    use crate::{SearchIntent, SearchOptions};

    #[test]
    fn consensus_views_rank_the_edited_source() {
        let mut builder =
            MultiViewIndexBuilder::new(MultiViewConfig::balanced()).expect("builder");
        builder
            .add_document(
                "source.txt",
                "before dawn the observatory opened copper shutters checked every instrument and published raw measurements",
            )
            .expect("source");
        builder
            .add_document(
                "noise.txt",
                "winter vegetables simmer in a ceramic pot beside a railway timetable and orchard map",
            )
            .expect("noise");
        let index = builder.build().expect("index");
        let results = index
            .search(
                "the observatory opened copper shutters before dawn checked every instrument and published raw measurements",
                &SearchOptions {
                    intent: SearchIntent::SourceAttribution,
                    minimum_similarity: 0.10,
                    minimum_query_coverage: 0.15,
                    minimum_matched_tokens: 8,
                    ..SearchOptions::default()
                },
            )
            .expect("search");
        assert!(!results.is_empty(), "{results:#?}");
        assert_eq!(results[0].representative.path, "source.txt");
        assert!(results[0].view_support >= 2);
    }

    #[test]
    fn persistence_preserves_consensus_results() {
        let text = "preserve every raw observation and document each transformation before comparing causal models";
        let mut builder =
            MultiViewIndexBuilder::new(MultiViewConfig::balanced()).expect("builder");
        builder.add_document("source.txt", text).expect("source");
        let index = builder.build().expect("index");
        let path = temporary_directory();
        index.save(&path).expect("save");
        let loaded = MultiViewIndex::load(&path).expect("load");
        let options = SearchOptions {
            minimum_similarity: 0.10,
            minimum_query_coverage: 0.10,
            minimum_matched_tokens: 8,
            ..SearchOptions::default()
        };
        let before = index.search(text, &options).expect("before");
        let after = loaded.search(text, &options).expect("after");
        fs::remove_dir_all(path).ok();
        assert_eq!(before.len(), after.len());
        assert_eq!(before[0].representative.path, after[0].representative.path);
        assert!((before[0].fused_score - after[0].fused_score).abs() < 1e-6);
    }

    fn temporary_directory() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "franken-overlap-multiview-{}-{nonce}",
            std::process::id()
        ))
    }
}
