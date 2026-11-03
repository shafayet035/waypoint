//! Data loading for text-first projects (design principle 1: text is the
//! source of truth; principle 2: everything has a schema).
//!
//! Parse errors become [`Diagnostic`]s with the file, line and column, so an
//! agent can jump straight to the problem.

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use std::path::Path;
use wp_core::Diagnostic;

/// A parsed file together with its source text (kept for locating fields).
#[derive(Clone, Debug)]
pub struct Loaded<T> {
    pub value: T,
    /// Path relative to the project root, with `/` separators.
    pub path: String,
    pub source: String,
}

impl<T> Loaded<T> {
    /// Best-effort location of `key:` (RON) or `key =` (TOML) in the source,
    /// for diagnostics about semantically invalid values.
    pub fn locate(&self, key: &str) -> (Option<usize>, Option<usize>) {
        locate(&self.source, key)
    }

    /// An error diagnostic pointing at `key` in this file.
    pub fn error_at(&self, code: &str, key: &str, message: impl Into<String>) -> Diagnostic {
        let (line, col) = self.locate(key);
        Diagnostic::error(code, message).at(self.path.clone(), line, col)
    }

    pub fn warning_at(&self, code: &str, key: &str, message: impl Into<String>) -> Diagnostic {
        let (line, col) = self.locate(key);
        Diagnostic::warning(code, message).at(self.path.clone(), line, col)
    }
}

/// Find the first `key:` or `key =` that is not inside a comment.
pub fn locate(source: &str, key: &str) -> (Option<usize>, Option<usize>) {
    for (i, line) in source.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        let code = code.split('#').next().unwrap_or("");
        let mut search = 0;
        while let Some(pos) = code[search..].find(key) {
            let start = search + pos;
            let before_ok = start == 0
                || !code[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
            let rest = code[start + key.len()..].trim_start();
            if before_ok && (rest.starts_with(':') || rest.starts_with('=')) {
                return (Some(i + 1), Some(start + 1));
            }
            search = start + key.len();
        }
    }
    (None, None)
}

fn read(root: &Path, rel: &str) -> Result<String, Diagnostic> {
    std::fs::read_to_string(root.join(rel)).map_err(|e| {
        Diagnostic::error("WP-D001", format!("cannot read file: {e}"))
            .in_file(rel)
            .help("check the path; paths are relative to the project root")
    })
}

/// Parse RON text. `path` is only used in diagnostics.
pub fn parse_ron<T: DeserializeOwned>(path: &str, source: &str) -> Result<T, Diagnostic> {
    let options = ron::Options::default().with_default_extension(
        ron::extensions::Extensions::IMPLICIT_SOME | ron::extensions::Extensions::UNWRAP_NEWTYPES,
    );
    options.from_str::<T>(source).map_err(|e| {
        let pos = e.span.start;
        Diagnostic::error("WP-D002", format!("invalid RON: {}", e.code))
            .at(path, Some(pos.line), Some(pos.col))
            .help("run `wp schema <type>` to see the expected structure")
    })
}

/// Parse TOML text. `path` is only used in diagnostics.
pub fn parse_toml<T: DeserializeOwned>(path: &str, source: &str) -> Result<T, Diagnostic> {
    toml::from_str::<T>(source).map_err(|e| {
        let (line, col) = e
            .span()
            .map(|span| line_col(source, span.start))
            .map(|(l, c)| (Some(l), Some(c)))
            .unwrap_or((None, None));
        Diagnostic::error("WP-D003", format!("invalid TOML: {}", e.message()))
            .at(path, line, col)
            .help("run `wp schema game` to see the expected structure")
    })
}

pub fn load_ron<T: DeserializeOwned>(root: &Path, rel: &str) -> Result<Loaded<T>, Diagnostic> {
    let source = read(root, rel)?;
    let value = parse_ron(rel, &source)?;
    Ok(Loaded {
        value,
        path: rel.to_string(),
        source,
    })
}

pub fn load_toml<T: DeserializeOwned>(root: &Path, rel: &str) -> Result<Loaded<T>, Diagnostic> {
    let source = read(root, rel)?;
    let value = parse_toml(rel, &source)?;
    Ok(Loaded {
        value,
        path: rel.to_string(),
        source,
    })
}

/// 1-based line and column of a byte offset.
pub fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let before = &source[..offset.min(source.len())];
    let line = before.matches('\n').count() + 1;
    let col = before
        .rsplit('\n')
        .next()
        .map(|l| l.chars().count())
        .unwrap_or(0)
        + 1;
    (line, col)
}

/// The JSON Schema of a data type, as pretty JSON.
pub fn schema_json<T: JsonSchema>() -> String {
    serde_json::to_string_pretty(&schemars::schema_for!(T)).expect("schema serializes")
}

/// Every file under `root` with the given suffix (e.g. `.vehicle.ron`), as
/// sorted root-relative paths. Skips `target/` and hidden directories.
pub fn find_files(root: &Path, suffix: &str) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, root, suffix, &mut out);
    out.sort();
    out
}

fn walk(root: &Path, dir: &Path, suffix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        if path.is_dir() {
            if name.starts_with('.') || name == "target" {
                continue;
            }
            walk(root, &path, suffix, out);
        } else if name.ends_with(suffix) {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Car {
        mass: f32,
        gears: Vec<f32>,
    }

    #[test]
    fn ron_errors_have_positions() {
        let src = "(\n  mass: 1200.0,\n  colour: 3,\n)";
        let err = parse_ron::<Car>("car.ron", src).unwrap_err();
        assert_eq!(err.code, "WP-D002");
        assert_eq!(err.location.unwrap().line, Some(3));
    }

    #[test]
    fn locate_skips_comments_and_prefixes() {
        let src = "// gears: nope\n(\n  top_gears: 1,\n  gears: [1.0],\n)";
        assert_eq!(locate(src, "gears"), (Some(4), Some(3)));
    }

    #[test]
    fn line_col_counts_from_one() {
        assert_eq!(line_col("ab\ncd", 3), (2, 1));
    }
}
