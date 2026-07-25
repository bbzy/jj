// Copyright 2025 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language and permissions and
// limitations under the License.

//! `.gitmodules` file parsing.
//!
//! The `.gitmodules` file defines Git submodule configuration. It uses the
//! same INI-like syntax as Git config files:
//!
//! ```text
//! [submodule "vendor"]
//!     path = vendor
//!     url = https://example.com/vendor.git
//!     branch = main
//! ```

use std::collections::HashMap;

use crate::repo_path::RepoPath;
use crate::repo_path::RepoPathBuf;

/// A single submodule entry parsed from `.gitmodules`.
///
/// This is part of the foundation for submodule support. Not yet integrated
/// into the checkout/snapshot pipeline.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmoduleEntry {
    /// The logical name of the submodule (the string in `[submodule "name"]`).
    pub name: String,
    /// The path where the submodule is checked out, relative to the repo root.
    pub path: RepoPathBuf,
    /// The URL of the submodule repository.
    pub url: String,
    /// The branch to track, if specified.
    pub branch: Option<String>,
    /// The update strategy, if specified (e.g. "checkout", "rebase", "merge",
    /// "none").
    pub update: Option<String>,
}

/// A parsed `.gitmodules` file.
///
/// This is part of the foundation for submodule support. Not yet integrated
/// into the checkout/snapshot pipeline.
#[allow(dead_code)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitModules {
    /// Submodule entries keyed by their path.
    by_path: HashMap<RepoPathBuf, SubmoduleEntry>,
    /// Submodule entries keyed by their name.
    by_name: HashMap<String, SubmoduleEntry>,
}

impl GitModules {
    /// Parses `.gitmodules` content from bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, GitModulesError> {
        let text = std::str::from_utf8(bytes).map_err(|err| GitModulesError {
            message: format!(".gitmodules is not valid UTF-8: {err}"),
        })?;
        Self::parse_str(text)
    }

    /// Parses `.gitmodules` content from a string.
    fn parse_str(text: &str) -> Result<Self, GitModulesError> {
        let mut modules = Self::default();
        let mut current_entry: Option<SubmoduleEntry> = None;

        for (line_num, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') {
                // Finish the previous entry.
                if let Some(entry) = current_entry.take() {
                    modules.add(entry);
                }
                // Parse section header: [submodule "name"] or [submodule]
                let section = line.trim_start_matches('[').trim_end_matches(']');
                let section = section.trim();
                let name = if section.eq_ignore_ascii_case("submodule") {
                    // [submodule] without a name — skip this section.
                    None
                } else if section
                    .to_lowercase()
                    .starts_with("submodule ")
                {
                    // [submodule "name"] — extract the quoted name.
                    let rest = section["submodule".len()..].trim();
                    extract_quoted_value(rest)
                } else {
                    // Not a submodule section — skip its entries.
                    None
                };
                if let Some(name) = name {
                    current_entry = Some(SubmoduleEntry {
                        name,
                        path: RepoPathBuf::root(),
                        url: String::new(),
                        branch: None,
                        update: None,
                    });
                }
            } else if current_entry.is_some() {
                // Parse key = value.
                let entry = current_entry.as_mut().unwrap();
                if let Some(eq_pos) = line.find('=') {
                    let key = line[..eq_pos].trim().to_lowercase();
                    let value = line[eq_pos + 1..].trim();
                    let value = unquote_value(value);
                    match key.as_str() {
                        "path" => {
                            entry.path =
                                RepoPathBuf::from_internal_string(&value).map_err(|err| {
                                    GitModulesError {
                                        message: format!(
                                            "line {}: invalid path: {err}",
                                            line_num + 1
                                        ),
                                    }
                                })?;
                        }
                        "url" => entry.url = value,
                        "branch" => entry.branch = Some(value),
                        "update" => entry.update = Some(value),
                        _ => {}
                    }
                }
            }
        }
        // Finish the last entry.
        if let Some(entry) = current_entry.take() {
            modules.add(entry);
        }
        Ok(modules)
    }

    /// Adds a submodule entry to both lookup maps.
    fn add(&mut self, entry: SubmoduleEntry) {
        self.by_name.insert(entry.name.clone(), entry.clone());
        self.by_path.insert(entry.path.clone(), entry);
    }

    /// Looks up a submodule by its checkout path.
    pub fn entry_by_path(&self, path: &RepoPath) -> Option<&SubmoduleEntry> {
        self.by_path.get(path)
    }

    /// Looks up a submodule by its logical name.
    pub fn entry_by_name(&self, name: &str) -> Option<&SubmoduleEntry> {
        self.by_name.get(name)
    }

    /// Returns an iterator over all submodule entries.
    pub fn entries(&self) -> impl Iterator<Item = &SubmoduleEntry> {
        self.by_path.values()
    }

    /// Returns true if there are no submodules.
    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
}

/// Extracts a quoted value from a string like `"name"`.
fn extract_quoted_value(s: &str) -> Option<String> {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        Some(s[1..s.len() - 1].to_string())
    } else {
        None
    }
}

/// Removes surrounding quotes and unescapes backslash sequences in a config
/// value.
fn unquote_value(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        let mut result = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('"') => result.push('"'),
                    Some('\\') => result.push('\\'),
                    Some('n') => result.push('\n'),
                    Some('t') => result.push('\t'),
                    Some(other) => {
                        result.push('\\');
                        result.push(other);
                    }
                    None => result.push('\\'),
                }
            } else {
                result.push(c);
            }
        }
        result
    } else {
        s.to_string()
    }
}

/// Errors from `.gitmodules` parsing.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct GitModulesError {
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    fn repo_path(path: &str) -> &RepoPath {
        RepoPath::from_internal_string(path).unwrap()
    }

    fn parse(input: &str) -> GitModules {
        GitModules::parse(input.as_bytes()).unwrap()
    }

    #[test]
    fn test_empty_file() {
        let modules = parse("");
        assert!(modules.is_empty());
    }

    #[test]
    fn test_single_submodule() {
        let modules = parse(indoc! {r#"
            [submodule "vendor"]
                path = vendor
                url = https://example.com/vendor.git
                branch = main
        "#});
        assert_eq!(modules.entries().count(), 1);
        let entry = modules.entry_by_name("vendor").unwrap();
        assert_eq!(entry.name, "vendor");
        assert_eq!(entry.path.as_ref(), repo_path("vendor"));
        assert_eq!(entry.url, "https://example.com/vendor.git");
        assert_eq!(entry.branch.as_deref(), Some("main"));
        assert_eq!(entry.update, None);
    }

    #[test]
    fn test_multiple_submodules() {
        let modules = parse(indoc! {r#"
            [submodule "vendor"]
                path = vendor
                url = https://example.com/vendor.git
            [submodule "deps/lib")
                path = deps/lib
                url = https://example.com/lib.git
        "#});
        // The second section has a typo in the bracket (`)` instead of `]`),
        // so it won't parse as a valid section. Only one entry should be found.
        assert_eq!(modules.entries().count(), 1);
    }

    #[test]
    fn test_multiple_submodules_valid() {
        let modules = parse(indoc! {r#"
            [submodule "vendor"]
                path = vendor
                url = https://example.com/vendor.git
            [submodule "lib"]
                path = deps/lib
                url = https://example.com/lib.git
        "#});
        assert_eq!(modules.entries().count(), 2);
        assert!(modules.entry_by_name("vendor").is_some());
        assert!(modules.entry_by_name("lib").is_some());
        assert!(modules.entry_by_path(repo_path("vendor")).is_some());
        assert!(modules.entry_by_path(repo_path("deps/lib")).is_some());
    }

    #[test]
    fn test_lookup_by_path() {
        let modules = parse(indoc! {r#"
            [submodule "myname"]
                path = some/deep/path
                url = https://example.com/repo.git
        "#});
        let entry = modules.entry_by_path(repo_path("some/deep/path")).unwrap();
        assert_eq!(entry.name, "myname");
        assert_eq!(entry.url, "https://example.com/repo.git");
    }

    #[test]
    fn test_update_strategy() {
        let modules = parse(indoc! {r#"
            [submodule "vendor"]
                path = vendor
                url = https://example.com/vendor.git
                update = rebase
        "#});
        let entry = modules.entry_by_name("vendor").unwrap();
        assert_eq!(entry.update.as_deref(), Some("rebase"));
    }

    #[test]
    fn test_comments_and_blank_lines() {
        let modules = parse(indoc! {r#"
            # This is a comment
            ; Another comment style

            [submodule "vendor"]
                # path comment
                path = vendor
                url = https://example.com/vendor.git
        "#});
        assert_eq!(modules.entries().count(), 1);
    }

    #[test]
    fn test_case_insensitive_section() {
        let modules = parse(indoc! {r#"
            [SUBMODULE "vendor"]
                path = vendor
                url = https://example.com/vendor.git
        "#});
        assert_eq!(modules.entries().count(), 1);
    }

    #[test]
    fn test_case_insensitive_keys() {
        let modules = parse(indoc! {r#"
            [submodule "vendor"]
                PATH = vendor
                URL = https://example.com/vendor.git
        "#});
        let entry = modules.entry_by_name("vendor").unwrap();
        assert_eq!(entry.path.as_ref(), repo_path("vendor"));
    }

    #[test]
    fn test_quoted_values() {
        let modules = parse(indoc! {r#"
            [submodule "vendor"]
                path = vendor
                url = "https://example.com/path with spaces/repo.git"
        "#});
        let entry = modules.entry_by_name("vendor").unwrap();
        assert_eq!(entry.url, "https://example.com/path with spaces/repo.git");
    }

    #[test]
    fn test_no_branch() {
        let modules = parse(indoc! {r#"
            [submodule "vendor"]
                path = vendor
                url = https://example.com/vendor.git
        "#});
        let entry = modules.entry_by_name("vendor").unwrap();
        assert_eq!(entry.branch, None);
    }

    #[test]
    fn test_invalid_utf8() {
        let invalid = b"\xff\xfe[submodule \"test\"]\npath = test\n";
        let result = GitModules::parse(invalid);
        assert!(result.is_err());
    }

    #[test]
    fn test_unquote_value_escapes() {
        // Basic escapes
        assert_eq!(unquote_value(r#""hello\nworld""#), "hello\nworld");
        assert_eq!(unquote_value(r#""tab\there""#), "tab\there");
        assert_eq!(unquote_value(r#""quote\"here""#), "quote\"here");
        assert_eq!(unquote_value(r#""back\\slash""#), "back\\slash");

        // Double-processing regression: \\n should be backslash + n, not newline
        assert_eq!(unquote_value(r#""test\\n""#), "test\\n");

        // Unknown escape: \x should remain as \x
        assert_eq!(unquote_value(r#""\x""#), "\\x");

        // Unquoted values are returned as-is
        assert_eq!(unquote_value("plain"), "plain");
        assert_eq!(unquote_value("  trimmed  "), "trimmed");

        // Empty quoted string
        assert_eq!(unquote_value(r#""""#), "");

        // Trailing backslash
        assert_eq!(unquote_value(r#""test\""#), "test\\");
    }
}
