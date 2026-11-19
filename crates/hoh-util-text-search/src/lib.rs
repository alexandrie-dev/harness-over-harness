//! Multilingual fuzzy search over an in-memory list.

mod fuzzysort;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use hoh_package_manifest::LocalizedText;
use hoh_util_values::{is_js_white_space, js_trim};
use icu_normalizer::{ComposingNormalizer, DecomposingNormalizer};
use icu_properties::{
    CodePointMapData,
    props::{GeneralCategory, GeneralCategoryGroup, Script},
    script::ScriptWithExtensions,
};

const FUZZY_PREFIX: f64 = 4.0;
const FUZZY_SUBSTRING: f64 = 3.0;
const FUZZY_SUBSEQUENCE: f64 = 2.0;
const SUBSTRING: f64 = 1.0;

/// The text one item offers to a search; absent and empty values never match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchKeys {
    /// Names, titles, labels, and other short identifying text.
    pub fuzzy: Vec<Option<String>>,
    /// Descriptions, paths, opaque ids, and other text matched contiguously.
    pub substring: Vec<Option<String>>,
}

/// Return every language carried by a localized value, in manifest order.
pub fn localized_values(text: Option<&LocalizedText>) -> Vec<&str> {
    match text {
        None => Vec::new(),
        Some(LocalizedText::Text(text)) => vec![text],
        Some(LocalizedText::Translations(values)) => values.values().map(String::as_str).collect(),
    }
}

#[derive(Debug, Clone, Copy)]
struct WordStart {
    byte: usize,
    utf16: usize,
}

#[derive(Debug)]
struct PreparedName {
    stripped: String,
    folded: String,
    starts: std::sync::OnceLock<Vec<WordStart>>,
    tails: Mutex<HashMap<usize, fuzzysort::Prepared>>,
}

impl PreparedName {
    fn new(value: &str) -> Self {
        let stripped = strip(value);
        let folded = stripped.to_lowercase();
        Self {
            stripped,
            folded,
            starts: std::sync::OnceLock::new(),
            tails: Mutex::new(HashMap::new()),
        }
    }

    fn starts(&self) -> &[WordStart] {
        self.starts.get_or_init(|| word_starts(&self.stripped))
    }
}

#[derive(Debug)]
struct PreparedKeys {
    fuzzy: Vec<Arc<PreparedName>>,
    substring: Vec<Arc<str>>,
}

#[derive(Debug, Default)]
struct Preparations {
    fuzzy: HashMap<String, Arc<PreparedName>>,
    substring: HashMap<String, Arc<str>>,
}

#[derive(Debug)]
struct Term {
    stripped: String,
    folded: String,
}

/// Prepared search index. Keys are read only while constructing or updating it.
#[derive(Debug)]
pub struct TextSearchIndex<T> {
    items: Vec<T>,
    keys: Vec<PreparedKeys>,
    preparations: Preparations,
}

impl<T> TextSearchIndex<T> {
    /// Prepare every key of every item in display order.
    pub fn new<F>(items: Vec<T>, keys: F) -> Self
    where
        F: FnMut(&T) -> SearchKeys,
    {
        Self::build(items, keys, None)
    }

    /// Build an index for a complete changed list, reusing matching preparations.
    pub fn update<F>(&self, items: Vec<T>, keys: F) -> Self
    where
        F: FnMut(&T) -> SearchKeys,
    {
        Self::build(items, keys, Some(&self.preparations))
    }

    /// The list owned by this index, in build order.
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Filter and rank items. Blank queries return references in build order.
    pub fn search(&self, query: &str) -> Vec<&T> {
        let terms = split_terms(query);
        if terms.is_empty() {
            return self.items.iter().collect();
        }

        let mut ranked = Vec::new();
        for (index, keys) in self.keys.iter().enumerate() {
            let mut score = 0.0;
            let mut matches = true;
            for term in &terms {
                if let Some(hit) = term_score(term, keys) {
                    score += hit;
                } else {
                    matches = false;
                    break;
                }
            }
            if matches {
                ranked.push((index, score));
            }
        }
        ranked.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        ranked
            .into_iter()
            .map(|(index, _)| &self.items[index])
            .collect()
    }

    fn build<F>(items: Vec<T>, mut keys: F, reuse: Option<&Preparations>) -> Self
    where
        F: FnMut(&T) -> SearchKeys,
    {
        let mut preparations = Preparations::default();
        let prepared = items
            .iter()
            .map(|item| prepare_keys(keys(item), reuse, &mut preparations))
            .collect();
        Self {
            items,
            keys: prepared,
            preparations,
        }
    }
}

fn prepare_keys(
    keys: SearchKeys,
    reuse: Option<&Preparations>,
    into: &mut Preparations,
) -> PreparedKeys {
    let fuzzy = keys
        .fuzzy
        .into_iter()
        .flatten()
        .filter(|value| !value.is_empty())
        .map(|value| {
            let prepared = into
                .fuzzy
                .get(&value)
                .or_else(|| reuse.and_then(|old| old.fuzzy.get(&value)))
                .cloned()
                .unwrap_or_else(|| Arc::new(PreparedName::new(&value)));
            into.fuzzy.insert(value, Arc::clone(&prepared));
            prepared
        })
        .collect();
    let substring = keys
        .substring
        .into_iter()
        .flatten()
        .filter(|value| !value.is_empty())
        .map(|value| {
            let prepared = into
                .substring
                .get(&value)
                .or_else(|| reuse.and_then(|old| old.substring.get(&value)))
                .cloned()
                .unwrap_or_else(|| Arc::from(fold(&value)));
            into.substring.insert(value, Arc::clone(&prepared));
            prepared
        })
        .collect();
    PreparedKeys { fuzzy, substring }
}

fn split_terms(query: &str) -> Vec<Term> {
    js_trim(query)
        .split(is_js_white_space)
        .filter(|term| !term.is_empty())
        .filter_map(|term| {
            let stripped = strip(term);
            let folded = stripped.to_lowercase();
            (!folded.is_empty()).then_some(Term { stripped, folded })
        })
        .collect()
}

fn term_score(term: &Term, keys: &PreparedKeys) -> Option<f64> {
    let mut best: Option<f64> = None;
    let mut take = |score: f64| best = Some(best.map_or(score, |current| current.max(score)));
    for name in &keys.fuzzy {
        if name.folded.starts_with(&term.folded) {
            take(FUZZY_PREFIX);
        } else if name.folded.contains(&term.folded) {
            take(FUZZY_SUBSTRING);
        } else if let Some(score) = fuzzy_name_score(term, name) {
            take(FUZZY_SUBSEQUENCE + score.min(0.99));
        }
    }
    for text in &keys.substring {
        if text.contains(&term.folded) {
            take(SUBSTRING);
        }
    }
    best
}

fn fuzzy_name_score(term: &Term, name: &PreparedName) -> Option<f64> {
    if !in_order(&term.folded, &name.folded) {
        return None;
    }
    let first = term
        .stripped
        .chars()
        .next()?
        .to_lowercase()
        .collect::<String>();
    let mut best: Option<f64> = None;
    for start in name.starts() {
        let start_char = name.stripped[start.byte..].chars().next()?;
        if start_char.to_lowercase().collect::<String>() != first {
            continue;
        }
        let result = {
            let mut tails = name
                .tails
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let tail = tails
                .entry(start.byte)
                .or_insert_with(|| fuzzysort::Prepared::new(&name.stripped[start.byte..]));
            fuzzysort::single(&term.stripped, tail)
        };
        if let Some(result) = result {
            let match_utf16 = start.utf16 + result.first_index;
            if word_start_at_utf16(&name.stripped, match_utf16) {
                best = Some(best.map_or(result.score, |current| current.max(result.score)));
            }
        }
    }
    best
}

fn in_order(term: &str, text: &str) -> bool {
    let mut remaining = text;
    for character in term.chars() {
        let Some(at) = remaining.find(character) else {
            return false;
        };
        remaining = &remaining[at + character.len_utf8()..];
    }
    true
}

fn word_starts(text: &str) -> Vec<WordStart> {
    let mut starts = Vec::new();
    let mut previous = None;
    let mut utf16 = 0;
    for (byte, character) in text.char_indices() {
        if previous.is_none_or(|prior| is_word_start(prior, character)) {
            starts.push(WordStart { byte, utf16 });
        }
        previous = Some(character);
        utf16 += character.len_utf16();
    }
    starts
}

fn word_start_at_utf16(text: &str, wanted: usize) -> bool {
    let mut previous = None;
    let mut utf16 = 0;
    for character in text.chars() {
        if utf16 == wanted {
            return previous.is_none_or(|prior| is_word_start(prior, character));
        }
        previous = Some(character);
        utf16 += character.len_utf16();
    }
    false
}

fn is_word_start(previous: char, current: char) -> bool {
    !is_word_char(previous)
        || is_unspaced(current)
        || (category(current) == GeneralCategory::UppercaseLetter
            && category(previous) == GeneralCategory::LowercaseLetter)
}

fn category(character: char) -> GeneralCategory {
    CodePointMapData::<GeneralCategory>::new().get(character)
}

fn is_word_char(character: char) -> bool {
    let value = category(character);
    GeneralCategoryGroup::Letter.contains(value) || GeneralCategoryGroup::Number.contains(value)
}

fn is_unspaced(character: char) -> bool {
    let scripts = ScriptWithExtensions::new();
    [
        Script::Han,
        Script::Hiragana,
        Script::Katakana,
        Script::Hangul,
    ]
    .into_iter()
    .any(|script| scripts.has_script(character, script))
}

fn strip(value: &str) -> String {
    if value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
        return value.to_owned();
    }
    let decomposed = DecomposingNormalizer::new_nfkd().normalize(value);
    let without_diacritics: String = decomposed
        .chars()
        .filter(|character| !('\u{0300}'..='\u{036f}').contains(character))
        .collect();
    let composed = ComposingNormalizer::new_nfkc().normalize(&without_diacritics);
    composed.chars().map(map_stroked).collect()
}

fn fold(value: &str) -> String {
    strip(value).to_lowercase()
}

fn map_stroked(character: char) -> char {
    match character {
        'ø' => 'o',
        'Ø' => 'O',
        'ł' => 'l',
        'Ł' => 'L',
        'đ' | 'ð' => 'd',
        'Đ' | 'Ð' => 'D',
        'ı' => 'i',
        'ħ' => 'h',
        'Ħ' => 'H',
        'ŧ' => 't',
        'Ŧ' => 'T',
        other => other,
    }
}
