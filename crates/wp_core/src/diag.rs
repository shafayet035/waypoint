//! Diagnostics: every error the engine reports has a stable code, a location,
//! what was expected and a suggested fix (design principle 6, "Errors teach").

use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// A position in a source file. Lines and columns start at 1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Location {
    pub file: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file)?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
            if let Some(col) = self.column {
                write!(f, ":{col}")?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable code such as `WP-V012`. Never reused for a different meaning.
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: code.to_string(),
            message: message.into(),
            location: None,
            expected: None,
            help: None,
        }
    }

    pub fn warning(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, message)
        }
    }

    pub fn at(
        mut self,
        file: impl Into<String>,
        line: Option<usize>,
        column: Option<usize>,
    ) -> Self {
        self.location = Some(Location {
            file: file.into(),
            line,
            column,
        });
        self
    }

    pub fn in_file(self, file: impl Into<String>) -> Self {
        self.at(file, None, None)
    }

    pub fn expected(mut self, expected: impl Into<String>) -> Self {
        self.expected = Some(expected.into());
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Render in the compiler-style format agents and humans both read.
    pub fn render(&self) -> String {
        let kind = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let mut out = format!("{kind}[{}]: {}\n", self.code, self.message);
        if let Some(loc) = &self.location {
            out.push_str(&format!("  --> {loc}\n"));
        }
        if let Some(e) = &self.expected {
            out.push_str(&format!("  expected {e}\n"));
        }
        if let Some(h) = &self.help {
            out.push_str(&format!("  help: {h}\n"));
        }
        out.push_str(&format!("  docs: wp docs errors/{}\n", self.code));
        out
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

impl std::error::Error for Diagnostic {}

/// An ordered collection of diagnostics.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(transparent)]
pub struct Diagnostics(pub Vec<Diagnostic>);

impl Diagnostics {
    pub fn push(&mut self, d: Diagnostic) {
        self.0.push(d);
    }
    pub fn extend(&mut self, ds: impl IntoIterator<Item = Diagnostic>) {
        self.0.extend(ds);
    }
    pub fn has_errors(&self) -> bool {
        self.0.iter().any(Diagnostic::is_error)
    }
    pub fn error_count(&self) -> usize {
        self.0.iter().filter(|d| d.is_error()).count()
    }
    pub fn warning_count(&self) -> usize {
        self.0.len() - self.error_count()
    }
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_in_compiler_style() {
        let d = Diagnostic::error("WP-V012", "gearbox has no ratios")
            .at("vehicles/car.vehicle.ron", Some(41), Some(5))
            .expected("at least 1 ratio")
            .help("add ratios, e.g. [3.6, 2.1, 1.4]");
        let text = d.render();
        assert!(text.starts_with("error[WP-V012]: gearbox has no ratios\n"));
        assert!(text.contains("  --> vehicles/car.vehicle.ron:41:5\n"));
        assert!(text.contains("  docs: wp docs errors/WP-V012\n"));
    }
}
