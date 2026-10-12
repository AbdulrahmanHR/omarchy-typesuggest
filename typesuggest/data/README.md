# Data files

TypeSuggest's code is MIT licensed (see [`LICENSE`](../LICENSE)). The data files in this
folder, which are also built into the `typesuggest` binary, keep their own licenses:

| File | Contents | Source | License |
|---|---|---|---|
| `en_50k.txt` | The 50,000 most frequent English words with counts | [FrequencyWords](https://github.com/hermitdave/FrequencyWords) by Hermit Dave, `content/2018/en/en_50k.txt`, built from [OpenSubtitles](https://www.opensubtitles.org) 2018 | [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) |
| `bigrams.tsv` | About 320,000 word pairs with counts | Built by [`build_bigrams.py`](build_bigrams.py) from the English sentences written by [Tatoeba](https://tatoeba.org) contributors (export of 2026-09-26) | [CC BY 2.0 FR](https://creativecommons.org/licenses/by/2.0/fr/) |
| `trigrams.tsv` | About 319,000 word triples with counts | Built by [`build_trigrams.py`](build_trigrams.py) from the same Tatoeba export | [CC BY 2.0 FR](https://creativecommons.org/licenses/by/2.0/fr/) |
| `proper_nouns.txt` | About 15,000 names, places, languages and acronyms as they are written (John, London, English, FBI, iPhone) | Built by [`build_proper_nouns.py`](build_proper_nouns.py) from the en_US Hunspell dictionary of [SCOWL](https://wordlist.aspell.net) (2026.02.25) | SCOWL license, below |

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
- `trigrams.tsv` counts word triples in the same sentences, with the same word filter and
  clause boundaries. A triple must be seen at least three times (a pair needs two) and each
  pair of words keeps its 10 most frequent followers (a word keeps 200), since a triple is
  consulted to choose between a handful of candidates rather than to enumerate them.
  [`build_trigrams.py`](build_trigrams.py) reproduces it exactly, and shares its tokenizing
  with `build_bigrams.py` so the two tables cannot drift apart.

- `proper_nouns.txt` keeps the dictionary's capitalized words, with their plurals (Arabs) but
  not their possessives, whose lowercase spelling the dictionary rejects, so a name that is
  also a common word (Mark and mark, May and may) is left out. A short list of names whose
  lowercase sense is rare (John and john, English and english) is kept regardless, and
  two-letter entries, mostly chemical symbols and state codes, are left out except for a
  few common ones (OK, TV, Mr, Dr).

### SCOWL license

```text
Copyright 2000-2026 by Kevin Atkinson

Permission to use, copy, modify, distribute, and sell any part of SCOWLv2, or
word lists created from it, is hereby granted without fee, provided that the
above copyright notice appears in all copies and that both the above
copyright notice and this notice appear in supporting documentation.  Kevin
Atkinson makes no representations about the suitability of this database for
any purpose.  It is provided "as is" without express or implied warranty.
```

## Tools

The scripts write their table to standard output:

```bash
python3 build_bigrams.py  eng_sentences.tsv.bz2 en_50k.txt > bigrams.tsv
python3 build_trigrams.py eng_sentences.tsv.bz2 en_50k.txt > trigrams.tsv
python3 build_proper_nouns.py /usr/share/hunspell/en_US.dic > proper_nouns.txt   # needs hunspell
```

`make_eval_data.py` measures a change to ranking honestly: it splits Tatoeba's sentences
deterministically by sentence id, 90% train and 10% evaluation, and builds its n-gram tables
from the training half only. Scoring a ranking on the evaluation half then measures how well it
generalizes rather than how well it memorizes, which a table scored against its own sentences
would trivially do.

```bash
python3 make_eval_data.py eng_sentences.tsv.bz2 en_50k.txt ../evaldata
cargo run --release --example ranking_eval -- ../evaldata
cargo run --release --example bench        # model load time and lookup latency
```
