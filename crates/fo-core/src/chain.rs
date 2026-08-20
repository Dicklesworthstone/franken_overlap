use std::cmp::Ordering;

#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub query_position: u32,
    pub corpus_position: u32,
    pub span: u16,
    pub weight: f32,
}

#[derive(Debug, Clone)]
pub struct ChainOptions {
    pub maximum_anchors: usize,
    pub predecessor_lookback: usize,
    pub maximum_gap: u32,
    pub drift_penalty: f32,
    pub gap_penalty: f32,
}

impl Default for ChainOptions {
    fn default() -> Self {
        Self {
            maximum_anchors: 4096,
            predecessor_lookback: 256,
            maximum_gap: 8192,
            drift_penalty: 0.08,
            gap_penalty: 0.18,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnchorChain {
    pub anchors: Vec<Anchor>,
    pub score: f32,
    pub query_start: u32,
    pub query_end: u32,
    pub corpus_start: u32,
    pub corpus_end: u32,
    pub covered_query_tokens: u32,
    pub median_diagonal: i64,
}

#[must_use]
pub fn chain_anchors(mut anchors: Vec<Anchor>, options: &ChainOptions) -> Option<AnchorChain> {
    if options.maximum_anchors == 0 || options.predecessor_lookback == 0 {
        return None;
    }
    anchors.retain(|anchor| anchor.weight.is_finite() && anchor.weight > 0.0 && anchor.span > 0);
    if anchors.is_empty() {
        return None;
    }
    if anchors.len() > options.maximum_anchors {
        anchors.sort_unstable_by(|left, right| {
            right
                .weight
                .partial_cmp(&left.weight)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.query_position.cmp(&right.query_position))
                .then_with(|| left.corpus_position.cmp(&right.corpus_position))
        });
        anchors.truncate(options.maximum_anchors);
    }
    anchors.sort_unstable_by(|left, right| {
        left.query_position
            .cmp(&right.query_position)
            .then_with(|| left.corpus_position.cmp(&right.corpus_position))
            .then_with(|| left.span.cmp(&right.span))
            .then_with(|| {
                right
                    .weight
                    .partial_cmp(&left.weight)
                    .unwrap_or(Ordering::Equal)
            })
    });
    anchors.dedup_by(|right, left| {
        right.query_position == left.query_position
            && right.corpus_position == left.corpus_position
            && right.span == left.span
    });

    let mut scores = vec![0.0f32; anchors.len()];
    let mut predecessors = vec![None; anchors.len()];
    let mut best_index = 0usize;
    for index in 0..anchors.len() {
        scores[index] = anchors[index].weight;
        let begin = index.saturating_sub(options.predecessor_lookback);
        for predecessor in begin..index {
            let left = anchors[predecessor];
            let right = anchors[index];
            if left.query_position >= right.query_position
                || left.corpus_position >= right.corpus_position
            {
                continue;
            }
            let query_gap = right.query_position - left.query_position;
            let corpus_gap = right.corpus_position - left.corpus_position;
            if query_gap > options.maximum_gap || corpus_gap > options.maximum_gap {
                continue;
            }
            let drift = query_gap.abs_diff(corpus_gap) as f32;
            let long_gap = query_gap
                .max(corpus_gap)
                .saturating_sub(u32::from(left.span));
            let penalty = drift * options.drift_penalty
                + (1.0 + long_gap as f32 / 32.0).ln() * options.gap_penalty;
            let candidate = scores[predecessor] + right.weight - penalty;
            if candidate > scores[index] {
                scores[index] = candidate;
                predecessors[index] = Some(predecessor);
            }
        }
        if scores[index] > scores[best_index] {
            best_index = index;
        }
    }

    let score = scores[best_index];
    let mut indices = Vec::new();
    let mut cursor = Some(best_index);
    while let Some(index) = cursor {
        indices.push(index);
        cursor = predecessors[index];
    }
    indices.reverse();
    let chain = indices
        .into_iter()
        .map(|index| anchors[index])
        .collect::<Vec<_>>();
    let first = *chain.first()?;
    let last = *chain.last()?;
    let mut diagonals = chain
        .iter()
        .map(|anchor| i64::from(anchor.corpus_position) - i64::from(anchor.query_position))
        .collect::<Vec<_>>();
    diagonals.sort_unstable();

    Some(AnchorChain {
        score,
        query_start: first.query_position,
        query_end: last.query_position.saturating_add(u32::from(last.span)),
        corpus_start: first.corpus_position,
        corpus_end: last.corpus_position.saturating_add(u32::from(last.span)),
        covered_query_tokens: interval_coverage(&chain),
        median_diagonal: diagonals[diagonals.len() / 2],
        anchors: chain,
    })
}

fn interval_coverage(anchors: &[Anchor]) -> u32 {
    let mut intervals = anchors
        .iter()
        .map(|anchor| {
            (
                anchor.query_position,
                anchor.query_position.saturating_add(u32::from(anchor.span)),
            )
        })
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let Some(&(mut start, mut end)) = intervals.first() else {
        return 0;
    };
    let mut covered = 0u32;
    for &(next_start, next_end) in intervals.iter().skip(1) {
        if next_start <= end {
            end = end.max(next_end);
        } else {
            covered = covered.saturating_add(end.saturating_sub(start));
            start = next_start;
            end = next_end;
        }
    }
    covered.saturating_add(end.saturating_sub(start))
}

#[cfg(test)]
mod tests {
    use super::{Anchor, ChainOptions, chain_anchors};

    #[test]
    fn chains_across_a_small_insertion() {
        let anchors = vec![
            Anchor {
                query_position: 0,
                corpus_position: 100,
                span: 7,
                weight: 2.0,
            },
            Anchor {
                query_position: 20,
                corpus_position: 120,
                span: 7,
                weight: 2.0,
            },
            Anchor {
                query_position: 40,
                corpus_position: 145,
                span: 7,
                weight: 2.0,
            },
            Anchor {
                query_position: 8,
                corpus_position: 900,
                span: 7,
                weight: 0.5,
            },
        ];
        let chain = chain_anchors(anchors, &ChainOptions::default()).expect("chain");
        assert_eq!(chain.anchors.len(), 3);
        assert_eq!(chain.median_diagonal, 100);
        assert!(chain.covered_query_tokens >= 21);
    }
}
