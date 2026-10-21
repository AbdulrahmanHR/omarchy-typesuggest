//! Measures suggestion ranking quality on held-out Tatoeba sentences.
//!
//! The n-gram tables are rebuilt from a train split and the cases come from a
//! disjoint eval split (see `data/make_eval_data.py`), so a configuration that
//! merely memorizes the corpus does not score well.
//!
//! Run with the eval data directory as the only argument:
//!   cargo run --release --example ranking_eval -- /tmp/eval

use typesuggest::dict::{Context, Dictionary, Ranking};

fn load_dictionary(dir: &str, with_trigrams: bool) -> Dictionary {
    let freq = include_str!("../data/en_50k.txt");
    let mut dict = Dictionary::from_frequency_text(freq);
    let bigrams = std::fs::read_to_string(format!("{dir}/bigrams.tsv")).expect("bigrams.tsv");
    dict.load_bigrams_tsv(&bigrams);
    if with_trigrams {
        let trigrams =
            std::fs::read_to_string(format!("{dir}/trigrams.tsv")).expect("trigrams.tsv");
        dict.load_trigrams_tsv(&trigrams);
    }
    dict.add_english_contractions();
    dict.set_learning(false);
    dict
}

type Case = (String, String, String, String);

fn load_cases(dir: &str, prefix_len: usize, sample: usize) -> Vec<Case> {
    let text = std::fs::read_to_string(format!("{dir}/cases_prefix{prefix_len}.tsv"))
        .unwrap_or_else(|e| panic!("cases_prefix{prefix_len}.tsv: {e}"));
    let mut cases: Vec<Case> = text
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            Some((
                f.next()?.to_string(),
                f.next()?.to_string(),
                f.next()?.to_string(),
                f.next()?.to_string(),
            ))
        })
        .collect();
    // A deterministic stride keeps the sweep fast without biasing the sample
    if cases.len() > sample {
        let stride = cases.len() / sample;
        cases = cases.into_iter().step_by(stride).collect();
    }
    cases
}

/// Top-1 / top-3 / top-5 hit rates over the held-out cases
fn score(dict: &Dictionary, cases: &[Case], limit: usize) -> (f64, f64, f64) {
    let (top1, top3, top5, _) = score_with_coverage(dict, cases, limit);
    (top1, top3, top5)
}

/// Like `score`, plus the percent of cases that returned any suggestion.
/// Hit rates are over ALL cases, so a case with no suggestion counts as a miss.
fn score_with_coverage(dict: &Dictionary, cases: &[Case], limit: usize) -> (f64, f64, f64, f64) {
    let mut hit = [0usize; 6];
    let mut n = 0usize;
    let mut answered = 0usize;
    for (prev, prev_prev, prefix, target) in cases {
        let ctx = Context::new(Some(prev.clone()), Some(prev_prev.clone()));
        let got = dict.suggest(prefix, &ctx, limit);
        n += 1;
        if !got.is_empty() {
            answered += 1;
        }
        for (rank, word) in got.iter().enumerate() {
            if word == target {
                // A hit at rank r is also a hit for every wider top-N that contains it
                for h in &mut hit[rank + 1..=limit.min(5)] {
                    *h += 1;
                }
                break;
            }
        }
    }
    let f = |v: usize| v as f64 * 100.0 / n as f64;
    (
        f(hit[1]),
        f(hit[3.min(limit)]),
        f(hit[5.min(limit)]),
        f(answered),
    )
}

/// Next-word prediction (empty prefix): coverage plus a unigram x trigram weight sweep
fn run_prediction(dir: &str, cases: &[Case]) {
    let mut d = load_dictionary(dir, true);
    let defaults = Ranking::default();

    d.set_ranking(defaults);
    let (_, _, _, coverage) = score_with_coverage(&d, cases, 5);
    println!("  coverage (at least one prediction): {coverage:.2}%");

    println!("  -- prediction weight sweep (unigram x trigram, bigram = remainder) --");
    for unigram in [0.0f64, 0.1, 0.2, 0.3] {
        for trigram in [0.0f64, 0.3, 0.45, 0.6, 0.7, 0.8] {
            let bigram = 1.0 - unigram - trigram;
            if bigram < -1e-9 {
                continue;
            }
            let bigram = bigram.max(0.0);
            d.set_ranking(Ranking {
                prediction_unigram: unigram,
                prediction_bigram: bigram,
                prediction_trigram: trigram,
                ..defaults
            });
            let t = score(&d, cases, 5);
            let is_default = (unigram - defaults.prediction_unigram).abs() < 1e-9
                && (trigram - defaults.prediction_trigram).abs() < 1e-9;
            let mark = if is_default { "  <- default" } else { "" };
            println!(
                "  unigram {unigram:.2} bigram {bigram:.2} trigram {trigram:.2}   top1 {:5.2}  top3 {:5.2}  top5 {:5.2}{mark}",
                t.0, t.1, t.2
            );
        }
    }
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/eval".to_string());
    let sample: usize = std::env::var("EVAL_SAMPLE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000);

    println!("eval data: {dir}   sample per prefix length: {sample}\n");

    for prefix_len in [0usize, 1, 3] {
        let cases = load_cases(&dir, prefix_len, sample);
        println!("=== prefix length {prefix_len} ({} cases) ===", cases.len());

        // A clean grid: does each order actually earn its place?
        let mut d = load_dictionary(&dir, true);

        // A prediction is scored with the prediction weights, so the grid sets those there
        let run = |label: &str, r: Ranking, d: &mut Dictionary| {
            d.set_ranking(if prefix_len == 0 {
                Ranking {
                    prediction_unigram: r.unigram,
                    prediction_bigram: r.bigram,
                    prediction_trigram: r.trigram,
                    ..r
                }
            } else {
                r
            });
            let t = score(d, &cases, 5);
            println!(
                "  {label:<34} top1 {:5.2}  top3 {:5.2}  top5 {:5.2}",
                t.0, t.1, t.2
            );
        };

        run(
            "no context (frequency only)",
            Ranking {
                unigram: 1.0,
                bigram: 0.0,
                trigram: 0.0,
                ..Ranking::default()
            },
            &mut d,
        );
        run(
            "unigram + bigram",
            Ranking {
                unigram: 0.36,
                bigram: 0.64,
                trigram: 0.0,
                ..Ranking::default()
            },
            &mut d,
        );
        run(
            "unigram + trigram (no bigram)",
            Ranking {
                unigram: 0.36,
                bigram: 0.0,
                trigram: 0.64,
                ..Ranking::default()
            },
            &mut d,
        );
        run("unigram + bigram + trigram", Ranking::default(), &mut d);

        if prefix_len == 0 {
            run_prediction(&dir, &cases);
            println!();
            continue;
        }

        println!("  -- prefix_weight sweep (all orders on) --");
        for pw in [0.0f64, 0.4, 0.8, 1.2, 2.0, 3.0] {
            run(
                &format!("prefix_weight {pw}"),
                Ranking {
                    prefix_weight: pw,
                    ..Ranking::default()
                },
                &mut d,
            );
        }

        println!("  -- trigram weight sweep --");
        for tw in [0.0f64, 0.15, 0.3, 0.45, 0.6, 0.8] {
            let bigram = (1.0 - tw).max(0.0) * 0.78;
            run(
                &format!("trigram {tw} bigram {bigram:.2}"),
                Ranking {
                    trigram: tw,
                    bigram,
                    ..Ranking::default()
                },
                &mut d,
            );
        }
        println!();
    }
}
