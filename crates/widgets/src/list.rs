//! Moving through a list: the arithmetic every list in the app shares, so a
//! scroll, a page step or a wrap-around behaves the same everywhere.

/// The scroll that keeps `sel` visible in a window of `rows` rows.
pub fn keep_visible(sel: usize, scroll: usize, rows: usize) -> usize {
    if rows == 0 {
        return scroll;
    }
    if sel < scroll {
        sel
    } else if sel >= scroll + rows {
        sel + 1 - rows
    } else {
        scroll
    }
}

/// `sel` moved by `dir` rows, clamped to `len` (an empty list stays at 0).
pub fn step(sel: usize, dir: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let last = len - 1;
    if dir < 0 {
        sel.saturating_sub(dir.unsigned_abs())
    } else {
        sel.saturating_add(dir as usize).min(last)
    }
}

/// `sel` moved by `dir` rows, wrapping around (`len` 0 stays at 0).
pub fn cycle(sel: usize, dir: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    ((sel as isize + dir).rem_euclid(len as isize)) as usize
}

/// `sel` moved a page of `rows` rows, clamped to `len`.
pub fn page(sel: usize, dir: isize, rows: usize, len: usize) -> usize {
    step(sel, dir.saturating_mul(rows as isize), len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_follows_the_selection() {
        // Already visible: the scroll does not move.
        assert_eq!(keep_visible(3, 2, 5), 2);
        // Above the window: it snaps to the selection.
        assert_eq!(keep_visible(1, 4, 5), 1);
        // Below the window: the selection lands on the last row.
        assert_eq!(keep_visible(9, 0, 5), 5);
        // No room at all: the scroll is left alone.
        assert_eq!(keep_visible(9, 3, 0), 3);
    }

    #[test]
    fn step_clamps_at_both_ends() {
        assert_eq!(step(0, -1, 5), 0);
        assert_eq!(step(2, -2, 5), 0);
        assert_eq!(step(4, 1, 5), 4);
        assert_eq!(step(0, 3, 5), 3);
        assert_eq!(step(3, -10, 5), 0);
        assert_eq!(step(3, 1, 0), 0);
    }

    #[test]
    fn cycle_wraps_both_ways() {
        assert_eq!(cycle(0, -1, 3), 2);
        assert_eq!(cycle(2, 1, 3), 0);
        assert_eq!(cycle(1, 1, 3), 2);
        assert_eq!(cycle(5, 1, 3), 0);
        assert_eq!(cycle(0, 1, 0), 0);
    }

    #[test]
    fn page_is_a_clamped_step() {
        assert_eq!(page(0, 1, 10, 25), 10);
        assert_eq!(page(20, 1, 10, 25), 24);
        assert_eq!(page(3, -1, 10, 25), 0);
    }
}
