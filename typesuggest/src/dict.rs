use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct WordCandidate {
    pub word: String,
    pub frequency: u64,
}

#[derive(Default)]
struct TrieNode {
    children: HashMap<char, TrieNode>,
    is_word: bool,
    frequency: u64,
    // Cached top candidates for instant O(prefix_length) retrieval
    top_candidates: Vec<WordCandidate>,
}

pub struct Dictionary {
    root: TrieNode,
    // Flat vocabulary list sorted by frequency for fast similarity/fuzzy matching fallback
    words: Vec<WordCandidate>,
    // Letter set of each entry in `words` (see `letter_bits`), kept contiguous so the typo scan can reject
    // most words without dereferencing their strings
    word_letters: Vec<u128>,
    // Preceding word -> sorted list of (follower_word, frequency)
    bigrams: HashMap<String, Vec<(String, u32)>>,
    // User dynamically learned bigrams: preceding_word -> list of (follower_word, frequency)
    user_bigrams: HashMap<String, Vec<(String, u32)>>,
    learn_enabled: bool,
    // Fall back to similar words (typo correction) when no word starts with the prefix
    typo_correction: bool,
}

impl Default for Dictionary {
    fn default() -> Self {
        Self::new()
    }
}

impl Dictionary {
    pub fn new() -> Self {
        Self {
            root: TrieNode::default(),
            words: Vec::new(),
            word_letters: Vec::new(),
            bigrams: HashMap::new(),
            user_bigrams: HashMap::new(),
            learn_enabled: true,
            typo_correction: true,
        }
    }

    pub fn set_learning(&mut self, enabled: bool) {
        self.learn_enabled = enabled;
    }

    pub fn is_learning_enabled(&self) -> bool {
        self.learn_enabled
    }

    pub fn set_typo_correction(&mut self, enabled: bool) {
        self.typo_correction = enabled;
    }

    /// Load from embedded frequency text: each line is "word frequency"
    pub fn from_frequency_text(text: &str) -> Self {
        let mut dict = Self::new();
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            if let (Some(word_str), Some(freq_str)) = (parts.next(), parts.next()) {
                let clean_word = word_str.trim().to_lowercase();
                if clean_word.starts_with('\'') || clean_word.len() < 2 {
                    continue;
                }
                if let Ok(freq) = freq_str.parse::<u64>() {
                    dict.insert(&clean_word, freq);
                }
            }
        }
        dict.rebuild_caches();
        dict
    }

    /// Load bigrams from TSV formatted text: each line is "w1\tw2\tfrequency"
    pub fn load_bigrams_tsv(&mut self, text: &str) {
        for line in text.lines() {
            let mut parts = line.split('\t');
            if let (Some(w1), Some(w2), Some(freq_str)) = (parts.next(), parts.next(), parts.next())
                && let Ok(freq) = freq_str.parse::<u32>()
            {
                let entry = self.bigrams.entry(w1.to_string()).or_default();
                entry.push((w2.to_string(), freq));
            }
        }
    }

    /// Load user custom learned bigrams from file
    pub fn load_user_bigrams_file(&mut self, path: &Path) {
        if let Ok(content) = std::fs::read_to_string(path) {
            for line in content.lines() {
                let mut parts = line.split('\t');
                if let (Some(w1), Some(w2), Some(freq_str)) =
                    (parts.next(), parts.next(), parts.next())
                    && let Ok(freq) = freq_str.trim().parse::<u32>()
                {
                    // The file is plain text the user (or a sync tool) may edit: only accept real
                    // words, since accepted suggestions are typed into apps, terminals included
                    let (w1, w2) = (w1.trim().to_lowercase(), w2.trim().to_lowercase());
                    if is_learnable_word(&w1) && is_learnable_word(&w2) {
                        self.user_bigrams.entry(w1).or_default().push((w2, freq));
                    }
                }
            }
        }
    }

    /// The learned phrases in the user_bigrams.tsv format
    pub fn user_bigrams_tsv(&self) -> String {
        let mut out = String::new();
        for (w1, followers) in &self.user_bigrams {
            for (w2, freq) in followers {
                out.push_str(&format!("{}\t{}\t{}\n", w1, w2, freq));
            }
        }
        out
    }

    /// Save user custom learned bigrams to file securely and atomically
    pub fn save_user_bigrams_file(&self, path: &Path) {
        write_user_bigrams_file(path, &self.user_bigrams_tsv());
    }

    /// Dynamically record a user's word transition into user_bigrams with bounded capacity
    pub fn record_user_bigram(&mut self, prev_word: &str, chosen_word: &str) {
        if !self.learn_enabled {
            return;
        }
        let p = prev_word.trim().to_lowercase();
        let c = chosen_word.trim().to_lowercase();
        // Only pairs of known words: names, codes and passphrase words are never remembered
        if p == c || !self.is_known_word(&p) || !self.is_known_word(&c) {
            return;
        }

        // Bounded capacity: cap total preceding words to 5000 to prevent unbounded memory and disk growth
        if self.user_bigrams.len() >= 5000
            && !self.user_bigrams.contains_key(&p)
            && let Some(evict_key) = self
                .user_bigrams
                .iter()
                .min_by_key(|(_, followers)| followers.iter().map(|(_, f)| *f).sum::<u32>())
                .map(|(k, _)| k.clone())
        {
            self.user_bigrams.remove(&evict_key);
        }

        let entry = self.user_bigrams.entry(p).or_default();
        if let Some(pos) = entry.iter().position(|(w, _)| w == &c) {
            entry[pos].1 = entry[pos].1.saturating_add(1);
            entry.sort_by_key(|a| std::cmp::Reverse(a.1));
        } else {
            entry.insert(0, (c, 1));
            // Cap followers per word at top 10
            if entry.len() > 10 {
                entry.truncate(10);
            }
        }
    }

    /// Whether `word` (lowercase) is in the vocabulary, including the user's custom words
    pub fn is_known_word(&self, word: &str) -> bool {
        let mut node = &self.root;
        for ch in word.chars() {
            match node.children.get(&ch) {
                Some(next) => node = next,
                None => return false,
            }
        }
        node.is_word
    }

    pub fn insert(&mut self, word: &str, frequency: u64) {
        let mut node = &mut self.root;
        for ch in word.chars() {
            node = node.children.entry(ch).or_default();
        }
        node.is_word = true;
        node.frequency = node.frequency.max(frequency);
    }

    /// Rebuilds top_candidates cache down the trie and updates flat word vocabulary
    pub fn rebuild_caches(&mut self) {
        self.words = Self::rebuild_node_cache(&mut self.root, "");
        self.word_letters = self.words.iter().map(|w| letter_bits(&w.word)).collect();
    }

    fn rebuild_node_cache(node: &mut TrieNode, current_prefix: &str) -> Vec<WordCandidate> {
        let mut all_words = Vec::new();

        if node.is_word {
            all_words.push(WordCandidate {
                word: current_prefix.to_string(),
                frequency: node.frequency,
            });
        }

        for (&ch, child) in node.children.iter_mut() {
            let mut next_prefix = current_prefix.to_string();
            next_prefix.push(ch);
            let child_words = Self::rebuild_node_cache(child, &next_prefix);
            all_words.extend(child_words);
        }

        // Sort descending by frequency
        all_words.sort_by_key(|a| std::cmp::Reverse(a.frequency));
        all_words.dedup_by(|a, b| a.word == b.word);

        // Keep top 15 in cache
        node.top_candidates = all_words.iter().take(15).cloned().collect();

        all_words
    }

    /// Unigram-only Trie candidate lookup
    fn suggest_unigram(&self, lower_prefix: &str, limit: usize) -> Vec<String> {
        let mut node = &self.root;
        for ch in lower_prefix.chars() {
            if let Some(next) = node.children.get(&ch) {
                node = next;
            } else {
                return Vec::new();
            }
        }
        node.top_candidates
            .iter()
            .take(limit)
            .map(|c| c.word.clone())
            .collect()
    }

    /// Suggest top `limit` completions for `prefix` conditioned on `prev_word` (sentence context)
    pub fn suggest(&self, prefix: &str, prev_word: Option<&str>, limit: usize) -> Vec<String> {
        if prefix.is_empty() {
            return Vec::new();
        }

        let is_all_caps = prefix.len() > 1 && prefix.chars().all(|c| c.is_uppercase());
        let is_capitalized = prefix
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false);

        let lower_prefix = prefix.to_lowercase();
        let mut results = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // 1. If we have a preceding word in the sentence context, check bigrams first!
        if let Some(prev) = prev_word {
            let clean_prev = prev.trim().to_lowercase();

            // 1a. User learned personal bigrams first (highest priority, if learning enabled)
            if self.learn_enabled
                && let Some(user_followers) = self.user_bigrams.get(&clean_prev)
            {
                for (follower, _) in user_followers {
                    if follower.starts_with(&lower_prefix)
                        && follower != &lower_prefix
                        && seen.insert(follower.clone())
                    {
                        results.push(follower.clone());
                        if results.len() >= limit {
                            break;
                        }
                    }
                }
            }

            // 1b. Standard English bigrams (contextually aware)
            if results.len() < limit
                && let Some(followers) = self.bigrams.get(&clean_prev)
            {
                for (follower, _) in followers {
                    if follower.starts_with(&lower_prefix)
                        && follower != &lower_prefix
                        && seen.insert(follower.clone())
                    {
                        results.push(follower.clone());
                        if results.len() >= limit {
                            break;
                        }
                    }
                }
            }
        }

        // 2. Back off to unigram Trie for any remaining candidate slots
        if results.len() < limit {
            let unigram_cands = self.suggest_unigram(&lower_prefix, limit * 2);
            for cand in unigram_cands {
                if cand != lower_prefix && seen.insert(cand.clone()) {
                    results.push(cand);
                    if results.len() >= limit {
                        break;
                    }
                }
            }
        }

        // 3. If exact prefix matching yielded NO candidates, fall back to similarity search!
        // The vocabulary is English, so only queries containing Latin letters can be near a word
        if self.typo_correction
            && results.is_empty()
            && lower_prefix.len() >= 2
            && lower_prefix.chars().any(|c| c.is_ascii_alphabetic())
        {
            let similar_cands = self.suggest_similar(&lower_prefix, prev_word, limit);
            for cand in similar_cands {
                if cand != lower_prefix && seen.insert(cand.clone()) {
                    results.push(cand);
                    if results.len() >= limit {
                        break;
                    }
                }
            }
        }

        // 4. Format casing to match the user's typed prefix
        results
            .into_iter()
            .take(limit)
            .map(|cand| {
                if is_all_caps {
                    cand.to_uppercase()
                } else if is_capitalized {
                    let mut chars = cand.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => cand,
                    }
                } else {
                    cand
                }
            })
            .collect()
    }

    /// Find the most similar words when exact prefix matching yields no results
    pub fn suggest_similar(
        &self,
        query: &str,
        prev_word: Option<&str>,
        limit: usize,
    ) -> Vec<String> {
        let q_chars: Vec<char> = query.chars().collect();
        let q_len = q_chars.len();
        if q_len < 2 {
            return Vec::new();
        }

        let max_dist = if q_len <= 3 { 1 } else { 2 };
        let mut candidates: Vec<(String, u64)> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // 1. Check bigram followers of prev_word first (contextual priority)
        if let Some(pw) = prev_word {
            let clean = pw.trim().to_lowercase();
            let mut f_chars: Vec<char> = Vec::with_capacity(32);
            let mut check_followers = |followers: &[(String, u32)]| {
                for (follower, freq) in followers {
                    // Same exact letter-set bound as the vocabulary scan below
                    let letters = letter_bits(follower);
                    if q_chars
                        .iter()
                        .filter(|&&c| letters & letter_bit(c) == 0)
                        .count()
                        > max_dist
                    {
                        continue;
                    }
                    f_chars.clear();
                    f_chars.extend(follower.chars());
                    let dist = min_prefix_or_word_distance(&q_chars, &f_chars, max_dist);
                    if dist <= max_dist && seen.insert(follower.clone()) {
                        let score = (10 - dist as u64) * 1_000_000_000 + (*freq as u64) * 100_000;
                        candidates.push((follower.clone(), score));
                    }
                }
            };

            if let Some(user_f) = self.user_bigrams.get(&clean) {
                check_followers(user_f);
            }
            if let Some(bigram_f) = self.bigrams.get(&clean) {
                check_followers(bigram_f);
            }
        }

        // 2. Scan vocabulary words
        let min_wlen = q_len.saturating_sub(max_dist);
        let max_wlen = q_len + 8;

        // Reused across the whole vocabulary scan instead of allocating per word
        let mut w_chars: Vec<char> = Vec::with_capacity(32);
        for (item, &letters) in self.words.iter().zip(&self.word_letters) {
            let w_len = item.word.len();
            if w_len < min_wlen || w_len > max_wlen {
                continue;
            }

            // Same lower bound as in min_prefix_or_word_distance, but against the whole word's
            // letters (a superset), so it never rejects a word the full check would accept
            let missing = q_chars
                .iter()
                .filter(|&&c| letters & letter_bit(c) == 0)
                .count();
            if missing > max_dist {
                continue;
            }

            w_chars.clear();
            w_chars.extend(item.word.chars());
            let dist = min_prefix_or_word_distance(&q_chars, &w_chars, max_dist);
            if dist <= max_dist && !seen.contains(&item.word) {
                seen.insert(item.word.clone());
                let score = (10 - dist as u64) * 1_000_000_000 + item.frequency.min(500_000_000);
                candidates.push((item.word.clone(), score));
                if candidates.len() >= 25 && dist == 1 {
                    break;
                }
            }
        }

        candidates.sort_by_key(|a| std::cmp::Reverse(a.1));
        candidates.into_iter().take(limit).map(|(w, _)| w).collect()
    }
}

/// One of 128 bits standing for `c`. ASCII letters get their own bit; other characters share
/// bits, which only weakens the "letter is missing" bound, never breaks it.
fn letter_bit(c: char) -> u128 {
    1 << (c as u32 & 127)
}

/// Bit set of the characters in `word` (see [`letter_bit`])
fn letter_bits(word: &str) -> u128 {
    word.chars().fold(0, |set, c| set | letter_bit(c))
}

/// A word that may be stored in, or loaded from, the learned phrases file
pub fn is_learnable_word(word: &str) -> bool {
    (2..=64).contains(&word.chars().count()) && word.chars().all(|c| c.is_alphabetic() || c == '\'')
}

/// Atomically replace `path` with `content` (mode 0600). The temporary file is created fresh and
/// never through a symlink, and is removed if anything fails.
pub fn write_user_bigrams_file(path: &Path, content: &str) {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    let temp_path = path.with_extension("tsv.tmp");
    // Removing first unlinks a leftover (or planted symlink) itself, never its target
    let _ = std::fs::remove_file(&temp_path);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(&temp_path)
        .and_then(|mut file| {
            file.write_all(content.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temp_path, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
}

/// Computes minimum Damerau-Levenshtein distance between query and candidate word (or prefixes of candidate word).
/// Distances above `max_dist` are only guaranteed to be reported as some value greater than `max_dist`.
///
/// This runs against every vocabulary word on a typo search, so it keeps just three rolling
/// rows on the stack and stops as soon as no alignment can come back within `max_dist`.
pub fn min_prefix_or_word_distance(
    query_chars: &[char],
    word_chars: &[char],
    max_dist: usize,
) -> usize {
    let q_len = query_chars.len();
    let w_len = word_chars.len();

    if q_len == 0 {
        return w_len;
    }
    if w_len == 0 {
        return q_len;
    }

    // Only inspect word prefix up to q_len + 3
    let max_check_len = (q_len + 3).min(w_len);
    if max_check_len > 32 || q_len > 32 {
        return max_dist + 1;
    }

    // Cheap exact lower bound: a query letter that occurs nowhere in the inspected part of the
    // word must be substituted or deleted
    let word_letters = word_chars[..max_check_len]
        .iter()
        .fold(0u128, |set, &c| set | letter_bit(c));
    let missing = query_chars
        .iter()
        .filter(|&&c| word_letters & letter_bit(c) == 0)
        .count();
    if missing > max_dist {
        return max_dist + 1;
    }

    // Rows i-2, i-1 and i of the distance matrix, indexed by i % 3
    let mut rows = [[0u8; 34]; 3];
    for (j, cell) in rows[0].iter_mut().enumerate().take(max_check_len + 1) {
        *cell = j as u8;
    }
    let mut prev_row_min = 0u8;

    for i in 1..=q_len {
        let (cur, prev, prev2) = (i % 3, (i + 2) % 3, (i + 1) % 3);
        rows[cur][0] = i as u8;
        let mut row_min = rows[cur][0];

        for j in 1..=max_check_len {
            let cost = u8::from(query_chars[i - 1] != word_chars[j - 1]);
            let mut val = (rows[prev][j] + 1)
                .min(rows[cur][j - 1] + 1)
                .min(rows[prev][j - 1] + cost);

            // Damerau transposition
            if i > 1
                && j > 1
                && query_chars[i - 1] == word_chars[j - 2]
                && query_chars[i - 2] == word_chars[j - 1]
            {
                val = val.min(rows[prev2][j - 2] + 1);
            }

            rows[cur][j] = val;
            row_min = row_min.min(val);
        }

        // Every later cell is at least the minimum of the two rows above it, so once both rows
        // exceed max_dist the final answer must too.
        if usize::from(row_min) > max_dist && usize::from(prev_row_min) > max_dist {
            return max_dist + 1;
        }
        prev_row_min = row_min;
    }

    // Minimum distance among full word (if within max_check_len) and prefixes around q_len
    let min_k = q_len - 1;
    rows[q_len % 3]
        .get(min_k..=max_check_len)
        .and_then(|cells| cells.iter().min())
        .map_or(usize::MAX, |&d| usize::from(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefix_suggestions() {
        let text = "program 1000\nprogress 800\nproject 900\nproblem 1500\ncompany 2000";
        let dict = Dictionary::from_frequency_text(text);

        let res = dict.suggest("pro", None, 3);
        assert_eq!(res, vec!["problem", "program", "project"]);
    }

    #[test]
    fn test_capitalization_handling() {
        let text = "program 1000\nprogress 800\nproject 900";
        let dict = Dictionary::from_frequency_text(text);

        let res_cap = dict.suggest("Pro", None, 2);
        assert_eq!(res_cap, vec!["Program", "Project"]);

        let res_upper = dict.suggest("PRO", None, 2);
        assert_eq!(res_upper, vec!["PROGRAM", "PROJECT"]);
    }

    #[test]
    fn test_bigram_context_awareness() {
        let unigram_text = "morning 100\nman 200\nmusic 300\nmuch 400";
        let mut dict = Dictionary::from_frequency_text(unigram_text);

        // Bigram says: "good morning" has huge frequency
        let bigram_text = "good\tmorning\t1000000\ngood\tman\t500000";
        dict.load_bigrams_tsv(bigram_text);

        // Without context: "much" is top unigram
        let no_ctx = dict.suggest("m", None, 3);
        assert_eq!(no_ctx[0], "much");

        // With "good" as context: "morning" is promoted to #1!
        let with_ctx = dict.suggest("m", Some("good"), 3);
        assert_eq!(with_ctx[0], "morning");
        assert_eq!(with_ctx[1], "man");
    }

    #[test]
    fn test_user_learned_bigrams() {
        let text = "server 500\nservice 200\nmy 1000";
        let mut dict = Dictionary::from_frequency_text(text);
        assert_eq!(dict.suggest("s", Some("my"), 2)[0], "server");

        // User picks "service" after "my": it now comes first in that context
        dict.record_user_bigram("my", "service");
        assert_eq!(dict.suggest("s", Some("my"), 2)[0], "service");
    }

    #[test]
    fn test_only_known_words_are_learned() {
        let text = "server 500\nservice 200\nmy 1000";
        let mut dict = Dictionary::from_frequency_text(text);

        // Names, codes and passphrase words are never remembered
        dict.record_user_bigram("hunter2x", "service");
        dict.record_user_bigram("my", "zqxjk");
        assert!(dict.user_bigrams_tsv().is_empty());

        dict.record_user_bigram("my", "service");
        assert_eq!(dict.user_bigrams_tsv(), "my\tservice\t1\n");
    }

    #[test]
    fn test_learned_file_rejects_control_characters() {
        let dir = std::env::temp_dir().join(format!("typesuggest_learned_{}", std::process::id()));
        let path = dir.join("user_bigrams.tsv");
        write_user_bigrams_file(
            &path,
            "my\tservice\t3\nmy\tse\u{1b}[2J\t9\nevil\trm\r\t9\nok\tbad\tNaN\n",
        );
        let mut dict = Dictionary::new();
        dict.load_user_bigrams_file(&path);
        let tsv = dict.user_bigrams_tsv();
        let mut lines: Vec<&str> = tsv.lines().collect();
        lines.sort_unstable();
        // The escape sequence line is dropped; the stray \r is trimmed off as whitespace
        assert_eq!(lines, ["evil\trm\t9", "my\tservice\t3"]);
        assert!(
            !tsv.chars()
                .any(|c| c.is_control() && c != '\t' && c != '\n')
        );

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!path.with_extension("tsv.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_learning_toggle() {
        let text = "server 500\nservice 200\nmy 1000";
        let mut dict = Dictionary::from_frequency_text(text);

        // Disable learning
        dict.set_learning(false);
        assert!(!dict.is_learning_enabled());

        // Attempting to record should do nothing
        dict.record_user_bigram("my", "service");

        // "server" has higher unigram frequency, so it stays #1
        let res = dict.suggest("s", Some("my"), 2);
        assert_eq!(res[0], "server");

        // Re-enable learning
        dict.set_learning(true);
        dict.record_user_bigram("my", "service");
        let res = dict.suggest("s", Some("my"), 2);
        assert_eq!(res[0], "service");
    }

    #[test]
    fn test_similar_words_fallback_on_typos() {
        let text = "extension 5000\ndefinitely 4000\nthe 10000\nreceived 3000\ncomputer 2000";
        let dict = Dictionary::from_frequency_text(text);

        // 1. "exteon" (missing letters / typo) -> suggests "extension"
        let res_exteon = dict.suggest("exteon", None, 1);
        assert_eq!(res_exteon, vec!["extension"]);

        // 2. "definately" ('a' instead of 'i') -> suggests "definitely"
        let res_def = dict.suggest("definately", None, 1);
        assert_eq!(res_def, vec!["definitely"]);

        // 3. "teh" (swapped adjacent letters) -> suggests "the"
        let res_teh = dict.suggest("teh", None, 1);
        assert_eq!(res_teh, vec!["the"]);

        // 4. "recieved" (swapped 'e' and 'i') -> suggests "received"
        let res_rec = dict.suggest("recieved", None, 1);
        assert_eq!(res_rec, vec!["received"]);

        // 5. "comuter" (omitted 'p') -> suggests "computer"
        let res_com = dict.suggest("comuter", None, 1);
        assert_eq!(res_com, vec!["computer"]);

        // 6. Capitalization preservation on typo corrections:
        let res_cap = dict.suggest("Exteon", None, 1);
        assert_eq!(res_cap, vec!["Extension"]);

        let res_all_caps = dict.suggest("TEH", None, 1);
        assert_eq!(res_all_caps, vec!["THE"]);
    }

    #[test]
    fn test_typo_correction_toggle() {
        let text = "extension 5000\nthe 10000\ntheme 900";
        let mut dict = Dictionary::from_frequency_text(text);
        assert_eq!(dict.suggest("teh", None, 1), vec!["the"]);

        dict.set_typo_correction(false);
        assert!(dict.suggest("teh", None, 3).is_empty());
        assert!(dict.suggest("exteon", None, 3).is_empty());
        // Prefix completion is unaffected
        assert_eq!(dict.suggest("th", None, 2), vec!["the", "theme"]);

        dict.set_typo_correction(true);
        assert_eq!(dict.suggest("exteon", None, 1), vec!["extension"]);
    }

    #[test]
    fn test_min_prefix_or_word_distance_measurements() {
        let q1: Vec<char> = "teh".chars().collect();
        let w1: Vec<char> = "the".chars().collect();
        assert_eq!(min_prefix_or_word_distance(&q1, &w1, 2), 1);

        let q2: Vec<char> = "exteon".chars().collect();
        let w2: Vec<char> = "extension".chars().collect();
        assert_eq!(min_prefix_or_word_distance(&q2, &w2, 2), 1);

        let q3: Vec<char> = "definately".chars().collect();
        let w3: Vec<char> = "definitely".chars().collect();
        assert_eq!(min_prefix_or_word_distance(&q3, &w3, 2), 1);
    }

    /// The straightforward full-matrix version the optimized function must agree with
    #[allow(clippy::needless_range_loop)]
    fn reference_distance(q: &[char], w: &[char], max_dist: usize) -> usize {
        if q.is_empty() {
            return w.len();
        }
        if w.is_empty() {
            return q.len();
        }
        let max_check_len = (q.len() + 3).min(w.len());
        if max_check_len > 32 || q.len() > 32 {
            return max_dist + 1;
        }
        let mut d = [[0usize; 34]; 34];
        for (i, row) in d.iter_mut().enumerate().take(q.len() + 1) {
            row[0] = i;
        }
        for j in 0..=max_check_len {
            d[0][j] = j;
        }
        for i in 1..=q.len() {
            for j in 1..=max_check_len {
                let cost = usize::from(q[i - 1] != w[j - 1]);
                let mut val = (d[i - 1][j] + 1)
                    .min(d[i][j - 1] + 1)
                    .min(d[i - 1][j - 1] + cost);
                if i > 1 && j > 1 && q[i - 1] == w[j - 2] && q[i - 2] == w[j - 1] {
                    val = val.min(d[i - 2][j - 2] + 1);
                }
                d[i][j] = val;
            }
        }
        (q.len() - 1..=max_check_len)
            .map(|k| d[q.len()][k])
            .min()
            .unwrap_or(usize::MAX)
    }

    #[test]
    fn test_distance_matches_reference_on_real_vocabulary() {
        let vocab: Vec<Vec<char>> = include_str!("../data/en_50k.txt")
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .take(4000)
            .map(|w| w.chars().collect())
            .collect();
        let queries = [
            "teh",
            "recieve",
            "definately",
            "exteon",
            "comuter",
            "xqzt",
            "ab",
            "hel",
            "progr",
            "acommodate",
            "wierd",
            "thier",
            "seperate",
            "occured",
            "untill",
            "becuase",
            "fr",
            "qwertyuiop",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "naïve",
            "zz",
        ];
        for q in queries {
            let q: Vec<char> = q.chars().collect();
            for w in &vocab {
                for max_dist in [1, 2] {
                    let fast = min_prefix_or_word_distance(&q, w, max_dist);
                    let slow = reference_distance(&q, w, max_dist);
                    if slow <= max_dist {
                        assert_eq!(fast, slow, "{q:?} vs {w:?}");
                    } else {
                        assert!(fast > max_dist, "{q:?} vs {w:?}: {fast} <= {max_dist}");
                    }
                }
            }
        }
    }
}
