//! Quote action copy owned by the browser plugin.

/// Quote plugin dictionary namespace.
pub const MESSAGE_QUOTES_NAMESPACE: &str = "messageQuotes";

/// Exact `quote.add` string for each source dictionary.
pub fn quote_add_label(locale: &str) -> Option<&'static str> {
    match locale {
        "zh" => Some("引用文本"),
        "en" => Some("Quote text"),
        "ja" => Some("テキストを引用"),
        "es" => Some("Texto de cotización"),
        "fr" => Some("Texte de citation"),
        "de" => Some("Zitattext"),
        "ru" => Some("Текст цитаты"),
        "pt" => Some("Citação de texto"),
        _ => None,
    }
}
