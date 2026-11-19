// SHIM(hoh-util-values): Node String.localeCompare collation; replace when
// hoh-util-values provides the shared ICU-compatible locale comparison helper.
use feruca::{Collator, Locale, Tailoring};

/// Sort names as V8 `names.sort((a, b) => a.localeCompare(b))` does under `en-US`:
/// ICU root collation (CLDR), non-ignorable punctuation, tertiary strength, no byte
/// tiebreak (equal keys compare equal; the stable sort keeps their input order).
pub(crate) fn sort_locale(names: &mut [String]) {
    let mut collator = Collator::new(Tailoring::Cldr(Locale::Root), false, false);
    names.sort_by(|a, b| collator.collate(a.as_str(), b.as_str()));
}
