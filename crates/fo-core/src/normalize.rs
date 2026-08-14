use unicode_normalization::UnicodeNormalization;

use crate::model::{NormalizationProfile, PunctuationMode};

#[derive(Debug, Clone)]
pub struct NormalizedText {
    pub text: String,
    pub tokens: Vec<u32>,
    token_byte_offsets: Vec<usize>,
}

impl NormalizedText {
    pub(crate) fn from_stored(text: String) -> Self {
        let tokens = text.chars().map(u32::from).collect::<Vec<_>>();
        let mut token_byte_offsets = text
            .char_indices()
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>();
        token_byte_offsets.push(text.len());
        Self {
            text,
            tokens,
            token_byte_offsets,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    #[must_use]
    pub fn slice_tokens(&self, start: usize, end: usize) -> &str {
        let start = start.min(self.tokens.len());
        let end = end.min(self.tokens.len()).max(start);
        &self.text[self.token_byte_offsets[start]..self.token_byte_offsets[end]]
    }
}

#[must_use]
pub fn normalize(input: &str, profile: &NormalizationProfile) -> NormalizedText {
    let mut stage = if profile.nfkc {
        input.nfkc().collect::<String>()
    } else {
        input.to_owned()
    };
    if profile.lowercase {
        stage = stage.chars().flat_map(char::to_lowercase).collect();
    }

    let mut output = String::with_capacity(stage.len());
    let mut last_was_space = true;
    for ch in stage.chars() {
        if ch.is_whitespace() {
            emit_space(
                &mut output,
                &mut last_was_space,
                profile.collapse_whitespace,
                ch,
            );
            continue;
        }
        let wordish = ch.is_alphanumeric() || ch == '_';
        if wordish || matches!(profile.punctuation, PunctuationMode::Keep) {
            output.push(ch);
            last_was_space = false;
            continue;
        }
        match profile.punctuation {
            PunctuationMode::Keep => {}
            PunctuationMode::ToSpace => emit_space(&mut output, &mut last_was_space, true, ' '),
            PunctuationMode::Drop => {}
        }
    }
    while output.ends_with(' ') {
        output.pop();
    }
    NormalizedText::from_stored(output)
}

fn emit_space(output: &mut String, last_was_space: &mut bool, collapse: bool, original: char) {
    if collapse {
        if !*last_was_space && !output.is_empty() {
            output.push(' ');
            *last_was_space = true;
        }
    } else {
        output.push(original);
        *last_was_space = original.is_whitespace();
    }
}

#[cfg(test)]
mod tests {
    use super::normalize;
    use crate::{NormalizationProfile, PunctuationMode};

    #[test]
    fn normalizes_width_case_punctuation_and_space() {
        let normalized = normalize("  Ｈello,\tWORLD!  ", &NormalizationProfile::default());
        assert_eq!(normalized.text, "hello world");
        assert_eq!(normalized.slice_tokens(0, 5), "hello");
    }

    #[test]
    fn keep_punctuation_retains_symbols() {
        let profile = NormalizationProfile {
            punctuation: PunctuationMode::Keep,
            ..NormalizationProfile::default()
        };
        assert_eq!(normalize("A+B", &profile).text, "a+b");
    }
}
