#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Alignment {
    pub distance: usize,
    pub text_start: usize,
    pub text_end: usize,
    pub similarity: f32,
}

#[derive(Debug, Clone, Copy)]
struct Cell {
    cost: u32,
    start: u32,
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
    let effective_band = band.max(pattern.len().abs_diff(text.len()).saturating_add(8));
    let alignment = semi_global_impl(pattern, text, expected_start, Some(effective_band));
    if alignment.distance >= INFINITY as usize / 2 {
        semi_global_impl(pattern, text, expected_start, None)
    } else {
        alignment
    }
}

fn semi_global_impl(
    pattern: &[u32],
    text: &[u32],
    expected_start: usize,
    band: Option<usize>,
) -> Alignment {
    let width = text.len() + 1;
    let mut previous = (0..width)
        .map(|column| Cell {
            cost: 0,
            start: column.min(u32::MAX as usize) as u32,
        })
        .collect::<Vec<_>>();
    let mut current = vec![Cell { cost: INFINITY, start: 0 }; width];

    for row in 1..=pattern.len() {
        current.fill(Cell { cost: INFINITY, start: 0 });
        current[0] = Cell {
            cost: row.min(u32::MAX as usize) as u32,
            start: 0,
        };
        let (first_column, last_column) = match band {
            Some(radius) => {
                let center = expected_start.saturating_add(row);
                (
                    center.saturating_sub(radius).max(1),
                    center.saturating_add(radius).min(text.len()),
                )
            }
            None => (1, text.len()),
        };
        if first_column <= last_column {
            for column in first_column..=last_column {
                let substitution = if pattern[row - 1] == text[column - 1] { 0 } else { 1 };
                let diagonal = Cell {
                    cost: previous[column - 1].cost.saturating_add(substitution),
                    start: previous[column - 1].start,
                };
                let deletion = Cell {
                    cost: previous[column].cost.saturating_add(1),
                    start: previous[column].start,
                };
                let insertion = Cell {
                    cost: current[column - 1].cost.saturating_add(1),
                    start: current[column - 1].start,
                };
                current[column] = best_cell(
                    [diagonal, deletion, insertion],
                    expected_start,
                    column,
                    pattern.len(),
                );
            }
        }
        std::mem::swap(&mut previous, &mut current);
    }

    let expected_end = expected_start.saturating_add(pattern.len());
    let (first_end, last_end) = match band {
        Some(radius) => (
            expected_end.saturating_sub(radius).min(text.len()),
            expected_end.saturating_add(radius).min(text.len()),
        ),
        None => (0, text.len()),
    };
    let mut best_end = first_end;
    let mut best = previous[first_end];
    if first_end < last_end {
        for end in first_end + 1..=last_end {
            let candidate = previous[end];
            if alignment_order(candidate, end, best, best_end, expected_start, pattern.len())
                .is_lt()
            {
                best = candidate;
                best_end = end;
            }
        }
    }
    let start = (best.start as usize).min(best_end);
    let matched_length = best_end.saturating_sub(start);
    let denominator = pattern.len().max(matched_length).max(1);
    let distance = best.cost as usize;
    let similarity = (1.0 - distance as f32 / denominator as f32).clamp(0.0, 1.0);
    Alignment {
        distance,
        text_start: start,
        text_end: best_end,
        similarity,
    }
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
            let substitution = previous[column_index]
                + if row_value == column_value { 0 } else { 1 };
            let deletion = previous[column_index + 1] + 1;
            let insertion = current[column_index] + 1;
            current[column_index + 1] = substitution.min(deletion).min(insertion);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[columns.len()]
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
}

#[cfg(test)]
mod tests {
    use super::{global_levenshtein, semi_global_banded};

    fn tokens(value: &str) -> Vec<u32> {
        value.chars().map(u32::from).collect()
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
}
