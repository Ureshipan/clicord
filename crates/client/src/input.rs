//! A small single-line text input with a movable caret, shared by every
//! editable field (server address, login fields, chat composer).
//!
//! `cursor` is a character index in `0..=len`. All editing keeps the caret and
//! the (UTF-8) byte offsets consistent.

#[derive(Default, Clone)]
pub struct TextInput {
    value: String,
    cursor: usize,
}

impl TextInput {
    pub fn with(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        Self { value, cursor }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn len(&self) -> usize {
        self.value.chars().count()
    }

    /// Replace the whole contents and park the caret at the end.
    pub fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.chars().count();
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    pub fn insert(&mut self, c: char) {
        let at = self.byte_of(self.cursor);
        self.value.insert(at, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_of(self.cursor - 1);
        let end = self.byte_of(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.len() {
            return;
        }
        let start = self.byte_of(self.cursor);
        let end = self.byte_of(self.cursor + 1);
        self.value.replace_range(start..end, "");
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        if self.cursor < self.len() {
            self.cursor += 1;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.len();
    }

    /// Byte offset of character index `ci` (or the string length at the end).
    fn byte_of(&self, ci: usize) -> usize {
        self.value
            .char_indices()
            .nth(ci)
            .map(|(b, _)| b)
            .unwrap_or(self.value.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_move() {
        let mut t = TextInput::default();
        for c in "helo".chars() {
            t.insert(c);
        }
        // caret at end; move left once and insert 'l' -> "hello"
        t.left();
        t.insert('l');
        assert_eq!(t.value(), "hello");
        assert_eq!(t.cursor(), 4);
    }

    #[test]
    fn home_end_delete() {
        let mut t = TextInput::with("abc");
        t.home();
        t.delete(); // removes 'a'
        assert_eq!(t.value(), "bc");
        assert_eq!(t.cursor(), 0);
        t.end();
        t.backspace(); // removes 'c'
        assert_eq!(t.value(), "b");
    }

    #[test]
    fn unicode_safe() {
        let mut t = TextInput::with("привет");
        t.home();
        t.right();
        t.insert('!');
        assert_eq!(t.value(), "п!ривет");
    }
}
