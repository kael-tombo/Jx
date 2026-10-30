//! jx's error-rendering framework.
//!
//! Principle 3 of the brief: every error states what happened, where, and
//! what to try next. [`JxError`] carries those three things plus an optional
//! cause chain; the CLI renders it, [`error::JxError::exit_code`] maps it to
//! a stable exit code (documented in docs/exit-codes.md).

use std::fmt;
use std::fmt::Write as _;

use crate::color;

/// Stable, open-ended error categories. The category drives the exit code
/// and gives scripts something reliable to match on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Bad usage of the jx command line. Exit code 2.
    Usage,
    /// Local filesystem / environment problem.
    Io,
    /// Network or registry problems.
    Network,
    /// JDK discovery, download or activation problems.
    Toolchain,
    /// project.toml problems.
    Manifest,
    /// jx.lock problems (stale, corrupt, mismatched checksums).
    Lock,
    /// Dependency resolution problems (conflicts, unsupported features).
    Resolve,
    /// Compilation, test or run problems.
    Build,
    /// A bug in jx. Always worth an issue from the user's point of view.
    Internal,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Usage => "usage",
            Kind::Io => "io",
            Kind::Network => "network",
            Kind::Toolchain => "toolchain",
            Kind::Manifest => "manifest",
            Kind::Lock => "lock",
            Kind::Resolve => "resolve",
            Kind::Build => "build",
            Kind::Internal => "internal",
        }
    }
}

/// A diagnosed jx error.
///
/// Construct with [`JxError::new`] and refine with [`JxError::at`] and
/// [`JxError::hint`]. `location` names the artifact, file, URL or command
/// involved; `hint` is the "what to try next" line.
#[derive(Debug)]
pub struct JxError {
    pub kind: Kind,
    pub message: String,
    pub location: Option<String>,
    pub hint: Option<String>,
    pub source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl JxError {
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        JxError {
            kind,
            message: message.into(),
            location: None,
            hint: None,
            source: None,
        }
    }

    /// Attach a location: a file path, artifact coordinate, URL or command.
    pub fn at(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    /// Attach the "what to try next" line.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Attach an underlying cause (shown fully under `-v`).
    pub fn caused_by(
        mut self,
        source: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    ) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Exit code for this error. See docs/exit-codes.md.
    pub fn exit_code(&self) -> i32 {
        match self.kind {
            Kind::Usage => 2,
            _ => 1,
        }
    }

    /// Render the full multi-line, possibly coloured message.
    pub fn render(&self, color_when: color::When, verbose: bool) -> String {
        let c = crate::color::Palette::from(color::enabled(color_when));
        let mut out = String::new();
        let _ = write!(
            out,
            "{}error{} [{}{}{}]: {}",
            c.error(),
            c.reset(),
            c.dim(),
            self.kind.as_str(),
            c.reset(),
            self.message
        );
        if let Some(loc) = &self.location {
            let _ = write!(out, "\n  {}at: {}{}", c.dim(), loc, c.reset());
        }
        if verbose {
            let mut chain: Vec<String> = Vec::new();
            let mut src: Option<&(dyn std::error::Error + 'static)> = self
                .source
                .as_ref()
                .map(|e| e.as_ref() as &(dyn std::error::Error + 'static));
            while let Some(e) = src {
                chain.push(e.to_string());
                src = e.source();
            }
            for cause in chain {
                let _ = write!(out, "\n  {}caused by: {}{}", c.dim(), cause, c.reset());
            }
        }
        if let Some(hint) = &self.hint {
            let _ = write!(out, "\n  {}hint{}: {}", c.hint(), c.dim(), hint);
        }
        out
    }
}

impl fmt::Display for JxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error [{}]: {}", self.kind.as_str(), self.message)?;
        if let Some(loc) = &self.location {
            write!(f, "\n  at: {loc}")?;
        }
        if let Some(hint) = &self.hint {
            write!(f, "\n  hint: {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for JxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn std::error::Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_message_location_and_hint() {
        let e = JxError::new(Kind::Manifest, "unknown key `dependancies`")
            .at("project.toml:7")
            .hint("did you mean `dependencies`?");
        let s = e.render(color::When::Never, false);
        assert!(s.contains("error [manifest]: unknown key `dependancies`"));
        assert!(s.contains("at: project.toml:7"));
        assert!(s.contains("hint: did you mean `dependencies`?"));
    }

    #[test]
    fn verbose_includes_cause_chain() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let e = JxError::new(Kind::Io, "cannot read jx.lock").caused_by(io);
        let quiet = e.render(color::When::Never, false);
        let verbose = e.render(color::When::Never, true);
        assert!(!quiet.contains("caused by"));
        assert!(verbose.contains("caused by: no such file"));
    }

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(JxError::new(Kind::Usage, "x").exit_code(), 2);
        assert_eq!(JxError::new(Kind::Network, "x").exit_code(), 1);
    }
}
