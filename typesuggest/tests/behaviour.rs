//! End-to-end behaviour of TypeSuggest with the vocabulary and language model it ships with.
//!
//! The unit tests next to the code check each piece on small made-up dictionaries; these
//! check what a user sees, through the same keys and app reports the daemon handles, so a
//! fork that changes the data, the ranking or the key handling finds out what it broke.
//!
//!   cargo test --test behaviour            # just these
//!   ../scripts/test.sh                     # everything a release is checked with
//!
//! No Wayland session is needed: the state machine is driven directly, as the daemon drives it.

use std::sync::OnceLock;
use typesuggest::config::Config;
use typesuggest::dict::{Context, Dictionary};
use typesuggest::state::{InputMode, KeyAction, StateMachine};

// X keysyms of the keys used below
const UP: u32 = 0xff52;
const RETURN: u32 = 0xff0d;
const TAB: u32 = 0xff09;
const BACKSPACE: u32 = 0xff08;
const SPACE: u32 = 0x0020;
const INSERT: u32 = 0xff63;
const V: u32 = 0x0076;

/// The built-in dictionary, loaded once for all tests (it takes about a second)
fn dict() -> &'static Dictionary {
    static DICT: OnceLock<Dictionary> = OnceLock::new();
    DICT.get_or_init(Dictionary::embedded)
}

fn suggest(prefix: &str) -> Vec<String> {
    dict().suggest(prefix, &Context::default(), 3)
}

fn suggest_after(prev_prev: Option<&str>, prev: &str, prefix: &str) -> Vec<String> {
    let ctx = Context::new(Some(prev.to_string()), prev_prev.map(str::to_string));
    dict().suggest(prefix, &ctx, 3)
}

/// An app that does not report its text, such as a terminal: only keys reach TypeSuggest
struct Terminal {
    sm: StateMachine,
}

impl Terminal {
    fn new() -> Self {
        Self {
            sm: StateMachine::new(3),
        }
    }

    /// Type `text` key by key, returning what the last key did
    fn type_text(&mut self, text: &str) -> KeyAction {
        let mut last = KeyAction::PassThrough;
        for c in text.chars() {
            last = self.sm.handle_key_press(c as u32, Some(c), false, dict());
        }
        last
    }

    fn key(&mut self, keysym: u32) -> KeyAction {
        self.sm.handle_key_press(keysym, None, false, dict())
    }

    fn ctrl(&mut self, keysym: u32) -> KeyAction {
        self.sm.handle_key_press(keysym, None, true, dict())
    }

    /// The text as TypeSuggest believes it to be
    fn text(&self) -> String {
        self.sm.buffer.chars.iter().collect()
    }
}

/// An app that reports its text after every change (GTK, Qt, Chromium with IME enabled)
struct GuiField {
    sm: StateMachine,
    before: String,
    after: String,
}

impl GuiField {
    /// A focused field holding `before` and `after` on either side of the caret
    fn new(before: &str, after: &str) -> Self {
        let mut field = Self {
            sm: StateMachine::new(3),
            before: before.to_string(),
            after: after.to_string(),
        };
        field.report();
        field
    }

    fn report(&mut self) -> KeyAction {
        let text = format!("{}{}", self.before, self.after);
        let caret = self.before.len();
        self.sm.handle_surrounding_text(&text, caret, caret, dict())
    }

    /// Type `text` key by key, each key followed by the app's report of it
    fn type_text(&mut self, text: &str) -> KeyAction {
        let mut last = KeyAction::PassThrough;
        for c in text.chars() {
            self.sm.handle_key_press(c as u32, Some(c), false, dict());
            self.before.push(c);
            last = self.report();
        }
        last
    }

    /// Paste `text` with Ctrl+V: the app inserts it and reports, no key typed it
    fn paste(&mut self, text: &str) -> KeyAction {
        self.sm.handle_key_press(V, None, true, dict());
        self.before.push_str(text);
        self.report()
    }

    /// Commit the first suggestion as a click does, and apply it to the field's text
    fn commit_first(&mut self) {
        let Some(KeyAction::CommitCandidate {
            deleted_before,
            deleted_after,
            replacement,
            ..
        }) = self.sm.commit_candidate(0)
        else {
            panic!("no suggestions to commit in {:?}", self.before);
        };
        self.before
            .truncate(self.before.len() - deleted_before.len());
        self.after = self.after[deleted_after.len()..].to_string();
        self.before.push_str(&replacement);
    }

    fn text(&self) -> String {
        format!("{}{}", self.before, self.after)
    }
}

fn shows(action: &KeyAction) -> Vec<String> {
    match action {
        KeyAction::ShowSuggestions { candidates, .. } => candidates.clone(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// Suggestions

#[test]
fn common_words_complete_from_the_first_letters() {
    assert!(
        suggest("prog").contains(&"program".to_string()),
        "{:?}",
        suggest("prog")
    );
    assert!(
        suggest("beca").contains(&"because".to_string()),
        "{:?}",
        suggest("beca")
    );
    assert_eq!(suggest("x").len(), 3);
    // The word already typed is not a suggestion for itself
    assert!(!suggest("program").contains(&"program".to_string()));
}

#[test]
fn completions_follow_the_typed_case() {
    assert!(suggest("Prog").contains(&"Program".to_string()));
    assert!(suggest("PROG").contains(&"PROGRAM".to_string()));
    assert!(suggest("i").iter().all(|w| w != "i"));
    assert_eq!(suggest("don'")[0], "don't");
    assert_eq!(
        dict().suggest("DON'", &Context::default(), 1),
        vec!["DON'T"]
    );
}

#[test]
fn context_changes_the_order() {
    // After "good", "morning" comes up front, ahead of words more frequent on their own
    let got = suggest_after(None, "good", "m");
    assert!(got[..2].contains(&"morning".to_string()), "{got:?}");
    assert!(!suggest("m")[..3].contains(&"morning".to_string()));
    // Three words of context beat two
    assert_eq!(suggest_after(Some("as"), "soon", "a")[0], "as");
}

#[test]
fn names_places_and_acronyms_keep_their_capitals() {
    let has = |prefix: &str, word: &str| {
        let got = suggest(prefix);
        assert!(
            got.contains(&word.to_string()),
            "{prefix:?} -> {got:?}, expected {word:?}"
        );
    };
    has("lon", "London");
    has("eng", "English");
    has("joh", "John");
    has("ahm", "Ahmed");
    has("fb", "FBI");
    has("iph", "iPhone");
    has("Ara", "Arabic");
    // In full but in lowercase, a name is offered with its capitals
    has("london", "London");
    // Words that are also names stay ordinary words
    assert!(
        suggest("mar").iter().all(|w| w != "Mark"),
        "{:?}",
        suggest("mar")
    );
}

#[test]
fn typos_fall_back_to_similar_words() {
    let got = suggest("becuase");
    assert!(got.contains(&"because".to_string()), "{got:?}");
}

// ---------------------------------------------------------------------------------------------
// Typing and committing

#[test]
fn accept_keys_commit_with_a_trailing_space() {
    for accept in [RETURN, SPACE, TAB] {
        let mut term = Terminal::new();
        assert!(!shows(&term.type_text("prog")).is_empty());
        assert_eq!(term.key(UP), KeyAction::UpdateSelection { index: 0 });
        let ch = (accept == SPACE).then_some(' ');
        let act = term.sm.handle_key_press(accept, ch, false, dict());
        assert!(matches!(act, KeyAction::CommitCandidate { .. }), "{act:?}");
        assert!(term.text().ends_with(' '), "{:?}", term.text());
        assert_eq!(term.sm.mode, InputMode::Idle);
    }
}

#[test]
fn identifiers_complete_one_segment_at_a_time() {
    let mut field = GuiField::new("", "");
    field.type_text("myProg");
    field.commit_first();
    assert!(field.text().starts_with("myProgram"), "{:?}", field.text());

    let mut field = GuiField::new("", "");
    field.type_text("get_jo");
    field.commit_first();
    // A name inside an identifier keeps the identifier's case
    assert!(field.text().starts_with("get_jo"), "{:?}", field.text());
    assert!(
        field.text().chars().all(|c| !c.is_uppercase()),
        "{:?}",
        field.text()
    );
}

#[test]
fn a_commit_keeps_an_opening_quote() {
    let mut field = GuiField::new("say ", "");
    field.type_text("'prog");
    field.commit_first();
    assert!(field.text().starts_with("say 'prog"), "{:?}", field.text());
}

#[test]
fn no_double_space_before_existing_text() {
    for (after, starts) in [(" world", "program world"), (".", "program.")] {
        let mut field = GuiField::new("", after);
        field.type_text("progr");
        field.commit_first();
        assert_eq!(field.text(), starts);
    }
}

#[test]
fn editing_a_word_in_the_middle_replaces_all_of_it() {
    let mut field = GuiField::new("I was beg", "inning to see");
    // The bar only appears for typing, so delete and retype the last letter
    field.sm.handle_key_press(BACKSPACE, None, false, dict());
    field.before.pop();
    field.report();
    let act = field.type_text("g");
    let candidates = shows(&act);
    let index = candidates
        .iter()
        .position(|w| w == "beginning")
        .unwrap_or_else(|| panic!("no \"beginning\" in {candidates:?}"));
    let Some(KeyAction::CommitCandidate {
        deleted_before,
        deleted_after,
        replacement,
        ..
    }) = field.sm.commit_candidate(index)
    else {
        panic!("nothing committed");
    };
    // The letters on both sides of the caret go, and the space already there is reused
    assert_eq!(
        (deleted_before.as_str(), deleted_after.as_str()),
        ("beg", "inning")
    );
    assert_eq!(replacement, "beginning");
}

// ---------------------------------------------------------------------------------------------
// Paste

#[test]
fn a_pasted_part_of_a_word_completes_in_apps_that_report_text() {
    let mut field = GuiField::new("", "");
    // The paste itself brings no bar up; the next letter does, for the whole word
    assert!(shows(&field.paste("prog")).is_empty());
    let act = field.type_text("r");
    assert!(
        matches!(&act, KeyAction::ShowSuggestions { prefix, candidates }
            if prefix == "progr" && candidates.contains(&"program".to_string())),
        "{act:?}"
    );
}

#[test]
fn a_paste_in_a_terminal_never_completes_a_wrong_word() {
    // The terminal shows "a hel" + "p" = "a help"; TypeSuggest only saw "a " and "p"
    for paste in [V, INSERT] {
        let mut term = Terminal::new();
        term.type_text("a ");
        if paste == V {
            term.ctrl(V);
        } else {
            term.key(INSERT);
        }
        assert_eq!(term.type_text("p"), KeyAction::PassThrough);
        assert_eq!(term.sm.commit_candidate(0), None);
        // The next word is typed in full and completes again
        assert!(!shows(&term.type_text(" prog")).is_empty());
    }
}

// ---------------------------------------------------------------------------------------------
// Privacy

#[test]
fn password_commands_are_flagged_in_terminals() {
    let mut term = Terminal::new();
    term.type_text("sudo pacman -Syu");
    assert_eq!(term.key(RETURN), KeyAction::SensitiveCommandSubmitted);
}

#[test]
fn only_known_words_are_learned() {
    let mut dict = Dictionary::embedded();
    dict.record_user_bigram("my", "hunter2");
    dict.record_user_bigram("correct", "horse");
    let learned = dict.user_bigrams_tsv();
    assert!(!learned.contains("hunter2"), "{learned}");
    assert!(learned.contains("correct\thorse\t1"), "{learned}");
}

// ---------------------------------------------------------------------------------------------
// Configuration

#[test]
fn the_default_config_file_parses_to_the_defaults() {
    let mut parsed = Config::default();
    parsed.parse_str(Config::default_config_file_content());
    assert_eq!(parsed, Config::default());

    let mut cfg = Config::default();
    cfg.parse_str("learn = false # private\nmax_candidates = 5\n");
    assert!(!cfg.learn);
    assert_eq!(cfg.max_candidates, 5);
}
