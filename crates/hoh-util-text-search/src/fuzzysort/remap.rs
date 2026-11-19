//! Character remapping used by fuzzysort before matching.

use icu_normalizer::DecomposingNormalizer;

pub(super) fn remap(value: &str) -> String {
    let mut remapped = String::with_capacity(value.len());
    for character in value.chars() {
        remapped.push_str(&remap_character(character));
    }
    remapped
}

fn remap_character(character: char) -> String {
    let mapped = match character {
        '\\' => Some('/'),
        '"' | '`' => Some('\''),
        '‘' | '’' | '‚' | '‛' | '“' | '”' | '„' | '‟' | '«' | '»' | '‹' | '›' => {
            Some('\'')
        }
        '‐' | '–' | '—' | '−' => Some('-'),
        '⁄' | '∕' => Some('/'),
        '…' => Some('.'),
        'ø' => Some('o'),
        'Ø' => Some('O'),
        'ł' => Some('l'),
        'Ł' => Some('L'),
        'đ' | 'ð' => Some('d'),
        'Đ' | 'Ð' => Some('D'),
        'ı' => Some('i'),
        'ħ' => Some('h'),
        'Ħ' => Some('H'),
        'ŧ' => Some('t'),
        'Ŧ' => Some('T'),
        _ => None,
    };
    if let Some(mapped) = mapped {
        return mapped.to_string();
    }
    if character.is_ascii() {
        return character.to_string();
    }
    let original = character.to_string();
    let decomposed = DecomposingNormalizer::new_nfkd().normalize(&original);
    let stripped: String = decomposed
        .chars()
        .filter(|value| !('\u{0300}'..='\u{036f}').contains(value))
        .collect();
    if stripped.encode_utf16().count() == 1 {
        stripped
    } else {
        original
    }
}
