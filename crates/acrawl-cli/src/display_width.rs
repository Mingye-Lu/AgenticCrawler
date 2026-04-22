use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) fn char_display_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

pub(crate) fn text_display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub(crate) fn prefix_display_width(text: &str, char_count: usize) -> usize {
    text.chars().take(char_count).map(char_display_width).sum()
}

pub(crate) fn char_count_for_display_col(text: &str, target_col: usize) -> usize {
    let mut char_count = 0usize;
    let mut width = 0usize;

    for ch in text.chars() {
        let ch_width = char_display_width(ch);
        if ch_width > 0 && width + ch_width > target_col {
            break;
        }
        width += ch_width;
        char_count += 1;
    }

    char_count
}

pub(crate) fn split_at_display_width(text: &str, max_width: usize) -> (usize, usize) {
    let mut end = 0usize;
    let mut width = 0usize;
    let mut saw_char = false;

    for (idx, ch) in text.char_indices() {
        let ch_width = char_display_width(ch);
        if saw_char && width + ch_width > max_width {
            break;
        }
        saw_char = true;
        width += ch_width;
        end = idx + ch.len_utf8();
        if width >= max_width && max_width > 0 {
            break;
        }
    }

    if saw_char {
        (end, width)
    } else {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        char_count_for_display_col, char_display_width, prefix_display_width,
        split_at_display_width, text_display_width,
    };

    #[test]
    fn wide_char_display_widths_match_terminal_cells() {
        assert_eq!(char_display_width('a'), 1);
        assert_eq!(char_display_width('中'), 2);
        assert_eq!(text_display_width("a中b"), 4);
        assert_eq!(prefix_display_width("中b", 1), 2);
    }

    #[test]
    fn display_col_maps_back_to_char_count() {
        assert_eq!(char_count_for_display_col("中b", 0), 0);
        assert_eq!(char_count_for_display_col("中b", 1), 0);
        assert_eq!(char_count_for_display_col("中b", 2), 1);
        assert_eq!(char_count_for_display_col("中b", 3), 2);
    }

    #[test]
    fn split_respects_wide_char_boundaries() {
        let (idx, width) = split_at_display_width("ab中cd", 4);
        assert_eq!(&"ab中cd"[..idx], "ab中");
        assert_eq!(width, 4);
    }
}
