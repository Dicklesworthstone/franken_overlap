use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Alignment {
    pub distance: usize,
    pub text_start: usize,
    pub text_end: usize,
    pub similarity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InfixCandidate {
    pub distance: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy)]
struct Cell {
    cost: u32,
    start: u32,
    touched_boundary: bool,
}

#[derive(Debug)]
struct BandRow {
    start: usize,
    cells: Vec<Cell>,
}

impl BandRow {
    fn new(start: usize, end: usize) -> Self {
        Self {
            start,
            cells: vec![infinite_cell(); end.saturating_sub(start).saturating_add(1)],
        }
    }

    fn end(&self) -> usize {
        self.start + self.cells.len().saturating_sub(1)
    }

    fn get(&self, column: usize) -> Cell {
        column
            .checked_sub(self.start)
            .and_then(|offset| self.cells.get(offset).copied())
            .unwrap_or_else(infinite_cell)
    }

    fn set(&mut self, column: usize, cell: Cell) {
        if let Some(offset) = column.checked_sub(self.start)
            && let Some(slot) = self.cells.get_mut(offset)
        {
            *slot = cell;
        }
    }
}

const INFINITY: u32 = u32::MAX / 4;

#[must_use]
pub fn semi_global_banded(
    pattern: &[u32],
    text: &[u32],
    expected_start: usize,
    band: usize,
) -> Alignment {
    if pattern.is_empty() {
        let position = expected_start.min(text.len());
        return Alignment {
            distance: 0,
            text_start: position,
            text_end: position,
            similarity: 1.0,
        };
    }
    if text.is_empty() {
        return Alignment {
            distance: pattern.len(),
            text_start: 0,
            text_end: 0,
            similarity: 0.0,
        };
    }
    if pattern.len() <= 64 {
        return semi_global_myers(pattern, text, expected_start);
    }

    let full_radius = expected_start
        .saturating_add(pattern.len())
        .max(text.len().saturating_sub(expected_start))
        .max(1);
    let mut radius = band
        .max(pattern.len().abs_diff(text.len()).saturating_add(8))
        .max(8)
        .min(full_radius);

    let mut prior_alignment = None;
    loop {
        let outcome = semi_global_band_impl(pattern, text, expected_start, radius);
        let stable = prior_alignment == Some(outcome.alignment);
        if radius >= full_radius || (stable && !outcome.touched_boundary) {
            return outcome.alignment;
        }
        prior_alignment = Some(outcome.alignment);
        let next = radius
            .saturating_mul(2)
            .max(radius.saturating_add(1))
            .min(full_radius);
        if next == radius {
            return semi_global_full(pattern, text, expected_start);
        }
        radius = next;
    }
}

#[must_use]
pub fn myers_infix_candidates(
    pattern: &[u32],
    text: &[u32],
    maximum_candidates: usize,
) -> Vec<InfixCandidate> {
    if pattern.is_empty() || pattern.len() > 64 || text.is_empty() || maximum_candidates == 0 {
        return Vec::new();
    }
    let suppression_radius = (pattern.len() / 4).max(1);
    let mut candidates = Vec::with_capacity(maximum_candidates);
    myers_infix_scores(pattern, text, |distance, end| {
        retain_candidate(
            &mut candidates,
            InfixCandidate { distance, end },
            maximum_candidates,
            suppression_radius,
        );
    });
    candidates
}

fn semi_global_myers(pattern: &[u32], text: &[u32], expected_start: usize) -> Alignment {
    let expected_end = expected_start.saturating_add(pattern.len());
    let mut best = InfixCandidate {
        distance: pattern.len(),
        end: expected_start.min(text.len()),
    };
    myers_infix_scores(pattern, text, |distance, end| {
        let ordering = distance.cmp(&best.distance).then_with(|| {
            end.abs_diff(expected_end)
                .cmp(&best.end.abs_diff(expected_end))
        });
        if ordering.is_lt() {
            best = InfixCandidate { distance, end };
        }
    });

    let maximum_span = pattern.len().saturating_add(best.distance);
    let local_start = best.end.saturating_sub(maximum_span);
    let local_text = &text[local_start..best.end];
    let local_expected = expected_start
        .saturating_sub(local_start)
        .min(local_text.len());
    let mut alignment = semi_global_full(pattern, local_text, local_expected);
    alignment.text_start = alignment.text_start.saturating_add(local_start);
    alignment.text_end = alignment.text_end.saturating_add(local_start);
    alignment
}

fn myers_infix_scores(pattern: &[u32], text: &[u32], mut visit: impl FnMut(usize, usize)) {
    debug_assert!(!pattern.is_empty() && pattern.len() <= 64);
    let mask = if pattern.len() == 64 {
        u64::MAX
    } else {
        (1u64 << pattern.len()) - 1
    };
    let highest_bit = 1u64 << (pattern.len() - 1);
    let mut equality = HashMap::<u32, u64>::with_capacity(pattern.len());
    for (position, &token) in pattern.iter().enumerate() {
        *equality.entry(token).or_default() |= 1u64 << position;
    }

    let mut positive = mask;
    let mut negative = 0u64;
    let mut score = pattern.len();
    for (index, token) in text.iter().enumerate() {
        let equal = equality.get(token).copied().unwrap_or(0);
        let vertical = equal | negative;
        let horizontal = ((((equal & positive).wrapping_add(positive)) ^ positive) | equal) & mask;
        let mut positive_horizontal = (negative | !(horizontal | positive)) & mask;
        let negative_horizontal = positive & horizontal;

        if positive_horizontal & highest_bit != 0 {
            score = score.saturating_add(1);
        }
        if negative_horizontal & highest_bit != 0 {
            score = score.saturating_sub(1);
        }

        // A zero entering at the low bit keeps the target prefix free, yielding
        // exact infix/semi-global rather than global-prefix edit distances.
        positive_horizontal = positive_horizontal.wrapping_shl(1) & mask;
        let shifted_negative = negative_horizontal.wrapping_shl(1) & mask;
        positive = (shifted_negative | !(vertical | positive_horizontal)) & mask;
        negative = positive_horizontal & vertical;
        visit(score, index + 1);
    }
}

fn retain_candidate(
    candidates: &mut Vec<InfixCandidate>,
    candidate: InfixCandidate,
    maximum: usize,
    suppression_radius: usize,
) {
    if let Some(index) = candidates
        .iter()
        .position(|existing| existing.end.abs_diff(candidate.end) <= suppression_radius)
    {
        if candidate_key(candidate) < candidate_key(candidates[index]) {
            candidates[index] = candidate;
        }
    } else {
        candidates.push(candidate);
    }
    candidates.sort_unstable_by_key(|candidate| candidate_key(*candidate));
    candidates.truncate(maximum);
}

fn candidate_key(candidate: InfixCandidate) -> (usize, usize) {
    (candidate.distance, candidate.end)
}

#[derive(Debug)]
struct BandOutcome {
    alignment: Alignment,
    touched_boundary: bool,
}

fn semi_global_band_impl(
    pattern: &[u32],
    text: &[u32],
    expected_start: usize,
    radius: usize,
) -> BandOutcome {
    let (first, last) = band_bounds(0, expected_start, text.len(), radius);
    let mut previous = BandRow::new(first, last);
    let previous_end = previous.end();
    for column in first..=last {
        previous.set(
            column,
            Cell {
                cost: 0,
                start: column.min(u32::MAX as usize) as u32,
                touched_boundary: artificial_boundary(column, first, previous_end, text.len()),
            },
        );
    }

    for row in 1..=pattern.len() {
        let (first_column, last_column) = band_bounds(row, expected_start, text.len(), radius);
        let mut current = BandRow::new(first_column, last_column);
        for column in first_column..=last_column {
            let boundary = artificial_boundary(column, first_column, last_column, text.len());
            let cell = if column == 0 {
                Cell {
                    cost: row.min(u32::MAX as usize) as u32,
                    start: 0,
                    touched_boundary: boundary,
                }
            } else {
                let substitution = if pattern[row - 1] == text[column - 1] {
                    0
                } else {
                    1
                };
                let diagonal = add_cost(previous.get(column - 1), substitution, boundary);
                let deletion = add_cost(previous.get(column), 1, boundary);
                let insertion = add_cost(current.get(column - 1), 1, boundary);
                best_cell(
                    [diagonal, deletion, insertion],
                    expected_start,
                    column,
                    pattern.len(),
                )
            };
            current.set(column, cell);
        }
        previous = current;
    }

    let mut best_end = previous.start;
    let mut best = previous.get(best_end);
    for end in previous.start.saturating_add(1)..=previous.end() {
        let candidate = previous.get(end);
        if alignment_order(
            candidate,
            end,
            best,
            best_end,
            expected_start,
            pattern.len(),
        )
        .is_lt()
        {
            best = candidate;
            best_end = end;
        }
    }
    BandOutcome {
        alignment: alignment_from_cell(best, best_end, pattern.len()),
        touched_boundary: best.touched_boundary,
    }
}

fn semi_global_full(pattern: &[u32], text: &[u32], expected_start: usize) -> Alignment {
    let width = text.len() + 1;
    let mut previous = (0..width)
        .map(|column| Cell {
            cost: 0,
            start: column.min(u32::MAX as usize) as u32,
            touched_boundary: false,
        })
        .collect::<Vec<_>>();
    let mut current = vec![infinite_cell(); width];

    for row in 1..=pattern.len() {
        current.fill(infinite_cell());
        current[0] = Cell {
            cost: row.min(u32::MAX as usize) as u32,
            start: 0,
            touched_boundary: false,
        };
        for column in 1..width {
            let substitution = if pattern[row - 1] == text[column - 1] {
                0
            } else {
                1
            };
            current[column] = best_cell(
                [
                    add_cost(previous[column - 1], substitution, false),
                    add_cost(previous[column], 1, false),
                    add_cost(current[column - 1], 1, false),
                ],
                expected_start,
                column,
                pattern.len(),
            );
        }
        std::mem::swap(&mut previous, &mut current);
    }

    let mut best_end = 0;
    let mut best = previous[0];
    for (end, &candidate) in previous.iter().enumerate().skip(1) {
        if alignment_order(
            candidate,
            end,
            best,
            best_end,
            expected_start,
            pattern.len(),
        )
        .is_lt()
        {
            best = candidate;
            best_end = end;
        }
    }
    alignment_from_cell(best, best_end, pattern.len())
}

#[must_use]
pub fn global_levenshtein(left: &[u32], right: &[u32]) -> usize {
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }
    let (rows, columns) = if right.len() <= left.len() {
        (left, right)
    } else {
        (right, left)
    };
    let mut previous = (0..=columns.len()).collect::<Vec<_>>();
    let mut current = vec![0usize; columns.len() + 1];
    for (row_index, &row_value) in rows.iter().enumerate() {
        current[0] = row_index + 1;
        for (column_index, &column_value) in columns.iter().enumerate() {
            let substitution =
                previous[column_index] + if row_value == column_value { 0 } else { 1 };
            let deletion = previous[column_index + 1] + 1;
            let insertion = current[column_index] + 1;
            current[column_index + 1] = substitution.min(deletion).min(insertion);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[columns.len()]
}

fn band_bounds(
    row: usize,
    expected_start: usize,
    text_length: usize,
    radius: usize,
) -> (usize, usize) {
    let center = expected_start.saturating_add(row);
    (
        center.saturating_sub(radius).min(text_length),
        center.saturating_add(radius).min(text_length),
    )
}

fn artificial_boundary(column: usize, first: usize, last: usize, text_length: usize) -> bool {
    (first > 0 && column == first) || (last < text_length && column == last)
}

fn add_cost(cell: Cell, amount: u32, boundary: bool) -> Cell {
    Cell {
        cost: cell.cost.saturating_add(amount).min(INFINITY),
        start: cell.start,
        touched_boundary: cell.touched_boundary || boundary,
    }
}

fn infinite_cell() -> Cell {
    Cell {
        cost: INFINITY,
        start: 0,
        touched_boundary: true,
    }
}

fn alignment_from_cell(cell: Cell, end: usize, pattern_length: usize) -> Alignment {
    let start = (cell.start as usize).min(end);
    let matched_length = end.saturating_sub(start);
    let denominator = pattern_length.max(matched_length).max(1);
    let distance = cell.cost as usize;
    let similarity = (1.0 - distance as f32 / denominator as f32).clamp(0.0, 1.0);
    Alignment {
        distance,
        text_start: start,
        text_end: end,
        similarity,
    }
}

fn best_cell(
    candidates: [Cell; 3],
    expected_start: usize,
    end: usize,
    pattern_length: usize,
) -> Cell {
    let mut best = candidates[0];
    for candidate in candidates.into_iter().skip(1) {
        if alignment_order(candidate, end, best, end, expected_start, pattern_length).is_lt() {
            best = candidate;
        }
    }
    best
}

fn alignment_order(
    left: Cell,
    left_end: usize,
    right: Cell,
    right_end: usize,
    expected_start: usize,
    pattern_length: usize,
) -> std::cmp::Ordering {
    left.cost
        .cmp(&right.cost)
        .then_with(|| {
            (left.start as usize)
                .abs_diff(expected_start)
                .cmp(&(right.start as usize).abs_diff(expected_start))
        })
        .then_with(|| {
            left_end
                .saturating_sub(left.start as usize)
                .abs_diff(pattern_length)
                .cmp(
                    &right_end
                        .saturating_sub(right.start as usize)
                        .abs_diff(pattern_length),
                )
        })
        .then_with(|| left.touched_boundary.cmp(&right.touched_boundary))
}

#[cfg(test)]
mod tests {
    use super::{global_levenshtein, myers_infix_candidates, semi_global_banded, semi_global_full};

    fn tokens(value: &str) -> Vec<u32> {
        value.chars().map(u32::from).collect()
    }

    fn binary_word(length: usize, bits: usize) -> Vec<u32> {
        (0..length)
            .map(|position| if (bits >> position) & 1 == 0 { 0 } else { 1 })
            .collect()
    }

    #[test]
    fn global_distance_handles_all_edit_types() {
        assert_eq!(global_levenshtein(&tokens("kitten"), &tokens("sitting")), 3);
    }

    #[test]
    fn semiglobal_finds_embedded_edited_text() {
        let pattern = tokens("the quick brown fox");
        let text = tokens("prefix the quick red brown fox suffix");
        let alignment = semi_global_banded(&pattern, &text, 7, 16);
        assert!(alignment.distance <= 4, "{alignment:?}");
        assert!(alignment.text_start >= 6 && alignment.text_start <= 8);
        assert!(alignment.similarity > 0.75);
    }

    #[test]
    fn myers_infix_matches_full_dp_exhaustively_for_small_binary_strings() {
        for pattern_length in 1..=4 {
            for text_length in 1..=6 {
                for pattern_bits in 0..1usize << pattern_length {
                    let pattern = binary_word(pattern_length, pattern_bits);
                    for text_bits in 0..1usize << text_length {
                        let text = binary_word(text_length, text_bits);
                        let myers = semi_global_banded(&pattern, &text, 0, 1);
                        let full = semi_global_full(&pattern, &text, 0);
                        assert_eq!(
                            myers.distance, full.distance,
                            "pattern={pattern:?} text={text:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn myers_candidates_find_infix_with_insertions() {
        let candidates = myers_infix_candidates(&tokens("abcdef"), &tokens("xxxabcXdefyyy"), 4);
        assert_eq!(candidates[0].distance, 1);
        assert!(candidates[0].end >= 9 && candidates[0].end <= 10);
    }

    #[test]
    fn long_pattern_uses_band_local_rows() {
        let pattern = "abcdefghij".repeat(8);
        let text = format!("prefix {} suffix", pattern.replace("def", "dXef"));
        let pattern = tokens(&pattern);
        let text = tokens(&text);
        let banded = semi_global_banded(&pattern, &text, 7, 12);
        let full = semi_global_full(&pattern, &text, 7);
        assert_eq!(banded.distance, full.distance);
        assert_eq!(banded.text_start, full.text_start);
        assert_eq!(banded.text_end, full.text_end);
    }
}
