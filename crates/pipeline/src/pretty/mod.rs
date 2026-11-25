//! Pretty forms of non-JSON bodies for the response view: XML and HTML
//! re-indented, YAML as received with folds, CSV and TSV as aligned columns.
//! Each formatter returns the text and, per line, the line that closes the
//! block it opens (`NO_FOLD` when it opens none), like `pretty_json`.

mod markup;
mod table;
mod yaml;

pub use markup::pretty_markup;
pub use table::{COLUMN_SEPARATOR, pretty_table};
pub use yaml::yaml_folds;

/// The format of a body, from its Content-Type or its first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BodyFormat {
    #[default]
    Text,
    Json,
    Xml,
    Html,
    Yaml,
    /// Delimited columns; the value is the separator byte.
    Csv(u8),
}

/// The format of a body that is not JSON. The Content-Type comes first; with
/// none that names a format, only markup is recognized from the first bytes
/// (YAML and CSV look like plain text).
pub fn detect(text: &str, content_type: Option<&str>) -> BodyFormat {
    let ct = content_type
        .and_then(|c| c.split(';').next())
        .map(|c| c.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if ct.contains("html") {
        return BodyFormat::Html;
    }
    if ct.ends_with("xml") {
        return BodyFormat::Xml;
    }
    if ct.contains("yaml") || ct.contains("yml") {
        return BodyFormat::Yaml;
    }
    match ct.as_str() {
        "text/csv" => return BodyFormat::Csv(b','),
        "text/tab-separated-values" => return BodyFormat::Csv(b'\t'),
        _ => {}
    }
    let head: String = text.trim_start().chars().take(15).collect();
    let head = head.to_ascii_lowercase();
    if head.starts_with("<?xml") {
        BodyFormat::Xml
    } else if head.starts_with("<!doctype html") || head.starts_with("<html") {
        BodyFormat::Html
    } else {
        BodyFormat::Text
    }
}

/// The pretty text and folds of `text` in `format`. `None` when the format has
/// no pretty form or the text does not parse as that format.
pub fn pretty(text: &str, format: BodyFormat) -> Option<(String, Vec<u32>)> {
    match format {
        BodyFormat::Xml => pretty_markup(text, false),
        BodyFormat::Html => pretty_markup(text, true),
        BodyFormat::Yaml => Some((text.to_string(), yaml_folds(text))),
        BodyFormat::Csv(sep) => pretty_table(text, sep),
        BodyFormat::Text | BodyFormat::Json => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_names_the_format() {
        let d = |ct| detect("", Some(ct));
        assert_eq!(d("application/xml; charset=utf-8"), BodyFormat::Xml);
        assert_eq!(d("application/atom+xml"), BodyFormat::Xml);
        assert_eq!(d("Text/HTML"), BodyFormat::Html);
        assert_eq!(d("application/xhtml+xml"), BodyFormat::Html);
        assert_eq!(d("application/yaml"), BodyFormat::Yaml);
        assert_eq!(d("text/x-yml"), BodyFormat::Yaml);
        assert_eq!(d("text/csv"), BodyFormat::Csv(b','));
        assert_eq!(d("text/tab-separated-values"), BodyFormat::Csv(b'\t'));
        assert_eq!(d("text/plain"), BodyFormat::Text);
    }

    #[test]
    fn only_markup_is_sniffed() {
        assert_eq!(
            detect("  <?xml version=\"1.0\"?><a/>", None),
            BodyFormat::Xml
        );
        assert_eq!(
            detect("<!DOCTYPE html><p>", Some("text/plain")),
            BodyFormat::Html
        );
        assert_eq!(detect("<HTML><body>", None), BodyFormat::Html);
        assert_eq!(detect("a: 1\nb: 2", None), BodyFormat::Text);
        assert_eq!(detect("a,b\n1,2", None), BodyFormat::Text);
        assert_eq!(detect("<p>", None), BodyFormat::Text);
    }
}
