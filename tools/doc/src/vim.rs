//! Talking to Neovim safely: text goes in as a quoted string, never as a
//! command line or keystrokes, so a file named `x'|!rm` stays a name.

/// `text` as a Vim string literal (`'…'`, with `'` doubled); none when it
/// has a control character, which no literal may carry.
pub fn string(text: &str) -> Option<String> {
    (!text.chars().any(char::is_control)).then(|| format!("'{}'", text.replace('\'', "''")))
}
