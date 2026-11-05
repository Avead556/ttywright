const SCROLLBACK: usize = 5000;

pub fn new_screen() -> vt100::Parser {
    vt100::Parser::new(24, 80, SCROLLBACK)
}

pub fn feed_screen(parser: &mut vt100::Parser, bytes: &[u8]) -> Vec<u8> {
    parser.process(bytes);
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    let mut answer = Vec::new();
    if has(b"\x1b[6n") {
        let (row, column) = parser.screen().cursor_position();
        answer.extend_from_slice(format!("\x1b[{};{}R", row + 1, column + 1).as_bytes());
    }
    if has(b"\x1b[c") {
        answer.extend_from_slice(b"\x1b[?62c");
    }
    answer
}

pub fn scroll_screen(parser: &mut vt100::Parser, delta: isize) {
    let screen = parser.screen_mut();
    screen.set_scrollback(screen.scrollback().saturating_add_signed(delta));
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ClaudeState {
    Waiting,
    Running,
    Exited(Option<i32>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeSend {
    Notify {
        method: String,
        params: serde_json::Value,
    },
    Type(String),
}

pub struct ClaudePanel {
    pub screen: vt100::Parser,
    pub state: ClaudeState,
    pub connected: bool,
    pub pending: Vec<ClaudeSend>,
    pub size: Option<(u16, u16)>,
}

impl ClaudePanel {
    pub fn new() -> Self {
        Self {
            screen: new_screen(),
            state: ClaudeState::Waiting,
            connected: false,
            pending: Vec::new(),
            size: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.state == ClaudeState::Running
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        feed_screen(&mut self.screen, bytes)
    }

    pub fn resize(&mut self, rows: u16, columns: u16) {
        self.size = Some((rows, columns));
        self.screen.screen_mut().set_size(rows, columns);
    }

    pub fn scroll_by(&mut self, delta: isize) {
        scroll_screen(&mut self.screen, delta);
    }
}

impl Default for ClaudePanel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screen_shows_what_the_program_drew() {
        let mut panel = ClaudePanel::new();
        panel.resize(5, 20);
        panel.feed(b"hi \x1b[31mred\x1b[0m\r\nnext");
        let contents = panel.screen.screen().contents();
        assert!(contents.starts_with("hi red\nnext"), "{contents}");
    }

    #[test]
    fn questions_to_the_terminal_get_answers() {
        let mut panel = ClaudePanel::new();
        panel.resize(5, 20);
        assert_eq!(panel.feed(b"\x1b[2;3H\x1b[6n"), b"\x1b[2;3R");
        assert_eq!(panel.feed(b"\x1b[c"), b"\x1b[?62c");
        assert_eq!(panel.feed(b"plain"), b"");
    }

    #[test]
    fn the_wheel_goes_back_through_what_scrolled_off() {
        let mut panel = ClaudePanel::new();
        panel.resize(2, 10);
        panel.feed(b"one\r\ntwo\r\nthree\r\nfour");
        panel.scroll_by(1);
        assert!(panel.screen.screen().contents().contains("two"));
        panel.scroll_by(-5);
        assert!(panel.screen.screen().contents().contains("four"));
    }
}
