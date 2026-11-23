//! Typo-correction accuracy / latency eval.
//!
//!   cargo run --release --example typo_eval
//!   VERBOSE=1 shows misses; MARGIN=<nats> overrides the show margin; SLOW=1 lists slow queries
//!
//! The baseline is the literal prefix completions alone (typo correction off).
//!
//! Hand-written set (data/typo_eval.tsv) plus a seeded synthetic set made from the vocabulary.

use std::time::Instant;
use typesuggest::dict::{Context, Dictionary};

struct Case {
    typed: String,
    expected: String,
    prefix: bool,
    synthetic: bool,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const ROWS: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
fn neighbours(c: char) -> Vec<char> {
    let mut out = Vec::new();
    for (r, row) in ROWS.iter().enumerate() {
        if let Some(x) = row.find(c) {
            let chars: Vec<char> = row.chars().collect();
            if x > 0 {
                out.push(chars[x - 1]);
            }
            if x + 1 < chars.len() {
                out.push(chars[x + 1]);
            }
            for dr in [-1i32, 1] {
                let rr = r as i32 + dr;
                if !(0..3).contains(&rr) {
                    continue;
                }
                let other: Vec<char> = ROWS[rr as usize].chars().collect();
                for dx in [0i32, if dr < 0 { -1 } else { 1 }] {
                    let xx = x as i32 + dx;
                    if xx >= 0 && (xx as usize) < other.len() {
                        out.push(other[xx as usize]);
                    }
                }
            }
        }
    }
    out
}

fn corrupt(word: &str, rng: &mut Rng) -> String {
    let mut c: Vec<char> = word.chars().collect();
    let pos = 1 + rng.below(c.len() - 1); // keep the first letter, as most typists do
    match rng.below(4) {
        0 => {
            let n = neighbours(c[pos]);
            if n.is_empty() {
                return word.to_string();
            }
            c[pos] = n[rng.below(n.len())];
        }
        1 => {
            if pos + 1 < c.len() {
                c.swap(pos, pos + 1);
            } else {
                c.swap(pos - 1, pos);
            }
        }
        2 => {
            c.remove(pos);
        }
        _ => {
            c.insert(pos, c[pos]);
        }
    }
    c.into_iter().collect()
}

fn pct(a: usize, n: usize) -> f64 {
    100.0 * a as f64 / n.max(1) as f64
}

fn main() {
    let t = Instant::now();
    let mut dict = Dictionary::embedded();
    let load = t.elapsed();
    let t2 = Instant::now();
    dict.build_sym_keys();
    let build = t2.elapsed();
    let rss1 = rss_kb();
    println!(
        "load={load:?} index rebuild={build:?} RSS={rss1} kB, typo index={} kB",
        dict.typo_index_bytes() / 1024
    );
    let margin: Option<f64> = std::env::var("MARGIN").ok().and_then(|m| m.parse().ok());
    if let Some(m) = margin {
        dict.set_typo_show_margin(m);
    }

    let freq_text = include_str!("../data/en_50k.txt");
    let vocab: Vec<&str> = freq_text
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();

    let mut cases: Vec<Case> = Vec::new();
    for line in include_str!("../data/typo_eval.tsv").lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 3 || f[2] == "x" {
            continue;
        }
        cases.push(Case {
            typed: f[0].into(),
            expected: f[1].into(),
            prefix: f[2] == "p",
            synthetic: false,
        });
    }
    let hand = cases.len();

    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut made = 0;
    while made < 1500 {
        let idx = 100 + rng.below(8000);
        let w = vocab[idx];
        if w.len() < 5 || !w.chars().all(|c| c.is_ascii_lowercase()) {
            continue;
        }
        let whole = rng.below(2) == 0;
        let (typed, expected, prefix) = if whole {
            (corrupt(w, &mut rng), w.to_string(), false)
        } else {
            let keep = (w.len() * 2 / 3).max(4);
            let p: String = w.chars().take(keep).collect();
            (corrupt(&p, &mut rng), p, true)
        };
        if typed == expected || typed.len() < 3 {
            continue;
        }
        // A typo that is itself a real word is not a mistake the dictionary can see
        if !prefix && dict.is_known_word(&typed) {
            continue;
        }
        cases.push(Case {
            typed,
            expected,
            prefix,
            synthetic: true,
        });
        made += 1;
    }

    let ctx = Context::default();
    let verbose = std::env::var("VERBOSE").is_ok();
    // [hand-w, hand-p, syn-w, syn-p] x [n, base top1, base top3, top1, top3]
    let mut stats = [[0usize; 5]; 4];
    let mut lat: Vec<f64> = Vec::new();
    for c in &cases {
        let hit = |w: &String| {
            if c.prefix {
                w.to_lowercase().starts_with(&c.expected)
            } else {
                w.to_lowercase() == c.expected
            }
        };
        dict.set_typo_correction(false);
        let base = dict.suggest(&c.typed, &ctx, 3);
        dict.set_typo_correction(true);
        let s = Instant::now();
        let got = dict.suggest(&c.typed, &ctx, 3);
        let us = s.elapsed().as_secs_f64() * 1e6;
        if std::env::var("SLOW").is_ok() && us > 1000.0 {
            println!("SLOW {:.0}us {} -> {:?}", us, c.typed, got);
        }
        lat.push(us);
        let k = (c.synthetic as usize) * 2 + c.prefix as usize;
        stats[k][0] += 1;
        stats[k][1] += base.first().is_some_and(hit) as usize;
        stats[k][2] += base.iter().any(hit) as usize;
        stats[k][3] += got.first().is_some_and(hit) as usize;
        stats[k][4] += got.iter().any(hit) as usize;
        if verbose && !got.iter().any(hit) {
            println!("MISS {:<14} want {:<14} got {:?}", c.typed, c.expected, got);
        }
    }
    println!(
        "hand={hand} synthetic={} margin={margin:?}",
        cases.len() - hand
    );
    println!(
        "{:<12} {:>5} {:>9} {:>9} {:>9} {:>9}",
        "", "n", "base top1", "base top3", "top1", "top3"
    );
    let row = |name: &str, s: [usize; 5]| {
        println!(
            "{name:<12} {:>5} {:>8.1}% {:>8.1}% {:>8.1}% {:>8.1}%",
            s[0],
            pct(s[1], s[0]),
            pct(s[2], s[0]),
            pct(s[3], s[0]),
            pct(s[4], s[0])
        );
    };
    for (name, s) in ["hand whole", "hand prefix", "syn whole", "syn prefix"]
        .iter()
        .zip(stats)
    {
        row(name, s);
    }
    let mut all = [0usize; 5];
    for s in stats {
        for i in 0..5 {
            all[i] += s[i];
        }
    }
    row("ALL", all);

    // Correctly typed words and prefixes must not be pushed out by a "correction" (top suggestion
    // that does not even start with what was typed), and should rarely gain one in the top 3
    let mut fp = [[0usize; 3]; 3]; // [common word, rare word, prefix] x [n, displaced, noisy]
    let mut samples: Vec<String> = Vec::new();
    let mut fr = Rng(12345);
    for _ in 0..1500 {
        let (kind, idx) = match fr.below(3) {
            0 => (0, fr.below(5000)),
            1 => (1, 20000 + fr.below(30000)),
            _ => (2, fr.below(20000)),
        };
        let w = vocab[idx];
        if w.len() < 3 || !w.chars().all(|c| c.is_ascii_lowercase()) {
            continue;
        }
        let typed: String = if kind == 2 {
            w.chars().take(3 + fr.below(w.len() - 2)).collect()
        } else {
            w.to_string()
        };
        dict.set_typo_correction(false);
        let base = dict.suggest(&typed, &ctx, 3);
        dict.set_typo_correction(true);
        let got = dict.suggest(&typed, &ctx, 3);
        fp[kind][0] += 1;
        let literal = |g: &String| g.to_lowercase().starts_with(&typed);
        if base.first().is_some_and(literal) && !got.first().is_some_and(literal) {
            fp[kind][1] += 1;
            if samples.len() < 25 {
                samples.push(format!("{typed}: {base:?} -> {got:?}"));
            }
        }
        if got.iter().any(|g| !literal(g) && !base.contains(g)) {
            fp[kind][2] += 1;
            if verbose {
                println!("  NOISE {typed}: {base:?} -> {got:?}");
            }
        }
    }
    for (name, f) in ["common word typed", "rare word typed", "word prefix typed"]
        .iter()
        .zip(fp)
    {
        println!(
            "CORRECT INPUT {name:<18} n={:<4} top1 displaced={:.1}% top3 gains a correction (noise)={:.1}%",
            f[0],
            pct(f[1], f[0]),
            pct(f[2], f[0])
        );
    }
    let (n, d, z) = fp
        .iter()
        .fold((0, 0, 0), |a, f| (a.0 + f[0], a.1 + f[1], a.2 + f[2]));
    println!(
        "CORRECT INPUT ALL n={n} displaced={:.2}% noise={:.1}%",
        pct(d, n),
        pct(z, n)
    );
    if verbose {
        for x in samples {
            println!("  DISPLACED {x}");
        }
    }
    lat.sort_by(|a, b| a.total_cmp(b));
    println!(
        "latency us: p50={:.0} p90={:.0} p99={:.0} max={:.0}",
        lat[lat.len() / 2],
        lat[lat.len() * 9 / 10],
        lat[lat.len() * 99 / 100],
        lat[lat.len() - 1]
    );
}

fn rss_kb() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}
