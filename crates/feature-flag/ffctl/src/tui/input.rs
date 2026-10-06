use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Default)]
pub struct Input {
    value: String,
    cursor: usize,
}

impl Input {
    pub fn with_value(value: &str) -> Self {
        Self {
            value: value.to_owned(),
            cursor: value.chars().count(),
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set(&mut self, value: &str) {
        *self = Self::with_value(value);
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn byte_index(&self, position: usize) -> usize {
        self.value
            .char_indices()
            .nth(position)
            .map(|(index, _)| index)
            .unwrap_or(self.value.len())
    }

    fn len(&self) -> usize {
        self.value.chars().count()
    }

    pub fn handle(&mut self, key: KeyEvent) -> bool {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Char('u') if control => self.clear(),
            KeyCode::Char('a') if control => self.cursor = 0,
            KeyCode::Char('e') if control => self.cursor = self.len(),
            KeyCode::Char(_) if control => return false,
            KeyCode::Char(character) => {
                let index = self.byte_index(self.cursor);
                self.value.insert(index, character);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let index = self.byte_index(self.cursor - 1);
                self.value.remove(index);
                self.cursor -= 1;
            }
            KeyCode::Delete if self.cursor < self.len() => {
                let index = self.byte_index(self.cursor);
                self.value.remove(index);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.len(),
            _ => return false,
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(input: &mut Input, code: KeyCode) {
        input.handle(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn edits_respect_the_cursor_and_multibyte_characters() {
        let mut input = Input::with_value("añb");

        press(&mut input, KeyCode::Left);
        press(&mut input, KeyCode::Backspace);
        assert_eq!(input.value(), "ab");
        assert_eq!(input.cursor(), 1);

        press(&mut input, KeyCode::Char('é'));
        assert_eq!(input.value(), "aéb");

        press(&mut input, KeyCode::Home);
        press(&mut input, KeyCode::Delete);
        assert_eq!(input.value(), "éb");

        press(&mut input, KeyCode::Backspace);
        assert_eq!(input.value(), "éb");
    }
}
