//! CSV and TSV as aligned columns. The `csv` crate reads the records, so a
//! quoted cell can hold the separator, quotes and line breaks.

use unicode_width::UnicodeWidthStr;

/// Bodies larger than this stay raw: aligning needs every cell in memory.
const MAX_TABLE_BYTES: usize = 10 * 1024 * 1024;

/// What separates two columns on a pretty line.
pub const COLUMN_SEPARATOR: &str = " │ ";

/// Each record on one line, its cells padded to the widest cell of their
/// column. A line break inside a cell shows as `⏎`. `None` when the text is
/// too large, does not parse, or its records have different cell counts.
pub fn pretty_table(text: &str, sep: u8) -> Option<(String, Vec<u32>)> {
    if text.len() > MAX_TABLE_BYTES {
        return None;
    }
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(sep)
        .from_reader(text.as_bytes());
    let mut rows: Vec<Vec<String>> = Vec::new();
    for record in reader.records() {
        let record = record.ok()?;
        rows.push(
            record
                .iter()
                .map(|c| c.replace("\r\n", "⏎").replace('\n', "⏎"))
                .collect(),
        );
    }
    let cols = rows.first()?.len();
    let mut widths = vec![0usize; cols];
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.width());
        }
    }
    let mut out = String::with_capacity(text.len() * 2);
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for (j, cell) in row.iter().enumerate() {
            if j > 0 {
                out.push_str(COLUMN_SEPARATOR);
            }
            out.push_str(cell);
            if j + 1 < cols {
                out.extend(std::iter::repeat_n(' ', widths[j] - cell.width()));
            }
        }
    }
    Some((out, Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_line_up() {
        let (p, folds) = pretty_table(
            "name,city,n\n\"Silva, Ana\",São Paulo,1\nBo,\"a \"\"b\"\"\nc\",22\n",
            b',',
        )
        .unwrap();
        assert_eq!(
            p,
            "name       │ city      │ n\n\
             Silva, Ana │ São Paulo │ 1\n\
             Bo         │ a \"b\"⏎c   │ 22"
        );
        assert!(folds.is_empty());
    }

    #[test]
    fn wide_characters_and_tabs() {
        let (p, _) = pretty_table("名前\tx\nab\ty", b'\t').unwrap();
        assert_eq!(p, "名前 │ x\nab   │ y");
    }

    #[test]
    fn uneven_rows_have_no_pretty_form() {
        assert!(pretty_table("a,b\n1,2,3\n", b',').is_none());
        assert!(pretty_table("", b',').is_none());
    }
}
