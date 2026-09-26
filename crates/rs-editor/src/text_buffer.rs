//! Text editing buffer for inline canvas text (pure logic).
//!
//! Byte offsets (UTF-8) internally; UTF-16 conversions for the platform
//! input handler (IME). Selection is `sel` (start ≤ end) plus `reversed`
//! (caret at `start` when true). `marked` = IME composition range.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextBuffer {
    pub text: String,
    pub sel: Range<usize>,
    pub reversed: bool,
    pub marked: Option<Range<usize>>,
    undo: Vec<(String, Range<usize>)>,
    redo: Vec<(String, Range<usize>)>,
}

impl TextBuffer {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let n = text.len();
        Self { text, sel: n..n, ..Default::default() }
    }

    /// Caret offset (the moving end of the selection).
    pub fn cursor(&self) -> usize {
        if self.reversed { self.sel.start } else { self.sel.end }
    }

    /// The fixed end of the selection.
    pub fn anchor(&self) -> usize {
        if self.reversed { self.sel.end } else { self.sel.start }
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.sel.clone()]
    }

    fn clamp(&self, i: usize) -> usize {
        let mut i = i.min(self.text.len());
        while !self.text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    /// Move the caret to `i`; with `extend`, keep the anchor (selection).
    pub fn set_cursor(&mut self, i: usize, extend: bool) {
        let i = self.clamp(i);
        if extend {
            let anchor = self.anchor();
            if i < anchor {
                self.sel = i..anchor;
                self.reversed = true;
            } else {
                self.sel = anchor..i;
                self.reversed = false;
            }
        } else {
            self.sel = i..i;
            self.reversed = false;
        }
    }

    pub fn select_all(&mut self) {
        self.sel = 0..self.text.len();
        self.reversed = false;
    }

    /// Select the word around `i` (double-click in text).
    pub fn select_word_at(&mut self, i: usize) {
        let i = self.clamp(i);
        for (start, w) in self.text.split_word_bound_indices() {
            let end = start + w.len();
            if i >= start && i <= end && !w.trim().is_empty() {
                self.sel = start..end;
                self.reversed = false;
                return;
            }
        }
        self.set_cursor(i, false);
    }

    fn snapshot(&mut self) {
        if self.undo.last().is_none_or(|(t, _)| *t != self.text) {
            self.undo.push((self.text.clone(), self.sel.clone()));
        }
        self.redo.clear();
    }

    /// Local undo while editing (⌘Z inside the text box).
    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some((t, s)) => {
                self.redo.push((std::mem::replace(&mut self.text, t), self.sel.clone()));
                self.sel = s;
                self.reversed = false;
                self.marked = None;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some((t, s)) => {
                self.undo.push((std::mem::replace(&mut self.text, t), self.sel.clone()));
                self.sel = s;
                self.reversed = false;
                self.marked = None;
                true
            }
            None => false,
        }
    }

    /// Replace `range` (default: marked range, else selection) with `s`.
    pub fn replace(&mut self, range: Option<Range<usize>>, s: &str) {
        let r = range.or_else(|| self.marked.clone()).unwrap_or(self.sel.clone());
        let r = self.clamp(r.start)..self.clamp(r.end.max(r.start));
        // Group typing into words for undo: snapshot at word boundaries.
        if s.contains(char::is_whitespace) || s.is_empty() || !r.is_empty() || self.undo.is_empty() {
            self.snapshot();
        }
        self.text.replace_range(r.clone(), s);
        let c = r.start + s.len();
        self.sel = c..c;
        self.reversed = false;
        self.marked = None;
    }

    /// Replace and mark (IME composition). `new_sel` is relative to the
    /// inserted text.
    pub fn replace_and_mark(&mut self, range: Option<Range<usize>>, s: &str, new_sel: Option<Range<usize>>) {
        let r = range.or_else(|| self.marked.clone()).unwrap_or(self.sel.clone());
        let r = self.clamp(r.start)..self.clamp(r.end.max(r.start));
        self.text.replace_range(r.clone(), s);
        self.marked = (!s.is_empty()).then(|| r.start..r.start + s.len());
        let sel = new_sel
            .map(|n| r.start + n.start.min(s.len())..r.start + n.end.min(s.len()))
            .unwrap_or(r.start + s.len()..r.start + s.len());
        self.sel = self.clamp(sel.start)..self.clamp(sel.end);
        self.reversed = false;
    }

    pub fn insert(&mut self, s: &str) {
        self.replace(None, s);
    }

    pub fn prev_boundary(&self, i: usize) -> usize {
        self.text.grapheme_indices(true).rev().find_map(|(ix, _)| (ix < i).then_some(ix)).unwrap_or(0)
    }

    pub fn next_boundary(&self, i: usize) -> usize {
        self.text.grapheme_indices(true).find_map(|(ix, _)| (ix > i).then_some(ix)).unwrap_or(self.text.len())
    }

    pub fn prev_word(&self, i: usize) -> usize {
        let mut last = 0;
        for (start, w) in self.text.split_word_bound_indices() {
            if start >= i {
                break;
            }
            if !w.trim().is_empty() {
                last = start;
            }
        }
        last
    }

    pub fn next_word(&self, i: usize) -> usize {
        for (start, w) in self.text.split_word_bound_indices() {
            let end = start + w.len();
            if end > i && !w.trim().is_empty() {
                return end;
            }
        }
        self.text.len()
    }

    /// Start of the logical line containing `i`.
    pub fn line_start(&self, i: usize) -> usize {
        self.text[..self.clamp(i)].rfind('\n').map(|p| p + 1).unwrap_or(0)
    }

    /// End of the logical line containing `i` (before the '\n').
    pub fn line_end(&self, i: usize) -> usize {
        let i = self.clamp(i);
        self.text[i..].find('\n').map(|p| i + p).unwrap_or(self.text.len())
    }

    pub fn backspace(&mut self) {
        if self.sel.is_empty() {
            let c = self.cursor();
            if c == 0 {
                return;
            }
            let p = self.prev_boundary(c);
            self.replace(Some(p..c), "");
        } else {
            self.replace(None, "");
        }
    }

    pub fn backspace_word(&mut self) {
        if self.sel.is_empty() {
            let c = self.cursor();
            let p = self.prev_word(c);
            self.replace(Some(p..c), "");
        } else {
            self.replace(None, "");
        }
    }

    pub fn delete_forward(&mut self) {
        if self.sel.is_empty() {
            let c = self.cursor();
            if c >= self.text.len() {
                return;
            }
            let n = self.next_boundary(c);
            self.replace(Some(c..n), "");
        } else {
            self.replace(None, "");
        }
    }

    pub fn move_left(&mut self, extend: bool) {
        if !extend && !self.sel.is_empty() {
            let s = self.sel.start;
            self.set_cursor(s, false);
        } else {
            let p = self.prev_boundary(self.cursor());
            self.set_cursor(p, extend);
        }
    }

    pub fn move_right(&mut self, extend: bool) {
        if !extend && !self.sel.is_empty() {
            let e = self.sel.end;
            self.set_cursor(e, false);
        } else {
            let n = self.next_boundary(self.cursor());
            self.set_cursor(n, extend);
        }
    }

    pub fn word_left(&mut self, extend: bool) {
        let p = self.prev_word(self.cursor());
        self.set_cursor(p, extend);
    }

    pub fn word_right(&mut self, extend: bool) {
        let n = self.next_word(self.cursor());
        self.set_cursor(n, extend);
    }

    pub fn home(&mut self, extend: bool) {
        let p = self.line_start(self.cursor());
        self.set_cursor(p, extend);
    }

    pub fn end(&mut self, extend: bool) {
        let p = self.line_end(self.cursor());
        self.set_cursor(p, extend);
    }

    // ---- UTF-16 <-> UTF-8 ----

    pub fn to_utf16(&self, offset: usize) -> usize {
        let mut u16s = 0;
        let mut u8s = 0;
        for ch in self.text.chars() {
            if u8s >= offset {
                break;
            }
            u8s += ch.len_utf8();
            u16s += ch.len_utf16();
        }
        u16s
    }

    pub fn from_utf16(&self, offset: usize) -> usize {
        let mut u16s = 0;
        let mut u8s = 0;
        for ch in self.text.chars() {
            if u16s >= offset {
                break;
            }
            u16s += ch.len_utf16();
            u8s += ch.len_utf8();
        }
        u8s
    }

    pub fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.to_utf16(r.start)..self.to_utf16(r.end)
    }

    pub fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.from_utf16(r.start)..self.from_utf16(r.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_backspace() {
        let mut b = TextBuffer::new("");
        b.insert("Hello");
        b.insert(" ");
        b.insert("wörld");
        assert_eq!(b.text, "Hello wörld");
        b.backspace();
        assert_eq!(b.text, "Hello wörl");
        b.move_left(false);
        b.move_left(false);
        b.move_left(false);
        b.backspace();
        assert_eq!(b.text, "Hello örl");
        assert_eq!(b.cursor(), 6);
    }

    #[test]
    fn selection_extend_and_replace() {
        let mut b = TextBuffer::new("abcdef");
        b.set_cursor(3, false);
        b.move_right(true);
        b.move_right(true);
        assert_eq!(b.selected_text(), "de");
        b.move_left(true);
        b.move_left(true);
        b.move_left(true);
        assert_eq!(b.selected_text(), "c");
        assert!(b.reversed);
        b.insert("X");
        assert_eq!(b.text, "abXdef");
        b.select_all();
        assert_eq!(b.selected_text(), "abXdef");
        b.move_right(false);
        assert_eq!(b.cursor(), 6);
    }

    #[test]
    fn lines_and_words() {
        let mut b = TextBuffer::new("one two\nthree four");
        b.set_cursor(10, false);
        assert_eq!(b.line_start(10), 8);
        assert_eq!(b.line_end(10), 18);
        assert_eq!(b.line_end(2), 7);
        b.home(false);
        assert_eq!(b.cursor(), 8);
        b.end(true);
        assert_eq!(b.selected_text(), "three four");
        assert_eq!(b.prev_word(7), 4);
        assert_eq!(b.next_word(0), 3);
        b.set_cursor(7, false);
        b.backspace_word();
        assert_eq!(b.text, "one \nthree four");
        b.select_word_at(12);
        assert_eq!(b.selected_text(), "four");
    }

    #[test]
    fn undo_redo() {
        let mut b = TextBuffer::new("");
        b.insert("a");
        b.insert("b");
        b.insert(" ");
        b.insert("c");
        assert!(b.undo());
        assert!(b.text.len() < 4);
        while b.undo() {}
        assert_eq!(b.text, "");
        assert!(b.redo());
        assert!(!b.text.is_empty());
    }

    #[test]
    fn ime_marking_and_utf16() {
        let mut b = TextBuffer::new("a😀b");
        assert_eq!(b.to_utf16(b.text.len()), 4);
        assert_eq!(b.from_utf16(3), 5);
        b.set_cursor(b.text.len(), false);
        b.replace_and_mark(None, "に", None);
        assert_eq!(b.marked, Some(6..9));
        b.replace_and_mark(None, "日本", Some(0..0));
        assert_eq!(b.text, "a😀b日本");
        assert_eq!(b.cursor(), 6);
        b.replace(None, "日本語");
        assert_eq!(b.text, "a😀b日本語");
        assert!(b.marked.is_none());
        // Out-of-range / mid-char offsets are clamped.
        b.set_cursor(2, false);
        assert!(b.text.is_char_boundary(b.cursor()));
    }
}
