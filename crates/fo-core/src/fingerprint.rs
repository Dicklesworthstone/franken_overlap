use serde::{Deserialize, Serialize};

use crate::{FoError, Result};

const BASE_LO: u64 = 0x9e37_79b1_85eb_ca87;
const BASE_HI: u64 = 0xc2b2_ae3d_27d4_eb4f;
const SEED_LO: u64 = 0x243f_6a88_85a3_08d3;
const SEED_HI: u64 = 0x1319_8a2e_0370_7344;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Fingerprint {
    pub hi: u64,
    pub lo: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    pub fingerprint: Fingerprint,
    pub position: u32,
}

pub fn qgram_hashes(tokens: &[u32], qgram_size: usize) -> Result<Vec<Feature>> {
    if qgram_size == 0 {
        return Err(FoError::InvalidConfig(
            "qgram_size must be greater than zero".to_owned(),
        ));
    }
    if tokens.len() < qgram_size {
        return Ok(Vec::new());
    }
    let last_position = tokens.len() - qgram_size;
    if last_position > u32::MAX as usize {
        return Err(FoError::InvalidConfig(
            "token stream exceeds the u32 feature-position limit".to_owned(),
        ));
    }

    let power_lo = wrapping_pow(BASE_LO, qgram_size - 1);
    let power_hi = wrapping_pow(BASE_HI, qgram_size - 1);
    let mut hash_lo = 0u64;
    let mut hash_hi = 0u64;
    for &token in &tokens[..qgram_size] {
        hash_lo = hash_lo
            .wrapping_mul(BASE_LO)
            .wrapping_add(token_word(token, SEED_LO));
        hash_hi = hash_hi
            .wrapping_mul(BASE_HI)
            .wrapping_add(token_word(token, SEED_HI));
    }

    let mut features = Vec::with_capacity(last_position + 1);
    features.push(Feature {
        fingerprint: finish(hash_hi, hash_lo, qgram_size),
        position: 0,
    });
    for position in 1..=last_position {
        let outgoing = tokens[position - 1];
        let incoming = tokens[position + qgram_size - 1];
        hash_lo = hash_lo
            .wrapping_sub(token_word(outgoing, SEED_LO).wrapping_mul(power_lo))
            .wrapping_mul(BASE_LO)
            .wrapping_add(token_word(incoming, SEED_LO));
        hash_hi = hash_hi
            .wrapping_sub(token_word(outgoing, SEED_HI).wrapping_mul(power_hi))
            .wrapping_mul(BASE_HI)
            .wrapping_add(token_word(incoming, SEED_HI));
        features.push(Feature {
            fingerprint: finish(hash_hi, hash_lo, qgram_size),
            position: u32::try_from(position)
                .map_err(|_| FoError::InvalidConfig("feature position exceeds u32".to_owned()))?,
        });
    }
    Ok(features)
}

#[must_use]
#[cfg_attr(not(feature = "frankenscipy"), allow(dead_code))]
pub(crate) fn categorical_hash(value: u32, repetition: usize) -> u64 {
    avalanche(u64::from(value) ^ SEED_LO ^ (repetition as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
}

fn finish(hash_hi: u64, hash_lo: u64, qgram_size: usize) -> Fingerprint {
    let q = qgram_size as u64;
    Fingerprint {
        hi: avalanche(hash_hi ^ q.wrapping_mul(SEED_HI)),
        lo: avalanche(hash_lo ^ q.wrapping_mul(SEED_LO)),
    }
}

fn token_word(token: u32, seed: u64) -> u64 {
    avalanche(u64::from(token).wrapping_add(seed)) | 1
}

fn wrapping_pow(mut base: u64, mut exponent: usize) -> u64 {
    let mut result = 1u64;
    while exponent > 0 {
        if exponent & 1 == 1 {
            result = result.wrapping_mul(base);
        }
        base = base.wrapping_mul(base);
        exponent >>= 1;
    }
    result
}

fn avalanche(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::qgram_hashes;

    #[test]
    fn equal_qgrams_have_equal_fingerprints() {
        let tokens = "abcabc".chars().map(u32::from).collect::<Vec<_>>();
        let features = qgram_hashes(&tokens, 3).expect("hashes");
        assert_eq!(features[0].fingerprint, features[3].fingerprint);
    }

    #[test]
    fn rolling_hash_matches_recomputed_substrings() {
        let tokens = "the quick brown fox"
            .chars()
            .map(u32::from)
            .collect::<Vec<_>>();
        let rolling = qgram_hashes(&tokens, 5).expect("hashes");
        for (position, feature) in rolling.iter().enumerate() {
            let direct = qgram_hashes(&tokens[position..position + 5], 5).expect("direct");
            assert_eq!(feature.fingerprint, direct[0].fingerprint);
        }
    }
}
