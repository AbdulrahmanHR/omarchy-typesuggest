//! Keyboard-aware typo distance for the typo fallback.
//!
//! Costs are in tenths of an edit so that the kinds of slip people actually make are cheaper than
//! an arbitrary edit: a key next to the intended one, two letters swapped, one half of a doubled
//! letter missing or added, one vowel for another. A slip in the first letter costs extra, since
//! people rarely get that one wrong.

pub(crate) const FULL: u16 = 10;
const ADJACENT_SUB: u16 = 5;
const VOWEL_SUB: u16 = 7;
pub(crate) const SWAP: u16 = 6;
pub(crate) const DOUBLE: u16 = 6;
pub(crate) const FIRST_LETTER_EXTRA: u16 = 5;

const ROW_OFFSET: [f32; 3] = [0.0, 0.25, 0.75];
const ROWS: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];

const fn key_pos(c: u8) -> Option<(usize, usize)> {
    let mut r = 0;
    while r < 3 {
        let row = ROWS[r].as_bytes();
        let mut i = 0;
        while i < row.len() {
            if row[i] == c {
                return Some((r, i));
            }
            i += 1;
        }
        r += 1;
    }
    None
}

const fn build_adjacency() -> [[bool; 26]; 26] {
    let mut t = [[false; 26]; 26];
    let mut a = 0;
    while a < 26 {
        let mut b = 0;
        while b < 26 {
            if a != b
                && let (Some((ra, ia)), Some((rb, ib))) = (key_pos(b'a' + a as u8), key_pos(b'a' + b as u8))
            {
                let dy = ra.abs_diff(rb);
                let xa = ia as f32 + ROW_OFFSET[ra];
                let xb = ib as f32 + ROW_OFFSET[rb];
                let dx = if xa > xb { xa - xb } else { xb - xa };
                t[a as usize][b as usize] = (dy == 0 && dx < 1.01) || (dy == 1 && dx < 1.0);
            }
            b += 1;
        }
        a += 1;
    }
    t
}

static ADJACENT: [[bool; 26]; 26] = build_adjacency();

fn adjacent(a: char, b: char) -> bool {
    a.is_ascii_lowercase() && b.is_ascii_lowercase() && ADJACENT[(a as u8 - b'a') as usize][(b as u8 - b'a') as usize]
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u')
}

pub(crate) fn sub_cost(q: char, w: char) -> u16 {
    if q == w {
        0
    } else if adjacent(q, w) {
        ADJACENT_SUB
    } else if is_vowel(q) && is_vowel(w) {
        VOWEL_SUB
    } else {
        FULL
    }
}

/// Largest weighted distance worth reporting for a query of `q_len` letters
pub fn cap_for(q_len: usize) -> u16 {
    match q_len {
        0..=3 => 7,  // one cheap slip: a swap, a neighbouring key, a doubled letter
        4..=5 => 10, // one edit of any kind
        6..=7 => 12, // an edit plus a cheap slip
        _ => 20,     // two edits: long words have few neighbours, and phonetic slips need both
    }
}

/// Cheapest way to turn `q` into some prefix of `w` of about the same length (a word typed so far
/// is compared with the start of the word it may become). Costs above `cap` return `cap + 1`.
/// Words longer than 32 letters never match.
pub fn weighted_prefix_distance(q: &[char], w: &[char], cap: u16) -> u16 {
    let ql = q.len();
    if ql == 0 || ql > 32 || w.len() > 40 {
        return cap + 1;
    }
    let wl = w.len().min(ql + 2);
    // Rows i-2, i-1, i
    let mut rows = [[0u16; 36]; 3];
    for j in 1..=wl {
        let ins = if j >= 2 && w[j - 1] == w[j - 2] { DOUBLE } else { FULL };
        let ins = if j == 1 { ins + FIRST_LETTER_EXTRA } else { ins };
        rows[0][j] = rows[0][j - 1] + ins;
    }
    let mut prev_min = 0u16;
    for i in 1..=ql {
        let (cur, prev, prev2) = (i % 3, (i + 2) % 3, (i + 1) % 3);
        let del = if i >= 2 && q[i - 1] == q[i - 2] { DOUBLE } else { FULL };
        let del = if i == 1 { del + FIRST_LETTER_EXTRA } else { del };
        rows[cur][0] = rows[prev][0] + del;
        let mut row_min = rows[cur][0];
        for j in 1..=wl {
            let mut sub = sub_cost(q[i - 1], w[j - 1]);
            if sub > 0 && i == 1 && j == 1 {
                sub += FIRST_LETTER_EXTRA;
            }
            let ins = if j >= 2 && w[j - 1] == w[j - 2] { DOUBLE } else { FULL };
            let ins = if j == 1 { ins + FIRST_LETTER_EXTRA } else { ins };
            let mut v = (rows[prev][j - 1] + sub)
                .min(rows[prev][j] + del)
                .min(rows[cur][j - 1] + ins);
            if i > 1 && j > 1 && q[i - 1] == w[j - 2] && q[i - 2] == w[j - 1] {
                v = v.min(rows[prev2][j - 2] + SWAP);
            }
            rows[cur][j] = v;
            row_min = row_min.min(v);
        }
        // The cheapest cell can only grow from here (every op costs something, and a swap
        // reaches back two rows), so give up when two rows in a row are over the cap
        if row_min > cap && prev_min > cap {
            return cap + 1;
        }
        prev_min = row_min;
    }
    let lo = ql.saturating_sub(2).min(wl);
    (lo..=wl).map(|j| rows[ql % 3][j]).min().unwrap_or(cap + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(q: &str, w: &str) -> u16 {
        let q: Vec<char> = q.chars().collect();
        let w: Vec<char> = w.chars().collect();
        weighted_prefix_distance(&q, &w, 40)
    }

    #[test]
    fn adjacency_matches_a_qwerty_keyboard() {
        assert!(adjacent('a', 's'));
        assert!(adjacent('q', 'w'));
        assert!(adjacent('s', 'w'));
        assert!(adjacent('g', 'b'));
        assert!(!adjacent('a', 'd'));
        assert!(!adjacent('q', 'p'));
        assert!(!adjacent('a', 'a'));
    }

    #[test]
    fn slips_people_make_are_cheaper_than_arbitrary_edits() {
        assert_eq!(d("teh", "the"), SWAP);
        assert_eq!(d("tommorow", "tomorrow"), 12); // transposed pair costs a swap + nothing else
        assert_eq!(d("tomorow", "tomorrow"), DOUBLE);
        assert_eq!(d("wprd", "word"), ADJACENT_SUB);
        assert!(d("wxrd", "word") > d("wprd", "word"));
        assert_eq!(d("same", "same"), 0);
    }

    #[test]
    fn a_typed_prefix_matches_the_start_of_a_longer_word() {
        assert_eq!(d("exmap", "example"), SWAP);
        assert_eq!(d("exam", "example"), 0);
    }

    #[test]
    fn the_cap_stops_the_search_early() {
        let q: Vec<char> = "zzzzzz".chars().collect();
        let w: Vec<char> = "example".chars().collect();
        assert_eq!(weighted_prefix_distance(&q, &w, 20), 21);
    }
}
