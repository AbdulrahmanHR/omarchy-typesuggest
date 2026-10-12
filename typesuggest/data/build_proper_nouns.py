#!/usr/bin/env python3
"""Build data/proper_nouns.txt (names and other always-capitalized words) from SCOWL.

Input: the en_US Hunspell dictionary derived from SCOWL (https://wordlist.aspell.net),
as installed by most distributions at /usr/share/hunspell/en_US.dic and en_US.aff
(Copyright 2000-2026 by Kevin Atkinson; see README.md in this folder for the terms).

Usage: python3 build_proper_nouns.py /usr/share/hunspell/en_US.dic > proper_nouns.txt
(needs the hunspell and unmunch commands, from the hunspell package)

Which words are kept:
- every form of a dictionary entry (Arab, Arabs; possessives are left out, TypeSuggest
  spells "London's" from "London") that is spelled with a capital letter: John, London,
  Monday, NASA, iPhone, McDonald
- it is letters, with apostrophes or hyphens only between letters, and at least three letters,
  or one of TWO_LETTER_WORDS (most two-letter entries are chemical symbols and state codes)
- hunspell rejects the word written in lowercase, so a word that is also a common noun or
  verb, or a form of one (Mark and mark, May and may, Weeks and weeks), stays lowercase.
  ALWAYS_CAPITALIZED overrides this for names whose lowercase sense is rare ("john",
  "english")
- a word with more than one capitalized spelling (MIT and Mit) is left out, as its case is
  not decided
Output lines are the words as they should be written, sorted case-insensitively.
"""

import collections
import os
import re
import subprocess
import sys

WORD = re.compile(r"^[^\W\d_]+(?:['-][^\W\d_]+)*$")

# Names that hunspell also lists in lowercase for a sense far rarer than the name, in
# everyday writing and in the subtitles en_50k.txt was counted from
ALWAYS_CAPITALIZED = """
    Anna Berlin Bible Billy Bob Bobby Brad Carl Carter Charlie Charlotte China Christian
    Donna Dr English German Harry Henry Hong Ian Jane Japan Jean Jenny Jerry Jimmy John
    Johnny Josh Kelly Ken Kyle Lewis Louis Maria Martin Mike Miller Molly Nancy Nick Peter
    Randy Rick Sally Smith Ted Terry Tom Tony Victor
""".split()

TWO_LETTER_WORDS = {"DJ", "Dr", "HQ", "Hz", "IQ", "Mr", "NY", "OK", "TV", "UK"}


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    dictionary = os.path.splitext(os.path.abspath(sys.argv[1]))[0]
    capitalized = collections.defaultdict(set)
    for word in all_forms(dictionary):
        if len(word) < 3 and word not in TWO_LETTER_WORDS:
            continue
        if not WORD.match(word) or word.endswith("'s"):
            continue
        if word != word.lower():
            capitalized[word.lower()].add(word)
    rejected = lowercase_rejected(dictionary, sorted(capitalized))
    always = {word.lower(): word for word in ALWAYS_CAPITALIZED}
    for lower in sorted(capitalized.keys() | always.keys()):
        if lower in always:
            print(always[lower])
            continue
        spellings = capitalized[lower]
        if lower in rejected and len(spellings) == 1:
            print(next(iter(spellings)))


def all_forms(dictionary):
    """Every word the dictionary accepts, each entry expanded with its affixes"""
    result = subprocess.run(
        ["unmunch", dictionary + ".dic", dictionary + ".aff"],
        capture_output=True,
        text=True,
        check=True,
    )
    return set(result.stdout.split())


def lowercase_rejected(dictionary, words):
    """The words hunspell does not accept as written, affixed forms of other entries included"""
    result = subprocess.run(
        ["hunspell", "-d", dictionary, "-l"],
        input="\n".join(words) + "\n",
        capture_output=True,
        text=True,
        check=True,
    )
    return set(result.stdout.split())


if __name__ == "__main__":
    main()
