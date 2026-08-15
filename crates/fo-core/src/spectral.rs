#[cfg(any(feature = "frankenscipy", test))]
use std::f64::consts::TAU;

use serde::{Deserialize, Serialize};

#[cfg(any(feature = "frankenscipy", test))]
use crate::fingerprint::categorical_hash;
use crate::{FoError, NormalizationProfile, Result, normalize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpectralOptions {
    pub repetitions: usize,
    /// Number of quantized unit-circle phases used by the FFT sketch.
    ///
    /// This field retains its original name for serialization compatibility.
    pub buckets: usize,
    pub max_results: usize,
    pub minimum_score: f32,
    pub local_maximum_radius: usize,
    /// Maximum exact token comparisons before the FFT backend is required.
    pub direct_work_limit: u64,
}

impl Default for SpectralOptions {
    fn default() -> Self {
        Self {
            repetitions: 4,
            buckets: 8,
            max_results: 20,
            minimum_score: 0.25,
            local_maximum_radius: 16,
            direct_work_limit: 250_000_000,
        }
    }
}

impl SpectralOptions {
    pub fn validate(&self) -> Result<()> {
        if !(1..=32).contains(&self.repetitions) {
            return Err(FoError::InvalidConfig(
                "spectral repetitions must be between 1 and 32".to_owned(),
            ));
        }
        if !(2..=4096).contains(&self.buckets) {
            return Err(FoError::InvalidConfig(
                "spectral phase buckets must be between 2 and 4096".to_owned(),
            ));
        }
        if self.max_results == 0 {
            return Err(FoError::InvalidConfig(
                "spectral max_results must be positive".to_owned(),
            ));
        }
        if !self.minimum_score.is_finite() || !(-1.0..=1.0).contains(&self.minimum_score) {
            return Err(FoError::InvalidConfig(
                "spectral minimum_score must be finite and lie in [-1, 1]".to_owned(),
            ));
        }
        if self.direct_work_limit == 0 {
            return Err(FoError::InvalidConfig(
                "spectral direct_work_limit must be positive".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpectralPeak {
    pub offset: usize,
    pub end: usize,
    pub score: f32,
    pub matched_text: String,
}

/// Dense categorical cross-correlation.
///
/// Small and medium workloads are evaluated exactly: the score at each offset is
/// the fraction of equal categorical tokens. Larger workloads use independently
/// hashed unit-circle phases. Equal categories contribute exactly one per
/// repetition; unequal categories have zero expected dot product. The phase
/// representation needs two real correlations per repetition instead of one
/// correlation per CountSketch bucket.
#[allow(clippy::missing_errors_doc)]
pub fn spectral_scan(
    corpus: &str,
    specimen: &str,
    profile: &NormalizationProfile,
    options: &SpectralOptions,
) -> Result<Vec<SpectralPeak>> {
    options.validate()?;
    let corpus = normalize(corpus, profile);
    let specimen = normalize(specimen, profile);
    if specimen.is_empty() {
        return Err(FoError::EmptySpecimen);
    }
    if corpus.len() < specimen.len() {
        return Ok(Vec::new());
    }

    let direct_work = comparison_work(corpus.len(), specimen.len());
    let scores = if direct_work <= options.direct_work_limit as u128 {
        exact_direct_scores(&corpus.tokens, &specimen.tokens)
    } else {
        #[cfg(feature = "frankenscipy")]
        {
            phase_fft_scores(&corpus.tokens, &specimen.tokens, options)?
        }
        #[cfg(not(feature = "frankenscipy"))]
        {
            return Err(FoError::Spectral(format!(
                "exact dense workload {direct_work} exceeds limit {}; rebuild with --features \
                 frankenscipy or raise direct_work_limit",
                options.direct_work_limit
            )));
        }
    };

    Ok(extract_peaks(
        &scores,
        &corpus,
        specimen.len(),
        options,
    ))
}

fn comparison_work(corpus_length: usize, specimen_length: usize) -> u128 {
    if corpus_length < specimen_length || specimen_length == 0 {
        return 0;
    }
    (corpus_length - specimen_length + 1) as u128 * specimen_length as u128
}

fn exact_direct_scores(corpus: &[u32], specimen: &[u32]) -> Vec<f32> {
    let offsets = corpus.len() - specimen.len() + 1;
    let denominator = specimen.len().max(1) as f32;
    let mut scores = vec![0.0f32; offsets];
    for (offset, score) in scores.iter_mut().enumerate() {
        let matches = specimen
            .iter()
            .zip(&corpus[offset..offset + specimen.len()])
            .filter(|(left, right)| left == right)
            .count();
        *score = matches as f32 / denominator;
    }
    scores
}

#[cfg(feature = "frankenscipy")]
fn phase_fft_scores(
    corpus: &[u32],
    specimen: &[u32],
    options: &SpectralOptions,
) -> Result<Vec<f32>> {
    let offsets = corpus.len() - specimen.len() + 1;
    let mut accumulated = vec![0.0f64; offsets];
    for repetition in 0..options.repetitions {
        let corpus_cos = corpus
            .iter()
            .map(|&token| phase_components(token, repetition, options.buckets).0)
            .collect::<Vec<_>>();
        let corpus_sin = corpus
            .iter()
            .map(|&token| phase_components(token, repetition, options.buckets).1)
            .collect::<Vec<_>>();
        let specimen_cos = specimen
            .iter()
            .map(|&token| phase_components(token, repetition, options.buckets).0)
            .collect::<Vec<_>>();
        let specimen_sin = specimen
            .iter()
            .map(|&token| phase_components(token, repetition, options.buckets).1)
            .collect::<Vec<_>>();

        let cosine = fsci_fft::fftcorrelate(&corpus_cos, &specimen_cos, "valid")
            .map_err(|error| FoError::Spectral(error.to_string()))?;
        let sine = fsci_fft::fftcorrelate(&corpus_sin, &specimen_sin, "valid")
            .map_err(|error| FoError::Spectral(error.to_string()))?;
        if cosine.len() != offsets || sine.len() != offsets {
            return Err(FoError::Spectral(format!(
                "FrankenSciPy returned phase channels of lengths {} and {}; expected {offsets}",
                cosine.len(),
                sine.len()
            )));
        }
        for ((sum, cosine_value), sine_value) in accumulated
            .iter_mut()
            .zip(cosine)
            .zip(sine)
        {
            *sum += cosine_value + sine_value;
        }
    }
    let denominator = specimen.len() as f64 * options.repetitions as f64;
    Ok(accumulated
        .into_iter()
        .map(|score| (score / denominator).clamp(-1.0, 1.0) as f32)
        .collect())
}

#[cfg(any(feature = "frankenscipy", test))]
fn phase_components(token: u32, repetition: usize, phase_count: usize) -> (f64, f64) {
    let phase = categorical_hash(token, repetition) % phase_count as u64;
    let angle = TAU * phase as f64 / phase_count as f64;
    let (sine, cosine) = angle.sin_cos();
    (cosine, sine)
}

fn extract_peaks(
    scores: &[f32],
    corpus: &crate::NormalizedText,
    specimen_length: usize,
    options: &SpectralOptions,
) -> Vec<SpectralPeak> {
    let mut peaks = Vec::new();
    for (offset, &score) in scores.iter().enumerate() {
        if score < options.minimum_score || !score.is_finite() {
            continue;
        }
        let left = offset.saturating_sub(options.local_maximum_radius);
        let right = offset
            .saturating_add(options.local_maximum_radius)
            .saturating_add(1)
            .min(scores.len());
        if scores[left..right]
            .iter()
            .enumerate()
            .any(|(relative, &other)| {
                let other_offset = left + relative;
                other > score || (other == score && other_offset < offset)
            })
        {
            continue;
        }
        let end = offset + specimen_length;
        peaks.push(SpectralPeak {
            offset,
            end,
            score,
            matched_text: corpus.slice_tokens(offset, end).to_owned(),
        });
    }
    peaks.sort_unstable_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.offset.cmp(&right.offset))
    });
    peaks.truncate(options.max_results);
    peaks
}

#[cfg(test)]
mod tests {
    use super::{
        SpectralOptions, exact_direct_scores, phase_components, spectral_scan,
    };
    use crate::NormalizationProfile;

    #[test]
    fn exact_occurrence_has_a_unit_peak() {
        let peaks = spectral_scan(
            "zero one two three four five",
            "two three four",
            &NormalizationProfile::default(),
            &SpectralOptions {
                minimum_score: 0.8,
                local_maximum_radius: 1,
                ..SpectralOptions::default()
            },
        )
        .expect("scan");
        assert!(!peaks.is_empty());
        assert_eq!(peaks[0].matched_text, "two three four");
        assert!((peaks[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn direct_lane_reports_exact_positional_equality() {
        let corpus = "abxabc".chars().map(u32::from).collect::<Vec<_>>();
        let specimen = "abc".chars().map(u32::from).collect::<Vec<_>>();
        let scores = exact_direct_scores(&corpus, &specimen);
        assert!((scores[0] - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(scores[3], 1.0);
    }

    #[test]
    fn direct_lane_is_independent_of_sketch_parameters() {
        let corpus = "the quick brown fox";
        let specimen = "quick brown";
        let first = spectral_scan(
            corpus,
            specimen,
            &NormalizationProfile::default(),
            &SpectralOptions {
                repetitions: 1,
                buckets: 2,
                minimum_score: 0.0,
                ..SpectralOptions::default()
            },
        )
        .expect("first");
        let second = spectral_scan(
            corpus,
            specimen,
            &NormalizationProfile::default(),
            &SpectralOptions {
                repetitions: 32,
                buckets: 4096,
                minimum_score: 0.0,
                ..SpectralOptions::default()
            },
        )
        .expect("second");
        assert_eq!(first[0].offset, second[0].offset);
        assert_eq!(first[0].score, second[0].score);
    }

    #[test]
    fn phase_vectors_have_unit_norm() {
        for token in [0, 1, 42, u32::MAX] {
            for repetition in 0..8 {
                let (cosine, sine) = phase_components(token, repetition, 257);
                assert!((cosine * cosine + sine * sine - 1.0).abs() < 1e-12);
            }
        }
    }
}
