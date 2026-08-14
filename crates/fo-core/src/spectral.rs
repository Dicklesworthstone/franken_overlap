use serde::{Deserialize, Serialize};

use crate::fingerprint::{categorical_hash, categorical_sign};
use crate::{FoError, NormalizationProfile, Result, normalize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpectralOptions {
    pub repetitions: usize,
    pub buckets: usize,
    pub max_results: usize,
    pub minimum_score: f32,
    pub local_maximum_radius: usize,
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
                "spectral buckets must be between 2 and 4096".to_owned(),
            ));
        }
        if self.max_results == 0 {
            return Err(FoError::InvalidConfig(
                "spectral max_results must be positive".to_owned(),
            ));
        }
        if !(-1.0..=1.0).contains(&self.minimum_score) {
            return Err(FoError::InvalidConfig(
                "spectral minimum_score must lie in [-1, 1]".to_owned(),
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

/// Dense CountSketch categorical cross-correlation.
///
/// Token IDs are never treated as magnitudes. Each repetition maps a token to a
/// random bucket and sign, so equal categories contribute +1 while unequal
/// categories cancel in expectation. With the `frankenscipy` feature this is
/// evaluated by batched FFT correlation; otherwise a bounded direct reference
/// kernel is used.
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

    #[cfg(feature = "frankenscipy")]
    let scores = fft_scores(&corpus.tokens, &specimen.tokens, options)?;
    #[cfg(not(feature = "frankenscipy"))]
    let scores = direct_scores(&corpus.tokens, &specimen.tokens, options)?;

    Ok(extract_peaks(
        &scores,
        &corpus,
        specimen.len(),
        options,
    ))
}

fn direct_scores(corpus: &[u32], specimen: &[u32], options: &SpectralOptions) -> Result<Vec<f32>> {
    let offsets = corpus.len() - specimen.len() + 1;
    let work = (offsets as u128)
        .saturating_mul(specimen.len() as u128)
        .saturating_mul(options.repetitions as u128);
    if work > options.direct_work_limit as u128 {
        return Err(FoError::Spectral(format!(
            "direct CountSketch workload {work} exceeds limit {}; rebuild with --features \
             frankenscipy or raise direct_work_limit",
            options.direct_work_limit
        )));
    }
    let denominator = specimen.len() as f64 * options.repetitions as f64;
    let mut scores = vec![0.0f32; offsets];
    for offset in 0..offsets {
        let mut score = 0.0f64;
        for repetition in 0..options.repetitions {
            for (position, &query_token) in specimen.iter().enumerate() {
                let corpus_token = corpus[offset + position];
                if categorical_hash(corpus_token, repetition) % options.buckets as u64
                    == categorical_hash(query_token, repetition) % options.buckets as u64
                {
                    score += categorical_sign(corpus_token, repetition)
                        * categorical_sign(query_token, repetition);
                }
            }
        }
        scores[offset] = (score / denominator).clamp(-1.0, 1.0) as f32;
    }
    Ok(scores)
}

#[cfg(feature = "frankenscipy")]
fn fft_scores(corpus: &[u32], specimen: &[u32], options: &SpectralOptions) -> Result<Vec<f32>> {
    let offsets = corpus.len() - specimen.len() + 1;
    let mut accumulated = vec![0.0f64; offsets];
    for repetition in 0..options.repetitions {
        for bucket in 0..options.buckets {
            let corpus_channel = corpus
                .iter()
                .map(|&token| {
                    if categorical_hash(token, repetition) % options.buckets as u64
                        == bucket as u64
                    {
                        categorical_sign(token, repetition)
                    } else {
                        0.0
                    }
                })
                .collect::<Vec<_>>();
            let specimen_channel = specimen
                .iter()
                .map(|&token| {
                    if categorical_hash(token, repetition) % options.buckets as u64
                        == bucket as u64
                    {
                        categorical_sign(token, repetition)
                    } else {
                        0.0
                    }
                })
                .collect::<Vec<_>>();
            let channel = fsci_fft::fftcorrelate(&corpus_channel, &specimen_channel, "valid")
                .map_err(|error| FoError::Spectral(error.to_string()))?;
            if channel.len() != offsets {
                return Err(FoError::Spectral(format!(
                    "FrankenSciPy returned {} valid offsets; expected {offsets}",
                    channel.len()
                )));
            }
            for (sum, value) in accumulated.iter_mut().zip(channel) {
                *sum += value;
            }
        }
    }
    let denominator = specimen.len() as f64 * options.repetitions as f64;
    Ok(accumulated
        .into_iter()
        .map(|score| (score / denominator).clamp(-1.0, 1.0) as f32)
        .collect())
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
    use super::{SpectralOptions, spectral_scan};
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
}
