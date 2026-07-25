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

//! Integration tests for Git filter driver (clean/smudge) support.
//!
//! These tests verify that the filter pipeline works end-to-end:
//! - During snapshot, the clean filter transforms file content before storing.
//! - During checkout, the smudge filter transforms stored content before
//!   writing to disk.

use std::fs;
use std::io::Write as _;
use std::sync::Arc;

use jj_lib::backend::TreeValue;
use jj_lib::git::get_git_repo;
use jj_lib::config::ConfigLayer;
use jj_lib::config::ConfigSource;
use jj_lib::default_backend_factories::default_backend_factories;
use jj_lib::default_backend_factories::default_working_copy_factories;
use jj_lib::repo::ReadonlyRepo;
use jj_lib::repo::Repo as _;
use jj_lib::repo_path::RepoPath;
use jj_lib::settings::UserSettings;
use jj_lib::workspace::Workspace;
use pollster::FutureExt as _;
use testutils::TestRepoBackend;
use testutils::TestResult;
use testutils::TestWorkspace;
use testutils::assert_tree_eq;
use testutils::base_user_config;
use testutils::commit_with_tree;
use testutils::empty_snapshot_options;
use testutils::repo_path;

/// Appends a `[filter "<name>"]` section to the internal Git repo's config
/// file.
fn write_filter_config(
    store: &jj_lib::store::Store,
    filter_name: &str,
    clean: Option<&str>,
    smudge: Option<&str>,
) {
    write_filter_config_full(store, filter_name, clean, smudge, false);
}

/// Like `write_filter_config` but also sets the `required` flag.
fn write_filter_config_full(
    store: &jj_lib::store::Store,
    filter_name: &str,
    clean: Option<&str>,
    smudge: Option<&str>,
    required: bool,
) {
    let git_repo = get_git_repo(store).expect("failed to open Git repo");
    let config_path = git_repo.path().join("config");
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&config_path)
        .unwrap_or_else(|e| {
            panic!(
                "failed to open Git config at {}: {e}",
                config_path.display()
            )
        });
    writeln!(file, "[filter \"{filter_name}\"]").unwrap();
    if let Some(clean) = clean {
        writeln!(file, "\tclean = {clean}").unwrap();
    }
    if let Some(smudge) = smudge {
        writeln!(file, "\tsmudge = {smudge}").unwrap();
    }
    if required {
        writeln!(file, "\trequired = true").unwrap();
    }
}

/// Holds the test workspace alive (which owns the temp dir) alongside the
/// reloaded workspace and repo.
struct FilterTest {
    // Keep the original TestWorkspace alive so the temp dir is not deleted.
    _initial: TestWorkspace,
    workspace: Workspace,
    repo: Arc<ReadonlyRepo>,
    workspace_root: std::path::PathBuf,
}

impl FilterTest {
    /// Sets up a test workspace with a filter driver configured and files
    /// written to the working copy. The workspace is reloaded after writing
    /// the Git config so that the new `GitBackend` picks up the filter
    /// driver configuration.
    fn new(
        filter_name: &str,
        clean: Option<&str>,
        smudge: Option<&str>,
        gitattributes_content: &[u8],
        files: &[(&RepoPath, &[u8])],
    ) -> Self {
        Self::new_with_settings(
            &testutils::user_settings(),
            filter_name,
            clean,
            smudge,
            gitattributes_content,
            files,
        )
    }

    /// Like `new()` but with custom user settings (e.g. for EOL conversion).
    fn new_with_settings(
        settings: &UserSettings,
        filter_name: &str,
        clean: Option<&str>,
        smudge: Option<&str>,
        gitattributes_content: &[u8],
        files: &[(&RepoPath, &[u8])],
    ) -> Self {
        Self::new_internal(settings, filter_name, clean, smudge, false, gitattributes_content, files)
    }

    /// Like `new()` but with the `required` flag set on the filter.
    fn new_required(
        filter_name: &str,
        clean: Option<&str>,
        smudge: Option<&str>,
        gitattributes_content: &[u8],
        files: &[(&RepoPath, &[u8])],
    ) -> Self {
        Self::new_internal(
            &testutils::user_settings(),
            filter_name,
            clean,
            smudge,
            true,
            gitattributes_content,
            files,
        )
    }

    fn new_internal(
        settings: &UserSettings,
        filter_name: &str,
        clean: Option<&str>,
        smudge: Option<&str>,
        required: bool,
        gitattributes_content: &[u8],
        files: &[(&RepoPath, &[u8])],
    ) -> Self {
        let initial =
            TestWorkspace::init_with_backend_and_settings(TestRepoBackend::Git, settings);
        let workspace_root = initial.workspace.workspace_root().to_path_buf();

        // Write the filter driver config to the internal Git repo.
        write_filter_config_full(initial.repo.store(), filter_name, clean, smudge, required);

        // Write .gitattributes and test files to the working copy.
        testutils::write_working_copy_file(
            &workspace_root,
            repo_path(".gitattributes"),
            gitattributes_content,
        );
        for (path, content) in files {
            testutils::write_working_copy_file(&workspace_root, path, content);
        }

        // Reload the workspace so the GitBackend re-reads the config file.
        let workspace = Workspace::load(
            settings,
            &workspace_root,
            &default_backend_factories(),
            &default_working_copy_factories(),
        )
        .expect("Failed to reload the workspace");
        let repo = workspace
            .repo_loader()
            .load_at_head()
            .block_on()
            .expect("Failed to load repo");

        Self {
            _initial: initial,
            workspace,
            repo,
            workspace_root,
        }
    }

    /// Snapshots the working copy and returns the tree.
    fn snapshot(&mut self) -> jj_lib::merged_tree::MergedTree {
        let mut locked_ws = self
            .workspace
            .start_working_copy_mutation()
            .block_on()
            .unwrap();
        let (tree, _stats) = locked_ws
            .locked_wc()
            .snapshot(&empty_snapshot_options())
            .block_on()
            .unwrap();
        locked_ws
            .finish(self.repo.op_id().clone())
            .block_on()
            .unwrap();
        tree
    }
}

/// Extracts the stored file content for a path from a `MergedTree`.
fn stored_file_content(tree: &jj_lib::merged_tree::MergedTree, path: &RepoPath) -> Vec<u8> {
    let value = tree.path_value(path).block_on().unwrap();
    let file_id = value
        .iter()
        .flatten()
        .find_map(|v| match v {
            TreeValue::File { id, .. } => Some(id.clone()),
            _ => None,
        })
        .expect("expected a file value");
    let store = tree.store();
    let mut reader = store.read_file(path, &file_id).block_on().unwrap();
    let mut content = Vec::new();
    futures::AsyncReadExt::read_to_end(&mut reader, &mut content)
        .block_on()
        .unwrap();
    content
}

/// Test that a clean/smudge filter round-trips correctly:
/// 1. Write a file with real content to the working copy.
/// 2. Snapshot — the clean filter should transform the content.
/// 3. Verify the stored content is the cleaned (pointer) version.
/// 4. Remove the file and snapshot.
/// 5. Checkout the original commit — the smudge filter should restore content.
/// 6. Verify the disk content matches the original.
#[test]
fn test_filter_clean_smudge_roundtrip() -> TestResult {
    let file_repo_path = repo_path("data.bin");
    let original_content = b"hello world from jj";
    let mut test = FilterTest::new(
        "testfilter",
        Some("tr o 0"),
        Some("tr 0 o"),
        b"*.bin filter=testfilter\n",
        &[(file_repo_path, original_content)],
    );

    let file_disk_path = file_repo_path.to_fs_path(&test.workspace_root).unwrap();

    // Snapshot — clean filter should run, replacing 'o' with '0'.
    let tree = test.snapshot();
    let stored = stored_file_content(&tree, file_repo_path);
    assert_eq!(
        stored, b"hell0 w0rld fr0m jj",
        "clean filter should have replaced 'o' with '0'"
    );

    // Create a commit with the cleaned tree.
    let file_added_commit = commit_with_tree(test.repo.store(), tree);

    // Remove the file and snapshot.
    fs::remove_file(&file_disk_path)?;
    let tree = test.snapshot();
    let file_removed_commit = commit_with_tree(test.repo.store(), tree);

    // Checkout the commit without the file.
    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_removed_commit)
        .block_on()?;
    assert!(!file_disk_path.exists());

    // Checkout the commit with the file — smudge filter should run.
    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_added_commit)
        .block_on()?;
    assert!(file_disk_path.exists(), "file should exist after checkout");

    let disk_content = fs::read(&file_disk_path)?;
    assert_eq!(
        disk_content, original_content,
        "smudge filter should have restored original content"
    );

    // Snapshot should be clean (no changes).
    let new_tree = test.snapshot();
    assert_tree_eq!(
        new_tree,
        file_added_commit.tree(),
        "working copy should be clean after smudge checkout"
    );

    Ok(())
}

/// Test that files with a `filter` attribute but no configured filter driver
/// are stored as-is (no clean filter applied).
#[test]
fn test_filter_no_driver_configured() -> TestResult {
    let file_repo_path = repo_path("data.bin");
    let original_content = b"hello world";
    let mut test = FilterTest::new(
        "lfs",
        None,
        None,
        b"*.bin filter=lfs\n",
        &[(file_repo_path, original_content)],
    );

    let tree = test.snapshot();
    let stored = stored_file_content(&tree, file_repo_path);
    assert_eq!(
        stored, original_content,
        "no filter driver configured, content should be stored as-is"
    );

    Ok(())
}

/// Test that a clean-only filter (no smudge) works: snapshot transforms
/// content, but checkout writes the stored (cleaned) content directly.
#[test]
fn test_filter_clean_only() -> TestResult {
    let file_repo_path = repo_path("data.bin");
    let original_content = b"hello world";
    let mut test = FilterTest::new(
        "cleanonly",
        Some("tr o 0"),
        None,
        b"*.bin filter=cleanonly\n",
        &[(file_repo_path, original_content)],
    );

    let file_disk_path = file_repo_path.to_fs_path(&test.workspace_root).unwrap();

    // Snapshot — clean filter runs.
    let tree = test.snapshot();
    let stored = stored_file_content(&tree, file_repo_path);
    assert_eq!(stored, b"hell0 w0rld");

    let file_added_commit = commit_with_tree(test.repo.store(), tree);

    // Remove file and checkout.
    fs::remove_file(&file_disk_path)?;
    let tree = test.snapshot();
    let file_removed_commit = commit_with_tree(test.repo.store(), tree);
    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_removed_commit)
        .block_on()?;

    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_added_commit)
        .block_on()?;

    // No smudge filter, so disk content should be the cleaned content.
    let disk_content = fs::read(&file_disk_path)?;
    assert_eq!(
        disk_content, b"hell0 w0rld",
        "no smudge filter, disk should have cleaned content"
    );

    Ok(())
}

/// Test that a smudge-only filter (no clean) works: snapshot stores content
/// as-is, but checkout applies the smudge transformation.
#[test]
fn test_filter_smudge_only() -> TestResult {
    let file_repo_path = repo_path("data.bin");
    // Content already in "pointer" form (with 0 instead of o).
    let pointer_content = b"hell0 w0rld";
    let mut test = FilterTest::new(
        "smudgeonly",
        None,
        Some("tr 0 o"),
        b"*.bin filter=smudgeonly\n",
        &[(file_repo_path, pointer_content)],
    );

    let file_disk_path = file_repo_path.to_fs_path(&test.workspace_root).unwrap();

    // Snapshot — no clean filter, content stored as-is.
    let tree = test.snapshot();
    let stored = stored_file_content(&tree, file_repo_path);
    assert_eq!(
        stored, pointer_content,
        "no clean filter, content stored as-is"
    );

    let file_added_commit = commit_with_tree(test.repo.store(), tree);

    // Remove file and checkout.
    fs::remove_file(&file_disk_path)?;
    let tree = test.snapshot();
    let file_removed_commit = commit_with_tree(test.repo.store(), tree);
    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_removed_commit)
        .block_on()?;

    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_added_commit)
        .block_on()?;

    // Smudge filter ran, replacing '0' with 'o'.
    let disk_content = fs::read(&file_disk_path)?;
    assert_eq!(
        disk_content, b"hello world",
        "smudge filter should have replaced '0' with 'o'"
    );

    Ok(())
}

/// Test that files without a matching `filter` attribute are not affected
/// by filter drivers, even if the driver is configured.
#[test]
fn test_filter_non_matching_file() -> TestResult {
    let txt_path = repo_path("data.txt");
    let txt_content = b"hello world";
    let mut test = FilterTest::new(
        "testfilter",
        Some("tr o 0"),
        Some("tr 0 o"),
        b"*.bin filter=testfilter\n",
        &[(txt_path, txt_content)],
    );

    let tree = test.snapshot();
    let stored = stored_file_content(&tree, txt_path);
    assert_eq!(
        stored, txt_content,
        "non-matching file should be stored as-is"
    );

    Ok(())
}

/// Test that a filter command using `echo` works (ignores stdin, writes
/// fixed content to stdout).
#[test]
fn test_filter_echo_command() -> TestResult {
    let file_repo_path = repo_path("data.bin");
    let mut test = FilterTest::new(
        "echofilter",
        Some("echo cleaned"),
        Some("echo smudged"),
        b"*.bin filter=echofilter\n",
        &[(file_repo_path, b"original content")],
    );

    let file_disk_path = file_repo_path.to_fs_path(&test.workspace_root).unwrap();

    // Snapshot — clean filter runs `echo cleaned`, storing "cleaned\n".
    let tree = test.snapshot();
    let stored = stored_file_content(&tree, file_repo_path);
    assert_eq!(stored, b"cleaned\n");

    let file_added_commit = commit_with_tree(test.repo.store(), tree);

    // Remove file and checkout.
    fs::remove_file(&file_disk_path)?;
    let tree = test.snapshot();
    let file_removed_commit = commit_with_tree(test.repo.store(), tree);
    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_removed_commit)
        .block_on()?;

    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_added_commit)
        .block_on()?;

    let disk_content = fs::read(&file_disk_path)?;
    assert_eq!(
        disk_content, b"smudged\n",
        "smudge filter should have run `echo smudged`"
    );

    Ok(())
}

/// Test that a failing clean filter (non-zero exit code) propagates the error
/// during snapshot.
#[test]
fn test_filter_clean_command_failure() {
    let file_repo_path = repo_path("data.bin");
    let mut test = FilterTest::new(
        "failingfilter",
        Some("false"), // always exits with non-zero
        Some("cat"),
        b"*.bin filter=failingfilter\n",
        &[(file_repo_path, b"hello world")],
    );

    // Snapshot should fail because the clean filter command fails.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        test.snapshot();
    }));
    assert!(
        result.is_err(),
        "snapshot should fail when clean filter command fails"
    );
}

/// Test that a failing smudge filter (non-zero exit code) propagates the error
/// during checkout when the filter is required.
#[test]
fn test_filter_smudge_command_failure() {
    let file_repo_path = repo_path("data.bin");
    let mut test = FilterTest::new_required(
        "failingfilter",
        Some("cat"),   // clean is a no-op (stores as-is)
        Some("false"), // smudge always fails
        b"*.bin filter=failingfilter\n",
        &[(file_repo_path, b"hello world")],
    );

    let workspace_root = test.workspace_root.clone();
    let file_disk_path = file_repo_path.to_fs_path(&workspace_root).unwrap();

    // Snapshot succeeds (clean filter is `cat`, a no-op).
    let tree = test.snapshot();
    let file_added_commit = commit_with_tree(test.repo.store(), tree);

    // Remove file and snapshot.
    fs::remove_file(&file_disk_path).unwrap();
    let tree = test.snapshot();
    let file_removed_commit = commit_with_tree(test.repo.store(), tree);
    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_removed_commit)
        .block_on()
        .unwrap();

    // Checkout should fail because the smudge filter command fails.
    let result = test
        .workspace
        .check_out(test.repo.op_id().clone(), None, &file_added_commit)
        .block_on();
    assert!(
        result.is_err(),
        "checkout should fail when required smudge filter command fails"
    );
}

/// Test that a failing smudge filter (non-zero exit code) falls back to
/// writing the raw stored content when the filter is NOT required (the
/// default). This matches Git's behavior for non-required filters.
#[test]
fn test_filter_smudge_failure_non_required_fallback() {
    let file_repo_path = repo_path("data.bin");
    let mut test = FilterTest::new(
        "failingfilter",
        Some("cat"),   // clean is a no-op (stores as-is)
        Some("false"), // smudge always fails
        b"*.bin filter=failingfilter\n",
        &[(file_repo_path, b"hello world")],
    );

    let workspace_root = test.workspace_root.clone();
    let file_disk_path = file_repo_path.to_fs_path(&workspace_root).unwrap();

    // Snapshot succeeds (clean filter is `cat`, a no-op).
    let tree = test.snapshot();
    let file_added_commit = commit_with_tree(test.repo.store(), tree);

    // Remove file and snapshot.
    fs::remove_file(&file_disk_path).unwrap();
    let tree = test.snapshot();
    let file_removed_commit = commit_with_tree(test.repo.store(), tree);
    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_removed_commit)
        .block_on()
        .unwrap();

    // Checkout should succeed (non-required filter falls back to raw content).
    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_added_commit)
        .block_on()
        .expect("checkout should succeed with raw content when non-required smudge fails");

    // The file should contain the raw stored content (same as what clean
    // produced), not the smudge output.
    let on_disk = fs::read(&file_disk_path).unwrap();
    assert_eq!(
        on_disk, b"hello world",
        "raw content should be written when smudge filter fails on non-required filter"
    );
}

/// Test that a required filter with a non-existent process command (no
/// per-file fallback) causes checkout to fail. This verifies that the
/// `required` flag is respected even in process mode.
#[test]
fn test_filter_required_process_mode_no_fallback() {
    let file_repo_path = repo_path("data.bin");
    let mut test = FilterTest::new_required(
        "brokenprocess",
        None,
        None,
        b"*.bin filter=brokenprocess\n",
        &[(file_repo_path, b"hello world")],
    );

    // Override the config to set a process command that doesn't exist.
    // Since clean/smudge are None, there's no per-file fallback — the
    // process failure should propagate as a hard error when required=true.
    let git_repo = get_git_repo(test.repo.store()).expect("failed to open Git repo");
    let config_path = git_repo.path().join("config");
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&config_path)
        .unwrap();
    writeln!(file, "\tprocess = nonexistent_filter_process_12345").unwrap();

    // Reload to pick up the new config.
    let workspace = Workspace::load(
        &testutils::user_settings(),
        &test.workspace_root,
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .expect("Failed to reload the workspace");
    let repo = workspace
        .repo_loader()
        .load_at_head()
        .block_on()
        .expect("Failed to load repo");
    test.workspace = workspace;
    test.repo = repo;

    // Snapshot should fail because clean filter (process mode) can't start
    // and there's no per-file fallback.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        test.snapshot();
    }));
    assert!(
        result.is_err(),
        "snapshot should fail when required process filter can't start and no per-file fallback exists"
    );
}

/// Test that a clean filter command that doesn't exist (spawn failure)
/// propagates the error during snapshot.
#[test]
fn test_filter_clean_command_not_found() {
    let file_repo_path = repo_path("data.bin");
    let mut test = FilterTest::new(
        "missingcmd",
        Some("nonexistent_binary_xyz123"),
        None,
        b"*.bin filter=missingcmd\n",
        &[(file_repo_path, b"hello world")],
    );

    // Snapshot should fail because the clean command doesn't exist.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        test.snapshot();
    }));
    assert!(
        result.is_err(),
        "snapshot should fail when clean filter command is not found"
    );
}

/// Test that a clean/smudge filter works with a `.gitattributes` file in a
/// subdirectory (not just at the repo root).
#[test]
fn test_filter_nested_gitattributes() -> TestResult {
    let settings = testutils::user_settings();
    let initial = TestWorkspace::init_with_backend_and_settings(TestRepoBackend::Git, &settings);
    let workspace_root = initial.workspace.workspace_root().to_path_buf();

    // Configure the filter driver in the internal Git repo.
    write_filter_config(
        initial.repo.store(),
        "testfilter",
        Some("tr o 0"),
        Some("tr 0 o"),
    );

    // Write a root .gitattributes that does NOT match .bin files.
    testutils::write_working_copy_file(
        &workspace_root,
        repo_path(".gitattributes"),
        b"*.txt text\n",
    );

    // Write a subdirectory .gitattributes that matches *.bin.
    testutils::write_working_copy_file(
        &workspace_root,
        repo_path("subdir/.gitattributes"),
        b"*.bin filter=testfilter\n",
    );

    // Write files in both root and subdir.
    let root_file_path = repo_path("root_file.bin");
    let subdir_file_path = repo_path("subdir/data.bin");
    testutils::write_working_copy_file(&workspace_root, root_file_path, b"hello world");
    testutils::write_working_copy_file(&workspace_root, subdir_file_path, b"hello world");

    // Reload the workspace so the GitBackend re-reads the config file.
    let mut workspace = Workspace::load(
        &settings,
        &workspace_root,
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .expect("Failed to reload the workspace");
    let repo = workspace
        .repo_loader()
        .load_at_head()
        .block_on()
        .expect("Failed to load repo");

    // Keep the initial TestWorkspace alive.
    let _initial = initial;

    // Snapshot.
    let mut locked_ws = workspace.start_working_copy_mutation().block_on().unwrap();
    let (tree, _stats) = locked_ws
        .locked_wc()
        .snapshot(&empty_snapshot_options())
        .block_on()
        .unwrap();
    locked_ws.finish(repo.op_id().clone()).block_on().unwrap();

    // Root .bin file should NOT be filtered (no matching .gitattributes at root).
    let root_stored = stored_file_content(&tree, root_file_path);
    assert_eq!(
        root_stored, b"hello world",
        "root .bin file should not be filtered"
    );

    // Subdir .bin file SHOULD be filtered (subdir .gitattributes matches).
    let subdir_stored = stored_file_content(&tree, subdir_file_path);
    assert_eq!(
        subdir_stored, b"hell0 w0rld",
        "subdir .bin file should be filtered by subdir .gitattributes"
    );

    Ok(())
}

/// Test that EOL conversion and clean filter work together.
///
/// Git's conversion order for snapshot is: working tree → clean filter →
/// EOL normalization → store. This test verifies that a CRLF file with a
/// clean filter gets both the clean filter AND EOL normalization applied.
#[test]
fn test_filter_and_eol_combined() -> TestResult {
    let mut config = base_user_config();
    config.add_layer(
        ConfigLayer::parse(
            ConfigSource::User,
            r#"working-copy.eol-conversion = "input-output""#,
        )
        .unwrap(),
    );
    let settings = UserSettings::from_config(config).unwrap();

    let file_repo_path = repo_path("data.bin");
    let original_content = b"hello\r\nworld\r\n"; // CRLF line endings
    let mut test = FilterTest::new_with_settings(
        &settings,
        "testfilter",
        Some("tr o 0"),       // clean: replace o with 0
        Some("tr 0 o"),       // smudge: replace 0 with o
        b"*.bin filter=testfilter\n",
        &[(file_repo_path, original_content)],
    );

    // Snapshot — clean filter runs (o→0), then EOL conversion (CRLF→LF).
    let tree = test.snapshot();
    let stored = stored_file_content(&tree, file_repo_path);
    assert_eq!(
        stored,
        b"hell0\nw0rld\n",
        "clean filter should replace o with 0, then EOL should convert CRLF to LF"
    );

    let file_disk_path = file_repo_path.to_fs_path(&test.workspace_root).unwrap();

    // Commit, remove, and checkout to test smudge + EOL.
    let file_added_commit = commit_with_tree(test.repo.store(), tree);
    fs::remove_file(&file_disk_path)?;
    let tree = test.snapshot();
    let file_removed_commit = commit_with_tree(test.repo.store(), tree);
    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_removed_commit)
        .block_on()?;
    test.workspace
        .check_out(test.repo.op_id().clone(), None, &file_added_commit)
        .block_on()?;

    // Checkout order: store → smudge (0→o) → EOL (LF→CRLF).
    let disk_content = fs::read(&file_disk_path)?;
    assert_eq!(
        disk_content,
        b"hello\r\nworld\r\n",
        "smudge filter should restore 0 to o, then EOL should convert LF to CRLF"
    );

    Ok(())
}
