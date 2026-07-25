use std::hint::black_box;
use std::time::{Duration, Instant};

use ziranma_decoder::{Decoder, PinyinEncodeError, SentenceSearchStats, encode_pinyin_phrase};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LatencySummary {
    pub(crate) samples: usize,
    pub(crate) minimum: Duration,
    pub(crate) median: Duration,
    pub(crate) p95: Duration,
    pub(crate) maximum: Duration,
    pub(crate) mean: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DecoderBenchmarkReport {
    pub(crate) repetitions: usize,
    pub(crate) word_queries: usize,
    pub(crate) short_sentence_queries: usize,
    pub(crate) long_sentence_queries: usize,
    pub(crate) word_latency: LatencySummary,
    pub(crate) short_sentence_latency: LatencySummary,
    pub(crate) long_sentence_latency: LatencySummary,
    pub(crate) short_sentence_work: SentenceSearchStats,
    pub(crate) long_sentence_work: SentenceSearchStats,
    pub(crate) result_checksum: usize,
}

pub(crate) fn run_decoder_benchmark(
    decoder: &Decoder,
    repetitions: usize,
) -> Result<DecoderBenchmarkReport, PinyinEncodeError> {
    let word_inputs = benchmark_word_inputs()?;
    let short_sentence_inputs = vec![
        fully_abbreviated_sentence(&["zi ran ma", "shu ru fa"])?,
        "nigkz".to_owned(),
    ];
    let long_sentence_inputs = vec![fully_abbreviated_sentence(&[
        "zi ran ma",
        "shu ru fa",
        "hou xuan",
        "pai xu",
    ])?];

    for observed in &word_inputs {
        black_box(
            decoder
                .decode(black_box(observed), 10)
                .expect("generated keys"),
        );
    }
    let mut short_sentence_work = SentenceSearchStats::default();
    for observed in &short_sentence_inputs {
        let (candidates, stats) = decoder
            .decode_sentence_with_stats(black_box(observed), 10)
            .expect("generated keys");
        add_sentence_stats(&mut short_sentence_work, stats);
        black_box(candidates);
    }
    let mut long_sentence_work = SentenceSearchStats::default();
    for observed in &long_sentence_inputs {
        let (candidates, stats) = decoder
            .decode_sentence_with_stats(black_box(observed), 10)
            .expect("generated keys");
        add_sentence_stats(&mut long_sentence_work, stats);
        black_box(candidates);
    }

    let mut word_durations = Vec::with_capacity(repetitions * word_inputs.len());
    let mut short_sentence_durations =
        Vec::with_capacity(repetitions * short_sentence_inputs.len());
    let mut long_sentence_durations = Vec::with_capacity(repetitions * long_sentence_inputs.len());
    let mut result_checksum = 0usize;

    for _ in 0..repetitions {
        for observed in &word_inputs {
            let started = Instant::now();
            let candidates = decoder
                .decode(black_box(observed), 10)
                .expect("generated keys");
            word_durations.push(started.elapsed());
            result_checksum = update_checksum(result_checksum, &candidates);
            black_box(candidates);
        }
        for observed in &short_sentence_inputs {
            let started = Instant::now();
            let candidates = decoder
                .decode_sentence(black_box(observed), 10)
                .expect("generated keys");
            short_sentence_durations.push(started.elapsed());
            result_checksum = result_checksum.wrapping_mul(31).wrapping_add(
                candidates
                    .iter()
                    .map(|candidate| candidate.text.len())
                    .sum(),
            );
            black_box(candidates);
        }
        for observed in &long_sentence_inputs {
            let started = Instant::now();
            let candidates = decoder
                .decode_sentence(black_box(observed), 10)
                .expect("generated keys");
            long_sentence_durations.push(started.elapsed());
            result_checksum = result_checksum.wrapping_mul(31).wrapping_add(
                candidates
                    .iter()
                    .map(|candidate| candidate.text.len())
                    .sum(),
            );
            black_box(candidates);
        }
    }

    Ok(DecoderBenchmarkReport {
        repetitions,
        word_queries: word_inputs.len(),
        short_sentence_queries: short_sentence_inputs.len(),
        long_sentence_queries: long_sentence_inputs.len(),
        word_latency: summarize_latencies(&mut word_durations).expect("word benchmark has samples"),
        short_sentence_latency: summarize_latencies(&mut short_sentence_durations)
            .expect("short sentence benchmark has samples"),
        long_sentence_latency: summarize_latencies(&mut long_sentence_durations)
            .expect("long sentence benchmark has samples"),
        short_sentence_work,
        long_sentence_work,
        result_checksum,
    })
}

fn benchmark_word_inputs() -> Result<Vec<String>, PinyinEncodeError> {
    let phrases = [
        "ni hao",
        "zi ran ma",
        "shu ru fa",
        "jian pan",
        "zhong wen",
        "suan fa",
        "yin si",
        "xing neng",
        "peng you",
        "shi jie",
        "gong zuo",
        "wen jian",
    ];
    phrases
        .iter()
        .enumerate()
        .map(|(index, phrase)| {
            let encoded = encode_pinyin_phrase(phrase)?;
            let mut observed = encoded.full_code.as_str().as_bytes().to_vec();
            match index % 4 {
                0 => {}
                1 => {
                    return Ok(encoded
                        .syllable_codes
                        .iter()
                        .map(|code| code.as_str().as_bytes()[0] as char)
                        .collect());
                }
                2 if observed.len() > 1 => {
                    let middle = observed.len() / 2;
                    if observed[middle - 1] != observed[middle] {
                        observed.swap(middle - 1, middle);
                    } else {
                        observed.push(observed[middle]);
                    }
                }
                _ => {
                    observed.push(*observed.last().expect("encoded phrase is non-empty"));
                }
            }
            Ok(String::from_utf8(observed).expect("codec emits lowercase ASCII"))
        })
        .collect()
}

fn fully_abbreviated_sentence(words: &[&str]) -> Result<String, PinyinEncodeError> {
    let mut observed = String::new();
    for word in words {
        let encoded = encode_pinyin_phrase(word)?;
        observed.extend(
            encoded
                .syllable_codes
                .iter()
                .map(|code| code.as_str().as_bytes()[0] as char),
        );
    }
    Ok(observed)
}

fn update_checksum(checksum: usize, candidates: &[ziranma_decoder::Candidate]) -> usize {
    candidates.iter().fold(checksum, |checksum, candidate| {
        checksum
            .wrapping_mul(31)
            .wrapping_add(candidate.text.len())
            .wrapping_add(candidate.spelling.code.as_str().len())
    })
}

fn add_sentence_stats(total: &mut SentenceSearchStats, sample: SentenceSearchStats) {
    total.segment_trie_scans += sample.segment_trie_scans;
    total.trie_path_visits += sample.trie_path_visits;
    total.alignment_states_examined += sample.alignment_states_examined;
    total.alignment_states_reused += sample.alignment_states_reused;
    total.terminal_path_matches += sample.terminal_path_matches;
    total.terminal_spelling_matches += sample.terminal_spelling_matches;
    total.lattice_transitions += sample.lattice_transitions;
    total.unresolved_lattice_transitions += sample.unresolved_lattice_transitions;
    total.lattice_transitions_materialized += sample.lattice_transitions_materialized;
    total.lattice_transitions_retained += sample.lattice_transitions_retained;
    total.unresolved_lattice_transitions_retained += sample.unresolved_lattice_transitions_retained;
    total.ranking_states_evaluated += sample.ranking_states_evaluated;
    total.ranking_state_cache_hits += sample.ranking_state_cache_hits;
    total.ranking_transitions_considered += sample.ranking_transitions_considered;
    total.ranking_transitions_retained += sample.ranking_transitions_retained;
    total.path_combinations_considered += sample.path_combinations_considered;
}

fn summarize_latencies(samples: &mut [Duration]) -> Option<LatencySummary> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let count = samples.len();
    let total_nanos = samples.iter().map(Duration::as_nanos).sum::<u128>();
    let mean_nanos = (total_nanos / count as u128).min(u64::MAX as u128) as u64;
    let p95_index = (count * 95).div_ceil(100).saturating_sub(1);
    Some(LatencySummary {
        samples: count,
        minimum: samples[0],
        median: samples[count / 2],
        p95: samples[p95_index],
        maximum: samples[count - 1],
        mean: Duration::from_nanos(mean_nanos),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::summarize_latencies;

    #[test]
    fn latency_summary_uses_deterministic_nearest_rank_percentiles() {
        let mut samples = (1..=100)
            .rev()
            .map(Duration::from_micros)
            .collect::<Vec<_>>();
        let summary = summarize_latencies(&mut samples).unwrap();

        assert_eq!(summary.samples, 100);
        assert_eq!(summary.minimum, Duration::from_micros(1));
        assert_eq!(summary.median, Duration::from_micros(51));
        assert_eq!(summary.p95, Duration::from_micros(95));
        assert_eq!(summary.maximum, Duration::from_micros(100));
        assert_eq!(
            summary.mean,
            Duration::from_micros(50) + Duration::from_nanos(500)
        );
    }
}
