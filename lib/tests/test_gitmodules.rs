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

//! Integration tests for `.gitmodules` parsing with real trees.

use jj_lib::gitmodules::GitModules;
use pollster::FutureExt as _;
use testutils::TestRepoBackend;
use testutils::TestResult;
use testutils::TestWorkspace;
use testutils::repo_path;

/// Test that `.gitmodules` can be read from a tree and parsed.
#[test]
fn test_gitmodules_from_tree() -> TestResult {
    let mut test_workspace = TestWorkspace::init_with_backend_and_settings(
        TestRepoBackend::Git,
        &testutils::user_settings(),
    );

    let workspace_root = test_workspace.workspace.workspace_root().to_path_buf();

    // Write a .gitmodules file to the working copy.
    let gitmodules_content = b"[submodule \"vendor\"]\n\
        \tpath = vendor\n\
        \turl = https://example.com/vendor.git\n\
        \tbranch = main\n\
[submodule \"deps/lib\"]\n\
        \tpath = deps/lib\n\
        \turl = https://example.com/lib.git\n";
    testutils::write_working_copy_file(
        &workspace_root,
        repo_path(".gitmodules"),
        gitmodules_content,
    );

    // Snapshot to store the file in the tree.
    let tree = test_workspace.snapshot()?;

    // Read the .gitmodules file from the tree.
    let path = repo_path(".gitmodules");
    let value = tree.path_value(path).block_on().unwrap();
    let file_id = value
        .iter()
        .flatten()
        .find_map(|v| match v {
            jj_lib::backend::TreeValue::File { id, .. } => Some(id.clone()),
            _ => None,
        })
        .expect("expected .gitmodules file in tree");
    let store = tree.store();
    let mut reader = store.read_file(path, &file_id).block_on().unwrap();
    let mut content = Vec::new();
    futures::AsyncReadExt::read_to_end(&mut reader, &mut content)
        .block_on()
        .unwrap();

    // Parse the .gitmodules content.
    let modules = GitModules::parse(&content).expect("failed to parse .gitmodules");
    assert_eq!(modules.entries().count(), 2);

    let vendor = modules.entry_by_name("vendor").unwrap();
    assert_eq!(vendor.name, "vendor");
    assert_eq!(vendor.path.as_ref(), repo_path("vendor"));
    assert_eq!(vendor.url, "https://example.com/vendor.git");
    assert_eq!(vendor.branch.as_deref(), Some("main"));

    let lib = modules.entry_by_path(repo_path("deps/lib")).unwrap();
    assert_eq!(lib.name, "deps/lib");
    assert_eq!(lib.url, "https://example.com/lib.git");
    assert_eq!(lib.branch, None);

    Ok(())
}

/// Test that a missing `.gitmodules` file is handled gracefully.
#[test]
fn test_gitmodules_missing() -> TestResult {
    let mut test_workspace = TestWorkspace::init_with_backend_and_settings(
        TestRepoBackend::Git,
        &testutils::user_settings(),
    );

    let tree = test_workspace.snapshot()?;
    let path = repo_path(".gitmodules");
    let value = tree.path_value(path).block_on().unwrap();
    assert!(
        value.is_absent(),
        ".gitmodules should not exist in empty tree"
    );

    Ok(())
}

/// Test that a `.gitmodules` file with unknown keys is parsed successfully
/// (unknown keys are silently ignored).
#[test]
fn test_gitmodules_unknown_keys() {
    let content = b"[submodule \"vendor\"]\n\
        \tpath = vendor\n\
        \turl = https://example.com/vendor.git\n\
        \tcustom_key = some_value\n\
        \tanother_key = 42\n";
    let modules = GitModules::parse(content).unwrap();
    let entry = modules.entry_by_name("vendor").unwrap();
    assert_eq!(entry.name, "vendor");
    assert_eq!(entry.url, "https://example.com/vendor.git");
}

/// Test that `.gitmodules` with non-submodule sections is parsed correctly
/// (non-submodule sections are skipped).
#[test]
fn test_gitmodules_non_submodule_sections() {
    let content = b"[core]\n\
        \trepositoryformatversion = 0\n\
[submodule \"vendor\"]\n\
        \tpath = vendor\n\
        \turl = https://example.com/vendor.git\n\
[remote \"origin\"]\n\
        \turl = https://example.com/origin.git\n\
        \tfetch = +refs/heads/*:refs/remotes/origin/*\n";
    let modules = GitModules::parse(content).unwrap();
    assert_eq!(modules.entries().count(), 1);
    assert!(modules.entry_by_name("vendor").is_some());
    assert!(modules.entry_by_name("origin").is_none());
}

/// Test that a `.gitmodules` with nested paths works correctly.
#[test]
fn test_gitmodules_nested_paths() {
    let content = b"[submodule \"deep\"]\n\
        \tpath = very/deep/nested/path\n\
        \turl = https://example.com/deep.git\n";
    let modules = GitModules::parse(content).unwrap();
    let entry = modules
        .entry_by_path(repo_path("very/deep/nested/path"))
        .unwrap();
    assert_eq!(entry.name, "deep");
    assert_eq!(entry.url, "https://example.com/deep.git");
}

/// Test that the `RepoPath` lookup uses the correct path comparison.
#[test]
fn test_gitmodules_path_lookup() {
    let content = b"[submodule \"a\"]\n\
        \tpath = path/a\n\
        \turl = https://example.com/a.git\n\
[submodule \"b\"]\n\
        \tpath = path/b\n\
        \turl = https://example.com/b.git\n";
    let modules = GitModules::parse(content).unwrap();

    // Looking up by path should distinguish between path/a and path/b.
    let a = modules.entry_by_path(repo_path("path/a")).unwrap();
    assert_eq!(a.url, "https://example.com/a.git");
    let b = modules.entry_by_path(repo_path("path/b")).unwrap();
    assert_eq!(b.url, "https://example.com/b.git");

    // Looking up a non-existent path should return None.
    assert!(modules.entry_by_path(repo_path("path/c")).is_none());
    assert!(modules.entry_by_path(repo_path("path")).is_none());
}

/// Test that `[submodule]` without a name is skipped, not treated as
/// an entry with an empty name.
#[test]
fn test_gitmodules_section_without_name() {
    let content = b"[submodule]\n\
        \tpath = orphan\n\
        \turl = https://example.com/orphan.git\n\
        [submodule \"real\"]\n\
        \tpath = real\n\
        \turl = https://example.com/real.git\n";
    let modules = GitModules::parse(content).unwrap();

    // The nameless section should NOT create an entry.
    assert_eq!(modules.entries().count(), 1);
    let entry = modules.entry_by_name("real").unwrap();
    assert_eq!(entry.url, "https://example.com/real.git");

    // The orphan path should not be registered.
    assert!(modules.entry_by_path(repo_path("orphan")).is_none());
}
