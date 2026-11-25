//! YAML folds. The text stays as received (comments, anchors and key order
//! matter), so only the blocks are found, from the indentation.

use crate::doc::NO_FOLD;

/// For each line, the last line of the block it opens: the lines after it
/// that have more indentation. Blank and comment lines do not start or end a
/// block. A `- ` list item counts one more column than its dash, so the item
/// folds its own keys and the key above folds the list.
pub fn yaml_folds(text: &str) -> Vec<u32> {
    let mut folds = Vec::new();
    // One entry per open block: its line and its indentation.
    let mut stack: Vec<(u32, usize)> = Vec::new();
    let mut last = 0u32;
    let close = |folds: &mut Vec<u32>, open: u32, last: u32| {
        if last > open {
            folds[open as usize] = last;
        }
    };
    for (i, line) in text.split('\n').enumerate() {
        folds.push(NO_FOLD);
        let rest = line.trim_start_matches(' ');
        let t = rest.trim_end();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut indent = line.len() - rest.len();
        if t == "-" || rest.starts_with("- ") {
            indent += 1;
        }
        while let Some(&(open, ind)) = stack.last() {
            if ind < indent {
                break;
            }
            stack.pop();
            close(&mut folds, open, last);
        }
        stack.push((i as u32, indent));
        last = i as u32;
    }
    for (open, _) in stack {
        close(&mut folds, open, last);
    }
    folds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_lists_and_block_strings_fold() {
        let src = "\
# comment
server:
  host: x   # trailing
  ports:
  - 80
  - name: tls

    port: 443
note: |
  line 1
  line 2
---
end: 1
";
        let f = yaml_folds(src);
        assert_eq!(f.len(), src.split('\n').count());
        assert_eq!(f[0], NO_FOLD);
        assert_eq!(f[1], 7); // server: up to the last item key
        assert_eq!(f[3], 7); // ports: folds its list
        assert_eq!(f[4], NO_FOLD); // - 80
        assert_eq!(f[5], 7); // - name: folds port, over the blank line
        assert_eq!(f[8], 10); // note: | folds its text
        assert_eq!(f[11], NO_FOLD); // ---
        assert_eq!(f[12], NO_FOLD);
    }
}
