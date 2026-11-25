//! XPath 3.1 queries over an XML response body, for the `xpath` script source.

use std::cell::{OnceCell, RefCell};

use xee_xpath::{DocumentHandle, Documents, Queries, Query};

/// The body parsed as XML once for all the rules of a run.
#[derive(Default)]
pub(crate) struct Xml(OnceCell<Result<RefCell<(Documents, DocumentHandle)>, String>>);

impl Xml {
    /// Evaluates `expr` against `body` and returns the string value of each
    /// item of the result sequence.
    pub(crate) fn query(&self, body: &[u8], expr: &str) -> Result<Vec<String>, String> {
        let doc = self
            .0
            .get_or_init(|| parse(body))
            .as_ref()
            .map_err(Clone::clone)?;
        let mut doc = doc.borrow_mut();
        let (documents, handle) = &mut *doc;
        let queries = Queries::default();
        let q = queries
            .many(expr.trim(), |documents, item| {
                Ok(item.string_value(documents.xot())?)
            })
            .map_err(|e| format!("invalid XPath: {e}"))?;
        q.execute(documents, *handle)
            .map_err(|e| format!("XPath failed: {e}"))
    }
}

fn parse(body: &[u8]) -> Result<RefCell<(Documents, DocumentHandle)>, String> {
    let text = std::str::from_utf8(body).map_err(|e| format!("body is not XML: {e}"))?;
    let mut documents = Documents::new();
    let handle = documents
        .add_string_without_uri(text)
        .map_err(|e| format!("body is not XML: {e}"))?;
    Ok(RefCell::new((documents, handle)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_nodes_attributes_and_functions() {
        let xml = Xml::default();
        let body = br#"<r><item id="1">a</item><item id="2">b</item></r>"#;
        assert_eq!(xml.query(body, "//item[@id='2']").unwrap(), ["b"]);
        assert_eq!(xml.query(body, "//item/@id").unwrap(), ["1", "2"]);
        assert_eq!(xml.query(body, "count(//item)").unwrap(), ["2"]);
        assert!(xml.query(body, "//none").unwrap().is_empty());
        assert!(
            xml.query(body, "//[")
                .unwrap_err()
                .starts_with("invalid XPath")
        );
    }

    #[test]
    fn rejects_a_body_that_is_not_xml() {
        let err = Xml::default().query(b"{\"a\":1}", "/a").unwrap_err();
        assert!(err.starts_with("body is not XML"), "{err}");
    }
}
