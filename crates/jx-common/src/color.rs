//! Colour decision for jx output.
//!
//! Rules: `--color always|never` wins; otherwise auto means "colour only if
//! stderr is a terminal and neither `NO_COLOR` nor `TERM=dumb` is set".
//! No colours contain semantic meaning that is lost without them.

use std::io::IsTerminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum When {
    #[default]
    Auto,
    Always,
    Never,
}

impl When {
    /// Parse the value of a `--color` flag; unknown values are a usage error.
    pub fn parse(s: &str) -> Result<When, crate::error::JxError> {
        match s {
            "auto" => Ok(When::Auto),
            "always" => Ok(When::Always),
            "never" => Ok(When::Never),
            other => Err(crate::error::JxError::new(
                crate::error::Kind::Usage,
                format!("unknown value `{other}` for --color"),
            )
            .hint("expected one of: auto, always, never")),
        }
    }
}

/// Decide whether to colour output going to `stderr`.
pub fn enabled(when: When) -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    match when {
        When::Always => true,
        When::Never => false,
        When::Auto => {
            !matches!(std::env::var("TERM").as_deref(), Ok("dumb"))
                && std::io::stderr().is_terminal()
        }
    }
}

/// Small ANSI helper; all styles are no-ops when colours are disabled.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub on: bool,
}

impl Palette {
    pub fn on() -> Self {
        Palette { on: true }
    }
    pub fn off() -> Self {
        Palette { on: false }
    }

    pub fn error(&self) -> &'static str {
        self.codes("\x1b[31;1m")
    }
    pub fn warn(&self) -> &'static str {
        self.codes("\x1b[33;1m")
    }
    pub fn ok(&self) -> &'static str {
        self.codes("\x1b[32m")
    }
    pub fn hint(&self) -> &'static str {
        self.codes("\x1b[36;1m")
    }
    pub fn dim(&self) -> &'static str {
        self.codes("\x1b[2m")
    }
    pub fn reset(&self) -> &'static str {
        self.codes("\x1b[0m")
    }

    fn codes(&self, code: &'static str) -> &'static str {
        if self.on { code } else { "" }
    }
}

impl From<bool> for Palette {
    fn from(on: bool) -> Self {
        Palette { on }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_on_off_styles() {
        assert_eq!(Palette::off().error(), "");
        assert_eq!(Palette::off().reset(), "");
        assert_ne!(Palette::on().error(), "");
        assert_ne!(Palette::on().hint(), "");
    }
}
