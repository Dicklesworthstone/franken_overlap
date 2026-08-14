use std::collections::VecDeque;

use crate::Feature;

#[must_use]
pub fn winnow(features: &[Feature], window: usize) -> Vec<Feature> {
    if features.is_empty() {
        return Vec::new();
    }
    if window <= 1 {
        return features.to_vec();
    }
    if features.len() < window {
        let mut minimum = 0usize;
        for index in 1..features.len() {
            if features[index].fingerprint <= features[minimum].fingerprint {
                minimum = index;
            }
        }
        return vec![features[minimum]];
    }

    let mut deque = VecDeque::<usize>::with_capacity(window);
    let mut selected = Vec::with_capacity(features.len().div_ceil(window));
    let mut last_selected = None;
    for index in 0..features.len() {
        while let Some(&back) = deque.back() {
            if features[back].fingerprint < features[index].fingerprint {
                break;
            }
            deque.pop_back();
        }
        deque.push_back(index);
        let first_live = index.saturating_add(1).saturating_sub(window);
        while deque.front().is_some_and(|&front| front < first_live) {
            deque.pop_front();
        }
        if index + 1 >= window {
            let Some(&minimum) = deque.front() else {
                continue;
            };
            if last_selected != Some(minimum) {
                selected.push(features[minimum]);
                last_selected = Some(minimum);
            }
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::winnow;
    use crate::{Feature, Fingerprint};

    fn feature(value: u64, position: u32) -> Feature {
        Feature {
            fingerprint: Fingerprint { hi: 0, lo: value },
            position,
        }
    }

    #[test]
    fn chooses_rightmost_minimum_on_ties() {
        let features = [feature(4, 0), feature(1, 1), feature(1, 2), feature(5, 3)];
        assert_eq!(winnow(&features, 3)[0].position, 2);
    }
}
