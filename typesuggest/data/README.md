# Data files

TypeSuggest's code is MIT licensed (see [`LICENSE`](../LICENSE)). The data files in this
folder, which are also built into the `typesuggest` binary, keep their own licenses:

| File | Contents | Source | License |
|---|---|---|---|
| `en_50k.txt` | The 50,000 most frequent English words with counts | [FrequencyWords](https://github.com/hermitdave/FrequencyWords) by Hermit Dave, `content/2018/en/en_50k.txt`, built from [OpenSubtitles](https://www.opensubtitles.org) 2018 | [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) |
| `bigrams.tsv` | About 320,000 word pairs with counts | Built by [`build_bigrams.py`](build_bigrams.py) from the English sentences written by [Tatoeba](https://tatoeba.org) contributors (export of 2026-09-26) | [CC BY 2.0 FR](https://creativecommons.org/licenses/by/2.0/fr/) |

## Changes

- `en_50k.txt` is unmodified. When loading it, TypeSuggest skips entries that are not words
  (endings the source split off, such as `'s` and `'t`, and abbreviations such as `mr.`),
  drops contraction halves and apostrophe-less spellings (`didn`, `dont`, `im`), and adds
  English contractions whose frequencies are derived from this list's counts (the
  `CONTRACTIONS` table in [`src/dict.rs`](../src/dict.rs)). That derived table is also under
  CC BY-SA 4.0.
- `bigrams.tsv` counts word pairs in Tatoeba's sentences: lowercased, never across
  punctuation, without the stand-in names "Tom" and "Mary", limited to words from
  `en_50k.txt` (and their contractions), pairs seen at least twice, and at most 200 followers
  per word. [`build_bigrams.py`](build_bigrams.py) reproduces it exactly.
