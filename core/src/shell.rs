//! Quoting for `sh`: the one way a path or an id becomes a word of a shell
//! line (the `~/.zshrc` block, the shim, "Copy shell override", the line
//! Resume in Terminal runs). The app's own folder has a space in its path,
//! and a project folder can hold anything.

use std::path::Path;

/// `text` as one `sh` word: single-quoted, with `'` written as `'\''`.
pub(crate) fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `path` as one `sh` word.
pub(crate) fn quote_path(path: &Path) -> String {
    quote(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_stays_one_word_whatever_it_holds() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(
            quote_path(Path::new("/Users/l/Application Support/x")),
            "'/Users/l/Application Support/x'"
        );
        assert_eq!(quote("it's; rm -rf $HOME"), r"'it'\''s; rm -rf $HOME'");
    }
}
