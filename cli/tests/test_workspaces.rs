// Copyright 2022 The Jujutsu Authors
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
// See the License for the specific language governing permissions and
// limitations under the License.

use test_case::test_case;
use testutils::TestResult;
use testutils::git;

use crate::common::CommandOutput;
use crate::common::TestEnvironment;
use crate::common::TestWorkDir;

#[test]
fn test_workspaces_invalid_name() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let main_dir = test_env.work_dir("repo");

    // refuse to create, directory not created
    let output = main_dir.run_jj(["workspace", "add", "--name", "", "../secondary"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: New workspace name cannot be empty
    [EOF]
    [exit status: 1]
    ");
    assert!(!test_env.env_root().join("secondary").exists());

    // refuse to rename
    let output = main_dir.run_jj(["workspace", "rename", ""]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: New workspace name cannot be empty
    [EOF]
    [exit status: 1]
    ");
}

/// Test adding a second and a third workspace
#[test]
fn test_workspaces_add_second_and_third_workspace() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    main_dir.write_file("file", "contents");
    main_dir.run_jj(["commit", "-m", "initial"]).success();

    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . rlvkpnrz 504e3d8c (empty) (no description set)
    [EOF]
    ");

    let output = main_dir.run_jj(["workspace", "add", "--name", "second", "../secondary"]);
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "../secondary"
    Working copy  (@) now at: rzvqmyuk bcc858e1 (empty) (no description set)
    Parent commit (@-)      : qpvuntsm 7b22a8cb initial
    Added 1 files, modified 0 files, removed 0 files
    [EOF]
    "#);

    // Can see the working-copy commit in each workspace in the log output. The "@"
    // node in the graph indicates the current workspace's working-copy commit.
    insta::assert_snapshot!(get_log_output(&main_dir), @r#"
    @  504e3d8c1bcd default@
    │ ○  bcc858e1d93f second@
    ├─╯
    ○  7b22a8cbe888 "initial"
    ◆  000000000000
    [EOF]
    "#);
    insta::assert_snapshot!(get_log_output(&secondary_dir), @r#"
    @  bcc858e1d93f second@
    │ ○  504e3d8c1bcd default@
    ├─╯
    ○  7b22a8cbe888 "initial"
    ◆  000000000000
    [EOF]
    "#);

    // Both workspaces show up when we list them
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . rlvkpnrz 504e3d8c (empty) (no description set)
    second: ../secondary rzvqmyuk bcc858e1 (empty) (no description set)
    [EOF]
    ");

    // Check that a workspace can be created in an existing empty directory
    main_dir.create_dir("../third");
    let output = main_dir.run_jj(["workspace", "add", "--name", "third", "../third"]);
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "../third"
    Working copy  (@) now at: nuwvvtmy d55e769c (empty) (no description set)
    Parent commit (@-)      : qpvuntsm 7b22a8cb initial
    Added 1 files, modified 0 files, removed 0 files
    [EOF]
    "#);

    // Duplicate names are not allowed, directory not created
    let output = main_dir.run_jj(["workspace", "add", "--name", "third", "../tertiary"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    ------- stderr -------
    Error: Workspace named 'third' already exists
    [EOF]
    [exit status: 1]
    ");
    assert!(!test_env.env_root().join("tertiary").exists());
}

#[test]
fn test_workspaces_add_with_message() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file", "contents");
    main_dir.run_jj(["commit", "-m", "initial"]).success();

    let output = main_dir.run_jj([
        "workspace",
        "add",
        "--name",
        "second",
        "-m",
        "add second workspace",
        "../secondary",
    ]);

    // Check that the newly created workspace has a description for the commit
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "../secondary"
    Working copy  (@) now at: pmmvwywv 47a2d9a1 (empty) add second workspace
    Parent commit (@-)      : qpvuntsm 7b22a8cb initial
    Added 1 files, modified 0 files, removed 0 files
    [EOF]
    "#);

    main_dir
        .run_jj([
            "workspace",
            "add",
            "--name",
            "third",
            "-m",
            "first message",
            "-m",
            "second message",
            "../tertiary",
        ])
        .success();

    // Check that multiple messages work as expected like with the "new" command
    let output = main_dir.run_jj(["log", "-r", "third@", "-Tdescription", "--no-graph"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    first message

    second message
    [EOF]
    ");

    // Test that adding workspace with no message has no trailers
    test_env.add_config(
        r#"[templates]
        commit_trailers = '"Signed-off-by: " ++ committer.email()'
        "#,
    );

    let output = main_dir.run_jj(["workspace", "add", "--name", "fourth", "../quaternary"]);

    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "../quaternary"
    Working copy  (@) now at: nppvrztz 0bfa7004 (empty) (no description set)
    Parent commit (@-)      : qpvuntsm 7b22a8cb initial
    Added 1 files, modified 0 files, removed 0 files
    [EOF]
    "#);
}

/// Test how sparse patterns are inherited
#[test]
fn test_workspaces_sparse_patterns() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "ws1"]).success();
    let ws1_dir = test_env.work_dir("ws1");
    let ws2_dir = test_env.work_dir("ws2");
    let ws3_dir = test_env.work_dir("ws3");
    let ws4_dir = test_env.work_dir("ws4");
    let ws5_dir = test_env.work_dir("ws5");
    let ws6_dir = test_env.work_dir("ws6");

    ws1_dir
        .run_jj(["sparse", "set", "--clear", "--add=foo"])
        .success();
    ws1_dir.run_jj(["workspace", "add", "../ws2"]).success();
    let output = ws2_dir.run_jj(["sparse", "list"]);
    insta::assert_snapshot!(output, @"
    foo
    [EOF]
    ");
    ws2_dir.run_jj(["sparse", "set", "--add=bar"]).success();
    ws2_dir.run_jj(["workspace", "add", "../ws3"]).success();
    let output = ws3_dir.run_jj(["sparse", "list"]);
    insta::assert_snapshot!(output, @"
    bar
    foo
    [EOF]
    ");
    // --sparse-patterns behavior
    ws3_dir
        .run_jj(["workspace", "add", "--sparse-patterns=copy", "../ws4"])
        .success();
    let output = ws4_dir.run_jj(["sparse", "list"]);
    insta::assert_snapshot!(output, @"
    bar
    foo
    [EOF]
    ");
    ws3_dir
        .run_jj(["workspace", "add", "--sparse-patterns=full", "../ws5"])
        .success();
    let output = ws5_dir.run_jj(["sparse", "list"]);
    insta::assert_snapshot!(output, @"
    .
    [EOF]
    ");
    ws3_dir
        .run_jj(["workspace", "add", "--sparse-patterns=empty", "../ws6"])
        .success();
    let output = ws6_dir.run_jj(["sparse", "list"]);
    insta::assert_snapshot!(output, @"");
}

/// Test adding a second workspace while the current workspace is editing a
/// merge
#[test]
fn test_workspaces_add_second_workspace_on_merge() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir.run_jj(["describe", "-m=left"]).success();
    main_dir.run_jj(["new", "@-", "-m=right"]).success();
    main_dir.run_jj(["new", "@-+", "-m=merge"]).success();

    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output, @"
    default: . zsuskuln 46ed31b6 (empty) merge
    [EOF]
    ");

    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../secondary"])
        .success();

    // The new workspace's working-copy commit shares all parents with the old one.
    insta::assert_snapshot!(get_log_output(&main_dir), @r#"
    @    46ed31b61ce9 default@ "merge"
    ├─╮
    │ │ ○  d23b2d4ff55c second@
    ╭─┬─╯
    │ ○  3c52528f5893 "left"
    ○ │  a3155ab1bf5a "right"
    ├─╯
    ◆  000000000000
    [EOF]
    "#);
}

/// Test that --ignore-working-copy is respected
#[test]
fn test_workspaces_add_ignore_working_copy() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    // TODO: maybe better to error out early?
    let output = main_dir.run_jj(["workspace", "add", "--ignore-working-copy", "../secondary"]);
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "../secondary"
    Error: This command must be able to update the working copy.
    Hint: Don't use --ignore-working-copy.
    [EOF]
    [exit status: 1]
    "#);
}

/// Test that --no-integrate-operation is respected
#[test]
fn test_workspaces_add_no_integrate_operation() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    // TODO: maybe better to error out early?
    let output = main_dir.run_jj([
        "workspace",
        "add",
        "--no-integrate-operation",
        "../secondary",
    ]);
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "../secondary"
    Error: This command must be able to update the working copy.
    Hint: Don't use --no-integrate-operation.
    [EOF]
    [exit status: 1]
    "#);
}

/// Test that --at-op is respected
#[test]
fn test_workspaces_add_at_operation() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file1", "");
    let output = main_dir.run_jj(["commit", "-m1"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Working copy  (@) now at: rlvkpnrz 59e07459 (empty) (no description set)
    Parent commit (@-)      : qpvuntsm 9e4b0b91 1
    [EOF]
    ");

    main_dir.write_file("file2", "");
    let output = main_dir.run_jj(["commit", "-m2"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Working copy  (@) now at: kkmpptxz 6e9610ac (empty) (no description set)
    Parent commit (@-)      : rlvkpnrz 8b7259b9 2
    [EOF]
    ");

    // --at-op should disable snapshot in the main workspace, but the newly
    // created workspace should still be writable.
    main_dir.write_file("file3", "");
    let output = main_dir.run_jj(["workspace", "add", "--at-op=@-", "../secondary"]);
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "../secondary"
    Working copy  (@) now at: rzvqmyuk b8772476 (empty) (no description set)
    Parent commit (@-)      : qpvuntsm 9e4b0b91 1
    Added 1 files, modified 0 files, removed 0 files
    [EOF]
    "#);
    let secondary_dir = test_env.work_dir("secondary");

    // New snapshot can be taken in the secondary workspace.
    secondary_dir.write_file("file4", "");
    let output = secondary_dir.run_jj(["status"]);
    insta::assert_snapshot!(output, @"
    Working copy changes:
    A file4
    Working copy  (@) : rzvqmyuk f2ff8257 (no description set)
    Parent commit (@-): qpvuntsm 9e4b0b91 1
    [EOF]
    ------- stderr -------
    Concurrent modification detected, resolving automatically.
    [EOF]
    ");

    let output = secondary_dir.run_jj(["op", "log", "-Tdescription"]);
    insta::assert_snapshot!(output, @"
    @  snapshot working copy
    ○    reconcile divergent operations
    ├─╮
    ○ │  commit 9152e822279787a168ddf4cede6440a21faa00d7
    │ ○  create initial working-copy commit in workspace secondary
    │ ○  add workspace 'secondary'
    ├─╯
    ○  snapshot working copy
    ○  commit 093c3c9624b6cfe22b310586f5638792aa80e6d7
    ○  snapshot working copy
    ○  add workspace 'default'
    ○
    [EOF]
    ");
}

/// Test adding a workspace, but at a specific revision using '-r'
#[test]
fn test_workspaces_add_workspace_at_revision() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    main_dir.write_file("file-1", "contents");
    main_dir.run_jj(["commit", "-m", "first"]).success();

    main_dir.write_file("file-2", "contents");
    main_dir.run_jj(["commit", "-m", "second"]).success();

    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output, @"
    default: . kkmpptxz 5ac9178d (empty) (no description set)
    [EOF]
    ");

    let output = main_dir.run_jj([
        "workspace",
        "add",
        "--name",
        "second",
        "../secondary",
        "-r",
        "@--",
    ]);
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "../secondary"
    Working copy  (@) now at: zxsnswpr ea5860fb (empty) (no description set)
    Parent commit (@-)      : qpvuntsm 27473635 first
    Added 1 files, modified 0 files, removed 0 files
    [EOF]
    "#);

    // Can see the working-copy commit in each workspace in the log output. The "@"
    // node in the graph indicates the current workspace's working-copy commit.
    insta::assert_snapshot!(get_log_output(&main_dir), @r#"
    @  5ac9178da8b2 default@
    ○  a47d8a593529 "second"
    │ ○  ea5860fbd622 second@
    ├─╯
    ○  27473635a942 "first"
    ◆  000000000000
    [EOF]
    "#);
    insta::assert_snapshot!(get_log_output(&secondary_dir), @r#"
    @  ea5860fbd622 second@
    │ ○  5ac9178da8b2 default@
    │ ○  a47d8a593529 "second"
    ├─╯
    ○  27473635a942 "first"
    ◆  000000000000
    [EOF]
    "#);
}

/// Test multiple `-r` flags to `workspace add` to create a workspace
/// working-copy commit with multiple parents.
#[test]
fn test_workspaces_add_workspace_multiple_revisions() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file-1", "contents");
    main_dir.run_jj(["commit", "-m", "first"]).success();
    main_dir.run_jj(["new", "-r", "root()"]).success();

    main_dir.write_file("file-2", "contents");
    main_dir.run_jj(["commit", "-m", "second"]).success();
    main_dir.run_jj(["new", "-r", "root()"]).success();

    main_dir.write_file("file-3", "contents");
    main_dir.run_jj(["commit", "-m", "third"]).success();
    main_dir.run_jj(["new", "-r", "root()"]).success();

    insta::assert_snapshot!(get_log_output(&main_dir), @r#"
    @  8d23abddc924
    │ ○  eba7f49e2358 "third"
    ├─╯
    │ ○  62444a45efcf "second"
    ├─╯
    │ ○  27473635a942 "first"
    ├─╯
    ◆  000000000000
    [EOF]
    "#);

    let output = main_dir.run_jj([
        "workspace",
        "add",
        "--name=merge",
        "../merged",
        "-r=subject(third)",
        "-r=subject(second)",
        "-r=subject(first)",
    ]);
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "../merged"
    Working copy  (@) now at: wmwvqwsz 2d7c9a2d (empty) (no description set)
    Parent commit (@-)      : mzvwutvl eba7f49e third
    Parent commit (@-)      : kkmpptxz 62444a45 second
    Parent commit (@-)      : qpvuntsm 27473635 first
    Added 3 files, modified 0 files, removed 0 files
    [EOF]
    "#);

    insta::assert_snapshot!(get_log_output(&main_dir), @r#"
    @  8d23abddc924 default@
    │ ○      2d7c9a2d41dc merge@
    │ ├─┬─╮
    │ │ │ ○  27473635a942 "first"
    ├─────╯
    │ │ ○  62444a45efcf "second"
    ├───╯
    │ ○  eba7f49e2358 "third"
    ├─╯
    ◆  000000000000
    [EOF]
    "#);
}

#[test]
fn test_workspaces_add_workspace_from_subdir() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    let subdir_dir = main_dir.create_dir("subdir");
    subdir_dir.write_file("file", "contents");
    main_dir.run_jj(["commit", "-m", "initial"]).success();

    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . rlvkpnrz 0ba0ff35 (empty) (no description set)
    [EOF]
    ");

    // Create workspace while in sub-directory of current workspace
    let output = subdir_dir.run_jj(["workspace", "add", "../../secondary"]);
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "../../secondary"
    Working copy  (@) now at: rzvqmyuk dea1be10 (empty) (no description set)
    Parent commit (@-)      : qpvuntsm 80b67806 initial
    Added 1 files, modified 0 files, removed 0 files
    [EOF]
    "#);

    // Both workspaces show up when we list them
    let output = secondary_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: ../main rlvkpnrz 0ba0ff35 (empty) (no description set)
    secondary: . rzvqmyuk dea1be10 (empty) (no description set)
    [EOF]
    ");
}

#[test]
fn test_workspaces_add_workspace_in_current_workspace() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file", "contents");
    main_dir.run_jj(["commit", "-m", "initial"]).success();

    // Try to create workspace using name instead of path
    let output = main_dir.run_jj(["workspace", "add", "secondary"]);
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "secondary"
    Warning: Workspace created inside current directory. If this was unintentional, delete the "secondary" directory and run `jj workspace forget secondary` to remove it.
    Working copy  (@) now at: pmmvwywv 058f604d (empty) (no description set)
    Parent commit (@-)      : qpvuntsm 7b22a8cb initial
    Added 1 files, modified 0 files, removed 0 files
    [EOF]
    "#);

    // Workspace created despite warning
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output, @"
    default: . rlvkpnrz 504e3d8c (empty) (no description set)
    secondary: secondary pmmvwywv 058f604d (empty) (no description set)
    [EOF]
    ");

    // Use explicit path instead (no warning)
    let output = main_dir.run_jj(["workspace", "add", "./third"]);
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "third"
    Working copy  (@) now at: zxsnswpr 1c1effec (empty) (no description set)
    Parent commit (@-)      : qpvuntsm 7b22a8cb initial
    Added 1 files, modified 0 files, removed 0 files
    [EOF]
    "#);

    // Both workspaces created
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output, @"
    default: . rlvkpnrz 504e3d8c (empty) (no description set)
    secondary: secondary pmmvwywv 058f604d (empty) (no description set)
    third: third zxsnswpr 1c1effec (empty) (no description set)
    [EOF]
    ");

    let output = main_dir.run_jj(["file", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    file
    [EOF]
    ");
}

#[test]
fn test_workspace_add_override_path_in_store() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file", "contents");
    main_dir.run_jj(["commit", "-m", "initial"]).success();

    let output = main_dir.run_jj(["workspace", "add", "--name", "second", "../secondary"]);
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "../secondary"
    Working copy  (@) now at: pmmvwywv 058f604d (empty) (no description set)
    Parent commit (@-)      : qpvuntsm 7b22a8cb initial
    Added 1 files, modified 0 files, removed 0 files
    [EOF]
    "#);

    // Both workspaces show up when we list them
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . rlvkpnrz 504e3d8c (empty) (no description set)
    second: ../secondary pmmvwywv 058f604d (empty) (no description set)
    [EOF]
    ");

    // Undoing workspace addition
    let output = main_dir.run_jj(["operation", "restore", "@--"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Restored to operation: 7d5981d12f2a (2001-02-03 08:05:08) commit 006bd1130b84e90ab082adeabd7409270d5a86da
    [EOF]
    ");

    // Only default workspace show up when we list them
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . rlvkpnrz 504e3d8c (empty) (no description set)
    [EOF]
    ");

    // Re-creating the same workspace with different path
    let output = main_dir.run_jj(["workspace", "add", "--name", "second", "../tertiary"]);
    insta::assert_snapshot!(output.normalize_backslash(), @r#"
    ------- stderr -------
    Created workspace in "../tertiary"
    Working copy  (@) now at: spxsnpux 96ef6c50 (empty) (no description set)
    Parent commit (@-)      : qpvuntsm 7b22a8cb initial
    Added 1 files, modified 0 files, removed 0 files
    [EOF]
    "#);

    // Both workspaces show up when we list them
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . rlvkpnrz 504e3d8c (empty) (no description set)
    second: ../tertiary spxsnpux 96ef6c50 (empty) (no description set)
    [EOF]
    ");

    // The 'second' workspace points to the new path
    let output = main_dir.run_jj(["workspace", "root", "--name", "second"]);
    insta::assert_snapshot!(output, @"
    $TEST_ENV/tertiary
    [EOF]
    ");
}

/// Test making changes to the working copy in a workspace as it gets rewritten
/// from another workspace
#[test]
fn test_workspaces_conflicting_edits() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    main_dir.write_file("file", "contents\n");
    main_dir.run_jj(["new"]).success();

    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();

    insta::assert_snapshot!(get_log_output(&main_dir), @"
    @  393250c59e39 default@
    │ ○  547036666102 secondary@
    ├─╯
    ○  9a462e35578a
    ◆  000000000000
    [EOF]
    ");

    // Make changes in both working copies
    main_dir.write_file("file", "changed in main\n");
    secondary_dir.write_file("file", "changed in second\n");
    // Squash the changes from the main workspace into the initial commit (before
    // running any command in the secondary workspace
    let output = main_dir.run_jj(["squash"]);
    insta::assert_snapshot!(output, @r"
    ------- stderr -------
    Rebased 1 descendant commits.
    Working copy  (@) now at: mzvwutvl 3a9b690d (empty) (no description set)
    Parent commit (@-)      : qpvuntsm b853f7c8 (no description set)
    [EOF]
    ");

    // The secondary workspace's working-copy commit was updated
    insta::assert_snapshot!(get_log_output(&main_dir), @"
    @  3a9b690d6e67 default@
    │ ○  90f3d42e0bff secondary@
    ├─╯
    ○  b853f7c8b006
    ◆  000000000000
    [EOF]
    ");
    let output = secondary_dir.run_jj(["st"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: The working copy is stale (not updated since operation 149761aea7d1).
    Hint: Run `jj workspace update-stale` to update it.
    See https://docs.jj-vcs.dev/latest/working-copy/#stale-working-copy for more information.
    [EOF]
    [exit status: 1]
    ");
    // Same error on second run, and from another command
    let output = secondary_dir.run_jj(["log"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: The working copy is stale (not updated since operation 149761aea7d1).
    Hint: Run `jj workspace update-stale` to update it.
    See https://docs.jj-vcs.dev/latest/working-copy/#stale-working-copy for more information.
    [EOF]
    [exit status: 1]
    ");
    // It was detected that the working copy is now stale.
    // Since there was an uncommitted change in the working copy, it should
    // have been committed first (causing divergence)
    let output = secondary_dir.run_jj(["workspace", "update-stale"]);
    insta::assert_snapshot!(output, @r"
    ------- stderr -------
    Concurrent modification detected, resolving automatically.
    Rebased 1 descendant commits onto commits rewritten by other operation.
    Working copy  (@) now at: pmmvwywv/2 90f3d42e (divergent) (empty) (no description set)
    Parent commit (@-)      : qpvuntsm b853f7c8 (no description set)
    Added 0 files, modified 1 files, removed 0 files
    Updated working copy to fresh commit 90f3d42e0bff
    [EOF]
    ");
    insta::assert_snapshot!(get_log_output(&secondary_dir),
    @"
    @  90f3d42e0bff secondary@ (divergent)
    │ ×  8823f4273170 (divergent)
    ├─╯
    │ ○  3a9b690d6e67 default@
    ├─╯
    ○  b853f7c8b006
    ◆  000000000000
    [EOF]
    ");
    // The stale working copy should have been resolved by the previous command
    insta::assert_snapshot!(get_log_output(&secondary_dir), @"
    @  90f3d42e0bff secondary@ (divergent)
    │ ×  8823f4273170 (divergent)
    ├─╯
    │ ○  3a9b690d6e67 default@
    ├─╯
    ○  b853f7c8b006
    ◆  000000000000
    [EOF]
    ");
}

/// Test a clean working copy that gets rewritten from another workspace
#[test]
fn test_workspaces_updated_by_other() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    main_dir.write_file("file", "contents\n");
    main_dir.run_jj(["new"]).success();

    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();

    insta::assert_snapshot!(get_log_output(&main_dir), @"
    @  393250c59e39 default@
    │ ○  547036666102 secondary@
    ├─╯
    ○  9a462e35578a
    ◆  000000000000
    [EOF]
    ");

    // Rewrite the check-out commit in one workspace.
    main_dir.write_file("file", "changed in main\n");
    let output = main_dir.run_jj(["squash"]);
    insta::assert_snapshot!(output, @r"
    ------- stderr -------
    Rebased 1 descendant commits.
    Working copy  (@) now at: mzvwutvl 3a9b690d (empty) (no description set)
    Parent commit (@-)      : qpvuntsm b853f7c8 (no description set)
    [EOF]
    ");

    // The secondary workspace's working-copy commit was updated.
    insta::assert_snapshot!(get_log_output(&main_dir), @"
    @  3a9b690d6e67 default@
    │ ○  90f3d42e0bff secondary@
    ├─╯
    ○  b853f7c8b006
    ◆  000000000000
    [EOF]
    ");
    let output = secondary_dir.run_jj(["st"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: The working copy is stale (not updated since operation 149761aea7d1).
    Hint: Run `jj workspace update-stale` to update it.
    See https://docs.jj-vcs.dev/latest/working-copy/#stale-working-copy for more information.
    [EOF]
    [exit status: 1]
    ");
    // It was detected that the working copy is now stale, but clean. So no
    // divergent commit should be created.
    let output = secondary_dir.run_jj(["workspace", "update-stale"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Working copy  (@) now at: pmmvwywv 90f3d42e (empty) (no description set)
    Parent commit (@-)      : qpvuntsm b853f7c8 (no description set)
    Added 0 files, modified 1 files, removed 0 files
    Updated working copy to fresh commit 90f3d42e0bff
    [EOF]
    ");
    insta::assert_snapshot!(get_log_output(&secondary_dir),
    @"
    @  90f3d42e0bff secondary@
    │ ○  3a9b690d6e67 default@
    ├─╯
    ○  b853f7c8b006
    ◆  000000000000
    [EOF]
    ");
}

/// Test a clean working copy that gets rewritten from another workspace
#[test]
fn test_workspaces_updated_by_other_automatic() {
    let test_env = TestEnvironment::default();
    test_env.add_config("snapshot.auto-update-stale = true\n");

    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    main_dir.write_file("file", "contents\n");
    main_dir.run_jj(["new"]).success();

    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();

    insta::assert_snapshot!(get_log_output(&main_dir), @"
    @  393250c59e39 default@
    │ ○  547036666102 secondary@
    ├─╯
    ○  9a462e35578a
    ◆  000000000000
    [EOF]
    ");

    // Rewrite the check-out commit in one workspace.
    main_dir.write_file("file", "changed in main\n");
    let output = main_dir.run_jj(["squash"]);
    insta::assert_snapshot!(output, @r"
    ------- stderr -------
    Rebased 1 descendant commits.
    Working copy  (@) now at: mzvwutvl 3a9b690d (empty) (no description set)
    Parent commit (@-)      : qpvuntsm b853f7c8 (no description set)
    [EOF]
    ");

    // The secondary workspace's working-copy commit was updated.
    insta::assert_snapshot!(get_log_output(&main_dir), @"
    @  3a9b690d6e67 default@
    │ ○  90f3d42e0bff secondary@
    ├─╯
    ○  b853f7c8b006
    ◆  000000000000
    [EOF]
    ");

    // The first working copy gets automatically updated.
    let output = secondary_dir.run_jj(["st"]);
    insta::assert_snapshot!(output, @"
    The working copy has no changes.
    Working copy  (@) : pmmvwywv 90f3d42e (empty) (no description set)
    Parent commit (@-): qpvuntsm b853f7c8 (no description set)
    [EOF]
    ------- stderr -------
    Working copy  (@) now at: pmmvwywv 90f3d42e (empty) (no description set)
    Parent commit (@-)      : qpvuntsm b853f7c8 (no description set)
    Added 0 files, modified 1 files, removed 0 files
    Updated working copy to fresh commit 90f3d42e0bff
    [EOF]
    ");

    insta::assert_snapshot!(get_log_output(&secondary_dir),
    @"
    @  90f3d42e0bff secondary@
    │ ○  3a9b690d6e67 default@
    ├─╯
    ○  b853f7c8b006
    ◆  000000000000
    [EOF]
    ");
}

/// Test a dirty working copy that gets rewritten from another workspace
#[test]
fn test_workspaces_updated_by_other_with_changes_in_working_copy_automatic() {
    let test_env = TestEnvironment::default();
    test_env.add_config("snapshot.auto-update-stale = true\n");

    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    main_dir.write_file("file", "contents\n");
    main_dir.run_jj(["new"]).success();

    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();

    insta::assert_snapshot!(get_log_output(&main_dir), @"
    @  393250c59e39 default@
    │ ○  547036666102 secondary@
    ├─╯
    ○  9a462e35578a
    ◆  000000000000
    [EOF]
    ");

    // Rewrite all commits from one workspace.
    main_dir.write_file("file", "changed in main\n");
    let output = main_dir.run_jj(["squash"]);
    insta::assert_snapshot!(output, @r"
    ------- stderr -------
    Rebased 1 descendant commits.
    Working copy  (@) now at: mzvwutvl 3a9b690d (empty) (no description set)
    Parent commit (@-)      : qpvuntsm b853f7c8 (no description set)
    [EOF]
    ");

    // The secondary workspace's working-copy commit was updated.
    insta::assert_snapshot!(get_log_output(&main_dir), @"
    @  3a9b690d6e67 default@
    │ ○  90f3d42e0bff secondary@
    ├─╯
    ○  b853f7c8b006
    ◆  000000000000
    [EOF]
    ");

    // The first working copy gets automatically updated.
    secondary_dir.write_file("file", "modified contents\n");
    let output = secondary_dir.run_jj(["describe", "-m", "modified"]);
    insta::assert_snapshot!(output, @r"
    ------- stderr -------
    Concurrent modification detected, resolving automatically.
    Rebased 1 descendant commits onto commits rewritten by other operation.
    Working copy  (@) now at: pmmvwywv/2 90f3d42e (divergent) (empty) (no description set)
    Parent commit (@-)      : qpvuntsm b853f7c8 (no description set)
    Added 0 files, modified 1 files, removed 0 files
    Updated working copy to fresh commit 90f3d42e0bff
    Working copy  (@) now at: pmmvwywv/0 c38323e3 (divergent) (empty) modified
    Parent commit (@-)      : qpvuntsm b853f7c8 (no description set)
    [EOF]
    ");

    // The snapshotting of the modified contents happens on top of the old
    // operation. The `describe` operation itself happens on top the reconciled
    // operation.
    let output = main_dir.run_jj(["op", "log", "-Tdescription"]);
    insta::assert_snapshot!(output, @"
    @  describe commit 90f3d42e0bff073721e2640e32c18fb1c386d7ce
    ○    reconcile divergent operations
    ├─╮
    ○ │  squash commits into 9a462e35578a347e6a3951bf7a58ad7146959a8b
    ○ │  snapshot working copy
    │ ○  snapshot working copy
    ├─╯
    ○  create initial working-copy commit in workspace secondary
    ○  add workspace 'secondary'
    ○  new empty commit
    ○  snapshot working copy
    ○  add workspace 'default'
    ○
    [EOF]
    ");

    // We get divergence between the newly described commit and the commit created
    // by snapshotting (the reconciliation happened to point secondary@ to the child
    // of the squashed commit rather than the snapshot commit).
    insta::assert_snapshot!(get_log_output(&secondary_dir),
    @r#"
    @  c38323e3e6f3 secondary@ (divergent) "modified"
    │ ×  48a90f069c8c (divergent)
    ├─╯
    │ ○  3a9b690d6e67 default@
    ├─╯
    ○  b853f7c8b006
    ◆  000000000000
    [EOF]
    "#);
}

#[test_case(false; "manual")]
#[test_case(true; "automatic")]
fn test_workspaces_current_op_discarded_by_other(automatic: bool) {
    let test_env = TestEnvironment::default();
    if automatic {
        test_env.add_config("snapshot.auto-update-stale = true\n");
    }

    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    main_dir.write_file("modified", "base\n");
    main_dir.write_file("deleted", "base\n");
    main_dir.write_file("sparse", "base\n");
    main_dir.run_jj(["new"]).success();
    main_dir.write_file("modified", "main\n");
    main_dir.run_jj(["new"]).success();

    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();
    // Make unsnapshotted writes in the secondary working copy
    secondary_dir
        .run_jj([
            "sparse",
            "set",
            "--clear",
            "--add=modified",
            "--add=deleted",
            "--add=added",
        ])
        .success();
    secondary_dir.write_file("modified", "secondary\n");
    secondary_dir.remove_file("deleted");
    secondary_dir.write_file("added", "secondary\n");

    // Create an op by abandoning the parent commit. Importantly, that commit also
    // changes the target tree in the secondary workspace.
    main_dir.run_jj(["abandon", "@-"]).success();

    let output = main_dir.run_jj([
        "operation",
        "log",
        "--template",
        r#"id.short(10) ++ " " ++ description"#,
    ]);
    insta::allow_duplicates! {
        insta::assert_snapshot!(output, @"
        @  a9b524b948 abandon commit de90575a14d8b9198dc0930f9de4a69f846ded36
        ○  0392b7d733 create initial working-copy commit in workspace secondary
        ○  766373d1f4 add workspace 'secondary'
        ○  eb6701963b new empty commit
        ○  1d937e1f1e snapshot working copy
        ○  e22ce69861 new empty commit
        ○  48aa617132 snapshot working copy
        ○  f63ee16f95 add workspace 'default'
        ○  0000000000
        [EOF]
        ");
    }

    // Abandon ops, including the one the secondary workspace is currently on.
    main_dir.run_jj(["operation", "abandon", "..@-"]).success();
    main_dir.run_jj(["util", "gc", "--expire=now"]).success();

    insta::allow_duplicates! {
        insta::assert_snapshot!(get_log_output(&main_dir), @"
        @  320bc89effc9 default@
        │ ○  891f00062e10 secondary@
        ├─╯
        ○  367415be5b44
        ◆  000000000000
        [EOF]
        ");
    }

    if automatic {
        // Run a no-op command to set the randomness seed for commit hashes.
        secondary_dir.run_jj(["help"]).success();

        let output = secondary_dir.run_jj(["st"]);
        insta::assert_snapshot!(output, @"
        Working copy changes:
        C {modified => added}
        D deleted
        M modified
        Working copy  (@) : kmkuslsw 18851b39 RECOVERY COMMIT FROM `jj workspace update-stale`
        Parent commit (@-): rzvqmyuk 891f0006 (empty) (no description set)
        [EOF]
        ------- stderr -------
        Failed to read working copy's current operation; attempting recovery. Error message from read attempt: Object 0392b7d73383f694906abd6ffd55416c30d1775a9790553062784f5c4553c09746b388aa86fb7d27b113d1096f50845e28dc24b3de8b1a9d515cbde3b44eb346 of type operation not found
        Created and checked out recovery commit 866928d1e0fd
        [EOF]
        ");
    } else {
        let output = secondary_dir.run_jj(["st"]);
        insta::assert_snapshot!(output, @"
        ------- stderr -------
        Error: Could not read working copy's operation.
        Hint: Run `jj workspace update-stale` to recover.
        See https://docs.jj-vcs.dev/latest/working-copy/#stale-working-copy for more information.
        [EOF]
        [exit status: 1]
        ");

        let output = secondary_dir.run_jj(["workspace", "update-stale"]);
        insta::assert_snapshot!(output, @"
        ------- stderr -------
        Failed to read working copy's current operation; attempting recovery. Error message from read attempt: Object 0392b7d73383f694906abd6ffd55416c30d1775a9790553062784f5c4553c09746b388aa86fb7d27b113d1096f50845e28dc24b3de8b1a9d515cbde3b44eb346 of type operation not found
        Created and checked out recovery commit 866928d1e0fd
        [EOF]
        ");
    }

    insta::allow_duplicates! {
        insta::assert_snapshot!(get_log_output(&main_dir), @r#"
        @  320bc89effc9 default@
        │ ○  18851b397d09 secondary@ "RECOVERY COMMIT FROM `jj workspace update-stale`"
        │ ○  891f00062e10
        ├─╯
        ○  367415be5b44
        ◆  000000000000
        [EOF]
        "#);
    }

    // The sparse patterns should remain
    let output = secondary_dir.run_jj(["sparse", "list"]);
    insta::allow_duplicates! {
        insta::assert_snapshot!(output, @"
        added
        deleted
        modified
        [EOF]
        ");
    }
    let output = secondary_dir.run_jj(["st"]);
    insta::allow_duplicates! {
        insta::assert_snapshot!(output, @"
        Working copy changes:
        C {modified => added}
        D deleted
        M modified
        Working copy  (@) : kmkuslsw 18851b39 RECOVERY COMMIT FROM `jj workspace update-stale`
        Parent commit (@-): rzvqmyuk 891f0006 (empty) (no description set)
        [EOF]
        ");
    }
    insta::allow_duplicates! {
        // The modified file should have the same contents it had before (not reset to
        // the base contents)
        insta::assert_snapshot!(secondary_dir.read_file("modified"), @"secondary");
    }

    let output = secondary_dir.run_jj(["evolog"]);
    if automatic {
        insta::assert_snapshot!(output, @"
        @  kmkuslsw test.user@example.com 2001-02-03 08:05:18 secondary@ 18851b39
        │  RECOVERY COMMIT FROM `jj workspace update-stale`
        │  -- operation 91f539374e6a snapshot working copy
        ○  kmkuslsw/1 test.user@example.com 2001-02-03 08:05:18 866928d1 (hidden)
           (empty) RECOVERY COMMIT FROM `jj workspace update-stale`
           -- operation 2a845e0b4514 recovery commit
        [EOF]
        ");
    } else {
        insta::assert_snapshot!(output, @"
        @  kmkuslsw test.user@example.com 2001-02-03 08:05:18 secondary@ 18851b39
        │  RECOVERY COMMIT FROM `jj workspace update-stale`
        │  -- operation 2d387a4a6355 snapshot working copy
        ○  kmkuslsw/1 test.user@example.com 2001-02-03 08:05:18 866928d1 (hidden)
           (empty) RECOVERY COMMIT FROM `jj workspace update-stale`
           -- operation 2a845e0b4514 recovery commit
        [EOF]
        ");
    }
}

#[test]
fn test_workspaces_update_stale_noop() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    let output = main_dir.run_jj(["workspace", "update-stale"]);
    insta::assert_snapshot!(output, @r"
    ------- stderr -------
    Attempted recovery, but the working copy is not stale.
    [EOF]
    ");

    let output = main_dir.run_jj(["workspace", "update-stale", "--ignore-working-copy"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: This command must be able to update the working copy.
    Hint: Don't use --ignore-working-copy.
    [EOF]
    [exit status: 1]
    ");

    let output = main_dir.run_jj(["op", "log", "-Tdescription"]);
    insta::assert_snapshot!(output, @"
    @  add workspace 'default'
    ○
    [EOF]
    ");
}

/// If the working copy was last updated to an unpublished operation, it should
/// be reported, even if the latest published operation has the same tree.
#[test]
fn test_workspaces_unpublished_operation_same_tree() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir.run_jj(["desc", "-m=A"]).success();
    let a_op_id = main_dir.current_operation_id();
    main_dir.run_jj(["new", "-m=B"]).success();
    let b_op_id = main_dir.current_operation_id();
    // Make the repo forget about the B operation
    main_dir.remove_file(format!(".jj/repo/op_heads/heads/{b_op_id}"));
    main_dir.write_file(format!(".jj/repo/op_heads/heads/{a_op_id}"), "");
    main_dir
        .run_jj(["new", "-m=C", "--ignore-working-copy"])
        .success();
    // The working copy should be stale
    let output = main_dir.run_jj(["status"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Internal error: The repo was loaded at operation eb46e46959f1, which seems to be a sibling of the working copy's operation 2a68fa5a5771
    Hint: Run `jj op integrate 2a68fa5a5771` to add the working copy's operation to the operation log.
    [EOF]
    [exit status: 255]
    ");
    // Test recovery by running `jj workspace update-stale` even though `jj op
    // integrate` is a better solution
    let output = main_dir.run_jj(["workspace", "update-stale"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Working copy  (@) now at: zsuskuln 36a15ac4 (empty) C
    Parent commit (@-)      : qpvuntsm 8777db25 (empty) A
    Updated working copy to fresh commit 36a15ac414e8
    [EOF]
    ");
}

/// Test "update-stale" in a dirty, but not stale working copy.
#[test]
fn test_workspaces_update_stale_snapshot() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    main_dir.write_file("file", "changed in main\n");
    main_dir.run_jj(["new"]).success();
    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();

    // Record new operation in one workspace.
    main_dir.run_jj(["new"]).success();

    // Snapshot the other working copy, which unfortunately results in concurrent
    // operations, but should be resolved cleanly.
    secondary_dir.write_file("file", "changed in second\n");
    let output = secondary_dir.run_jj(["workspace", "update-stale"]);
    insta::assert_snapshot!(output, @r"
    ------- stderr -------
    Concurrent modification detected, resolving automatically.
    Attempted recovery, but the working copy is not stale.
    [EOF]
    ");

    insta::assert_snapshot!(get_log_output(&secondary_dir), @"
    @  35d779b3baea secondary@
    │ ○  c9516583d53b default@
    │ ○  f6ae7810ef56
    ├─╯
    ○  7d5738ba9943
    ◆  000000000000
    [EOF]
    ");
}

/// Test that "workspace update-stale" works in colocated repos.
///
/// This is a regression test for a bug introduced in commit 7a296ca1 where
/// the reload-to-HEAD logic (added to fix a race condition) would break
/// "workspace update-stale" by reloading the repo to HEAD before snapshotting,
/// even though recovery intentionally loads at an old operation.
#[test]
fn test_colocated_workspace_update_stale() {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");
    let git_repo = git::open(main_dir.root());

    main_dir.write_file("file", "contents\n");
    main_dir.run_jj(["new"]).success();

    // Create new bookmarked revision from the main workspace.
    main_dir
        .run_jj(["new", "--no-edit", "root()", "-mold book1"])
        .success();
    main_dir
        .run_jj(["bookmark", "set", "-rsubject('old book1')", "book1"])
        .success();

    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();

    // Rewrite the check-out commit from the secondary workspace.
    // This makes the main (colocated) workspace's working copy stale.
    secondary_dir.write_file("file", "changed in secondary\n");
    secondary_dir.run_jj(["squash"]).success();

    // Update and export the bookmark from the secondary workspace.
    secondary_dir
        .run_jj(["new", "--no-edit", "root()", "-mnew book1"])
        .success();
    secondary_dir
        .run_jj([
            "bookmark",
            "set",
            "-rsubject('new book1')",
            "--allow-backwards",
            "book1",
        ])
        .success();
    secondary_dir.run_jj(["git", "export"]).success();

    // Create new Git ref and commit which will be imported later by "jj
    // workspace update-stale".
    git::add_commit(&git_repo, "refs/heads/book2", "file", b"", "book2", &[]);

    insta::assert_snapshot!(get_log_output(&secondary_dir), @r#"
    @  9cb8253861b5 secondary@
    │ ○  f562bf82f2da default@
    ├─╯
    ○  30ed2f28b710
    │ ○  7fe3ff3b9a60 book2 "book2"
    ├─╯
    │ ○  e97ad7861f78 book1 "new book1"
    ├─╯
    │ ○  f656b467890b "old book1"
    ├─╯
    ◆  000000000000
    [EOF]
    ------- stderr -------
    Done importing changes from the underlying Git repo.
    [EOF]
    "#);

    // The main workspace's working copy is now stale. The secondary
    // workspace's own HEAD must not leak into the repo-wide git_head: `jj st`
    // in the main workspace reports the staleness instead of "recovering" to
    // a divergent parent.
    let output = main_dir.run_jj(["st"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: The working copy is stale (not updated since operation 572e45b3fba3).
    Hint: Run `jj workspace update-stale` to update it.
    See https://docs.jj-vcs.dev/latest/working-copy/#stale-working-copy for more information.
    [EOF]
    [exit status: 1]
    ");

    // Before the fix, this would fail with the same "working copy is stale"
    // error because the colocated repo reload logic would reload to HEAD
    // before snapshotting, breaking the recovery.
    let output = main_dir.run_jj(["workspace", "update-stale"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Working copy  (@) now at: rlvkpnrz f562bf82 (empty) (no description set)
    Parent commit (@-)      : qpvuntsm 30ed2f28 (no description set)
    Added 0 files, modified 1 files, removed 0 files
    Updated working copy to fresh commit f562bf82f2da
    [EOF]
    ");

    // Verify the workspace is now up-to-date. New bookmark "book2" should have
    // been imported by the previous command.
    let output = main_dir.run_jj(["st"]);
    insta::assert_snapshot!(output, @"
    The working copy has no changes.
    Working copy  (@) : rlvkpnrz f562bf82 (empty) (no description set)
    Parent commit (@-): qpvuntsm 30ed2f28 (no description set)
    [EOF]
    ");

    // The updated bookmark "book1" shouldn't be re-imported as an external
    // change. If it were, the "old book1" revision would be abandoned.
    insta::assert_snapshot!(get_log_output(&main_dir), @r#"
    @  f562bf82f2da default@
    │ ○  9cb8253861b5 secondary@
    ├─╯
    ○  30ed2f28b710
    │ ○  7fe3ff3b9a60 book2 "book2"
    ├─╯
    │ ○  e97ad7861f78 book1 "new book1"
    ├─╯
    │ ○  f656b467890b "old book1"
    ├─╯
    ◆  000000000000
    [EOF]
    "#);
}

/// Test that bookmark changes made in a secondary workspace are exported to
/// the underlying Git repo, keeping refs/heads/* and the @git tracking
/// bookmarks in sync (e.g. after `jj git push`).
#[test]
fn test_secondary_workspace_exports_git_refs() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");
    let git_repo = git::open(main_dir.root());

    main_dir.write_file("file", "contents\n");
    main_dir.run_jj(["commit", "-m", "initial"]).success();
    main_dir.run_jj(["bookmark", "create", "-r@-", "book1"]).success();

    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();

    // Move the bookmark forward from the secondary workspace.
    secondary_dir.write_file("file", "changed in secondary\n");
    secondary_dir.run_jj(["squash", "--into", "book1"]).success();
    secondary_dir.run_jj(["new", "book1"]).success();

    // The Git ref should have been exported by the secondary workspace, and
    // the @git tracking bookmark should be in sync (no "behind by" lag).
    let output = secondary_dir.run_jj(["bookmark", "list", "--all", "book1"]);
    insta::assert_snapshot!(output, @r"
    book1: qpvuntsm 9957cd26 initial
      @git: qpvuntsm 9957cd26 initial
    [EOF]
    ");

    let book1_commit_id = format!(
        "{:?}",
        secondary_dir
            .run_jj(["log", "-r", "book1", "--no-graph", "-T", "commit_id"])
            .success()
            .stdout
    );
    assert_eq!(
        git_repo.find_reference("refs/heads/book1")?.id().to_string(),
        book1_commit_id.trim().trim_matches('"'),
    );

    Ok(())
}

/// Test that a secondary worktree's own HEAD is not imported into the
/// repo-wide git_head. The main and secondary workspaces usually have
/// different Git HEADs (each synced to its own @-); if the worktree's HEAD
/// were fed to import_head(), every command alternating between the two
/// workspaces would spuriously "import git head" and reset the working copy.
#[test]
fn test_secondary_workspace_head_does_not_clobber_git_head() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    main_dir.write_file("file", "contents\n");
    main_dir.run_jj(["commit", "-m", "c1"]).success();
    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();

    // Diverge the workspaces' @-, so their Git HEADs differ.
    main_dir.write_file("file", "changed in main\n");
    main_dir.run_jj(["commit", "-m", "c2"]).success();

    // Alternating commands must not produce "import git head" operations or
    // working-copy resets.
    for dir in [&main_dir, &secondary_dir, &main_dir, &secondary_dir] {
        let output = dir.run_jj(["st"]);
        let stderr = output.stderr.raw();
        assert!(
            !stderr.contains("Reset the working copy parent to the new Git HEAD"),
            "spurious git HEAD import in {}: {stderr}",
            dir.root().display(),
        );
    }
    Ok(())
}

/// Test that a secondary workspace backed by a fallback `gitdir:` pointer
/// (written when `git worktree add` fails, e.g. on an unborn HEAD) does not
/// touch the main repo's HEAD or index. The fallback shares the main repo's
/// git dir, so HEAD/index syncs from the secondary would corrupt the main
/// workspace's Git state.
#[test]
fn test_fallback_gitdir_pointer_workspace_preserves_main_head() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    // `git worktree add` fails on an unborn HEAD, so jj falls back to a raw
    // gitdir pointer sharing the main repo's git dir.
    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();
    let dot_git = std::fs::read_to_string(secondary_dir.root().join(".git"))?;
    let gitdir = dot_git.trim().strip_prefix("gitdir:").unwrap().trim();
    assert_eq!(
        dunce::canonicalize(gitdir)?,
        dunce::canonicalize(main_dir.root().join(".git"))?,
        "expected fallback gitdir pointer, got: {dot_git}"
    );

    main_dir.write_file("file", "contents\n");
    main_dir.run_jj(["commit", "-m", "c1"]).success();
    let git_repo = git::open(main_dir.root());
    let main_head_before = git_repo.head_id()?.to_string();

    // Commit in the secondary workspace.
    secondary_dir.write_file("other", "from secondary\n");
    secondary_dir.run_jj(["commit", "-m", "secondary work"]).success();

    // The main repo's HEAD and index must be untouched.
    assert_eq!(
        git_repo.head_id()?.to_string(),
        main_head_before,
        "secondary workspace must not move the main repo's HEAD"
    );
    let output = std::process::Command::new("git")
        .current_dir(main_dir.root())
        .args(["status", "--porcelain=v1"])
        .output()?;
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "",
        "main workspace's git index must not be clobbered by the secondary"
    );
    Ok(())
}

/// Test that bookmark changes made in a secondary workspace of a
/// non-colocated (clone-style) repo are exported to the backing Git store.
/// The linked worktree's `.git` file points into `.jj/repo/store/git`, which
/// the worktree detection must recognize.
#[test]
fn test_clone_style_secondary_workspace_exports_git_refs() -> TestResult {
    let test_env = TestEnvironment::default();
    let origin_repo = git::init(test_env.env_root().join("origin"));
    git::add_commit(
        &origin_repo,
        "refs/heads/main",
        "file",
        b"contents\n",
        "initial",
        &[],
    );
    git::set_symbolic_reference(&origin_repo, "HEAD", "refs/heads/main");
    test_env
        .run_jj_in(".", ["git", "clone", "origin", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    main_dir.write_file("file", "contents\n");
    main_dir.run_jj(["new", "-m", "initial"]).success();
    main_dir.run_jj(["bookmark", "create", "-r@", "book1"]).success();

    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();

    secondary_dir.write_file("file", "changed in secondary\n");
    secondary_dir.run_jj(["squash", "--into", "book1"]).success();
    secondary_dir.run_jj(["new", "book1"]).success();

    let book1_commit_id = format!(
        "{:?}",
        secondary_dir
            .run_jj(["log", "-r", "book1", "--no-graph", "-T", "commit_id"])
            .success()
            .stdout
    );
    // `jj git clone` is not colocated: the backing Git store lives in the jj
    // repo dir, not at `<workspace>/.git`.
    let git_repo = git::open(main_dir.root().join(".jj/repo/store/git"));
    assert_eq!(
        git_repo.find_reference("refs/heads/book1")?.id().to_string(),
        book1_commit_id.trim().trim_matches('"'),
    );
    Ok(())
}

/// Test that the linked-worktree HEAD sync never moves an attached branch:
/// if the user points the secondary worktree's HEAD at a branch with git,
/// jj operations must detach the HEAD instead of force-updating the branch
/// ref behind jj's back.
#[test]
fn test_secondary_workspace_attached_head_not_moved() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    main_dir.write_file("file", "contents\n");
    main_dir.run_jj(["commit", "-m", "initial"]).success();
    main_dir.run_jj(["bookmark", "create", "-r@-", "book1"]).success();

    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();

    // Attach the secondary worktree's HEAD to the book1 branch, as a plain
    // `git checkout book1` would.
    let git_repo = git::open(secondary_dir.root());
    git::set_symbolic_reference(&git_repo, "HEAD", "refs/heads/book1");

    // Run an operation in the secondary workspace that syncs its HEAD.
    secondary_dir.run_jj(["new", "-m", "secondary work"]).success();

    // refs/heads/book1 must not have been force-moved by the HEAD sync.
    let book1_commit_id = format!(
        "{:?}",
        secondary_dir
            .run_jj(["log", "-r", "book1", "--no-graph", "-T", "commit_id"])
            .success()
            .stdout
    );
    assert_eq!(
        git_repo.find_reference("refs/heads/book1")?.id().to_string(),
        book1_commit_id.trim().trim_matches('"'),
    );
    // The worktree's HEAD is detached at the working copy's parent.
    let parent_commit_id = format!(
        "{:?}",
        secondary_dir
            .run_jj(["log", "-r", "@-", "--no-graph", "-T", "commit_id"])
            .success()
            .stdout
    );
    assert_eq!(
        git_repo.find_reference("HEAD")?.id().to_string(),
        parent_commit_id.trim().trim_matches('"'),
    );
    Ok(())
}

/// Test forgetting workspaces
#[test]
fn test_workspaces_forget() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file", "contents");
    main_dir.run_jj(["new"]).success();

    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();
    let output = main_dir.run_jj(["workspace", "forget"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Warning: The current workspace 'default' no longer exists after this operation. The working copy was left untouched.
    Hint: Restore to an operation that contains the workspace (e.g. `jj undo` or `jj redo`).
    [EOF]
    ");

    // When listing workspaces, only the secondary workspace shows up
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    secondary: ../secondary pmmvwywv 31da1455 (empty) (no description set)
    [EOF]
    ");

    // After forgetting the default, secondary root is still recorded, default no
    // longer exists
    let output = main_dir.run_jj(["workspace", "root", "--name", "secondary"]);
    insta::assert_snapshot!(output, @"
    $TEST_ENV/secondary
    [EOF]
    ");
    let output = main_dir.run_jj(["workspace", "root", "--name", "default"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: No such workspace: default
    [EOF]
    [exit status: 1]
    ");

    // The old working copy doesn't get an "@" in the log output
    // TODO: It seems useful to still have the "secondary@" marker here even though
    // there's only one workspace. We should show it when the command is not run
    // from that workspace.
    insta::assert_snapshot!(get_log_output(&main_dir), @"
    ○  31da14559558
    ○  006bd1130b84
    ◆  000000000000
    [EOF]
    ");

    // Revision "@" cannot be used
    let output = main_dir.run_jj(["log", "-r", "@"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: Workspace `default` doesn't have a working-copy commit
    [EOF]
    [exit status: 1]
    ");

    // Try to add back the workspace
    // TODO: We should make this just add it back instead of failing
    let output = main_dir.run_jj(["workspace", "add", "."]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: Destination path exists and is not an empty directory
    [EOF]
    [exit status: 1]
    ");

    // Add a third workspace...
    main_dir.run_jj(["workspace", "add", "../third"]).success();
    // ... and then forget it, a non-existent one, and the secondary workspace too
    let output = main_dir.run_jj(["workspace", "forget", "secondary", "nonexistent", "third"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Warning: No such workspace: nonexistent
    [EOF]
    ");
    // No workspaces left
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output, @"");
}

/// Test forgetting workspace created before workspace store
#[test]
fn test_workspaces_forget_from_before_workspace_store() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir.remove_dir_all(".jj/repo/workspace_store");

    let output = main_dir.run_jj(["workspace", "forget"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Warning: The current workspace 'default' no longer exists after this operation. The working copy was left untouched.
    Hint: Restore to an operation that contains the workspace (e.g. `jj undo` or `jj redo`).
    [EOF]
    ");

    // No workspaces left
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output, @"");
}

#[test]
fn test_workspaces_forget_nothing_changed() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    let output = main_dir.run_jj(["workspace", "forget", "second", "third"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Warning: No such workspace: second
    Warning: No such workspace: third
    Nothing changed.
    [EOF]
    ");
}

#[test]
fn test_workspaces_forget_multi_transaction() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file", "contents");
    main_dir.run_jj(["new"]).success();

    main_dir.run_jj(["workspace", "add", "../second"]).success();
    main_dir.run_jj(["workspace", "add", "../third"]).success();

    // there should be three workspaces
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . rlvkpnrz f6bf8819 (empty) (no description set)
    second: ../second pmmvwywv 31da1455 (empty) (no description set)
    third: ../third rzvqmyuk bf5b5b4d (empty) (no description set)
    [EOF]
    ");

    // delete two at once, in a single tx
    main_dir
        .run_jj(["workspace", "forget", "second", "third", "fourth"])
        .success();
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . rlvkpnrz f6bf8819 (empty) (no description set)
    [EOF]
    ");

    // the op log should have the multiple valid workspaces forgotten in a single tx
    let output = main_dir.run_jj(["op", "log", "--limit", "1"]);
    insta::assert_snapshot!(output, @"
    @  da3075edb24f test-username@host.example.com default@ 2001-02-03 04:05:12.000 +07:00 - 2001-02-03 04:05:12.000 +07:00
    │  forget workspaces second, third
    │  args: jj workspace forget second third fourth
    [EOF]
    ");

    // now, undo, and that should restore both workspaces
    main_dir.run_jj(["undo"]).success();

    // finally, there should be three workspaces at the end
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . rlvkpnrz f6bf8819 (empty) (no description set)
    second: pmmvwywv 31da1455 (empty) (no description set)
    third: rzvqmyuk bf5b5b4d (empty) (no description set)
    [EOF]
    ");
}

#[test]
fn test_workspaces_forget_abandon_commits() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file", "contents");

    main_dir.run_jj(["workspace", "add", "../second"]).success();
    main_dir.run_jj(["workspace", "add", "../third"]).success();
    main_dir.run_jj(["workspace", "add", "../fourth"]).success();
    let third_dir = test_env.work_dir("third");
    third_dir.run_jj(["edit", "second@"]).success();
    let fourth_dir = test_env.work_dir("fourth");
    fourth_dir.run_jj(["edit", "second@"]).success();

    // there should be four workspaces, three of which are at the same empty commit
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . qpvuntsm 006bd113 (no description set)
    fourth: ../fourth uuqppmxq 94f41578 (empty) (no description set)
    second: ../second uuqppmxq 94f41578 (empty) (no description set)
    third: ../third uuqppmxq 94f41578 (empty) (no description set)
    [EOF]
    ");
    insta::assert_snapshot!(get_log_output(&main_dir), @"
    @  006bd1130b84 default@
    │ ○  94f41578a9e1 fourth@ second@ third@
    ├─╯
    ◆  000000000000
    [EOF]
    ");

    // delete the default workspace (should not abandon commit since not empty)
    main_dir
        .run_jj(["workspace", "forget", "default"])
        .success();
    insta::assert_snapshot!(get_log_output(&main_dir), @"
    ○  94f41578a9e1 fourth@ second@ third@
    │ ○  006bd1130b84
    ├─╯
    ◆  000000000000
    [EOF]
    ");

    // delete the second workspace (should not abandon commit since other workspaces
    // still have commit checked out)
    main_dir.run_jj(["workspace", "forget", "second"]).success();
    insta::assert_snapshot!(get_log_output(&main_dir), @"
    ○  94f41578a9e1 fourth@ third@
    │ ○  006bd1130b84
    ├─╯
    ◆  000000000000
    [EOF]
    ");

    // delete the last 2 workspaces (commit should be abandoned now even though
    // forgotten in same tx)
    main_dir
        .run_jj(["workspace", "forget", "third", "fourth"])
        .success();
    insta::assert_snapshot!(get_log_output(&main_dir), @"
    ○  006bd1130b84
    ◆  000000000000
    [EOF]
    ");
}

/// Test context of commit summary template
#[test]
fn test_list_workspaces_template() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    test_env.add_config(
        r#"
        templates.workspace_list = """name ++ ": " ++ target.commit_id().short() ++ " " ++
                                      target.description().first_line() ++
                                      if(target.current_working_copy(), " (current)") ++ "\n""""
        "#,
    );
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    main_dir.write_file("file", "contents");
    main_dir.run_jj(["commit", "-m", "initial"]).success();
    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../secondary"])
        .success();

    // "current_working_copy" should point to the workspace we operate on
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output, @"
    default: 504e3d8c1bcd  (current)
    second: 058f604dffcd 
    [EOF]
    ");

    let output = secondary_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output, @"
    default: 504e3d8c1bcd 
    second: 058f604dffcd  (current)
    [EOF]
    ");

    // Using template option
    let template = r#"name ++ ": " ++ target.commit_id().short() ++ "\n""#;
    let output = main_dir.run_jj(["workspace", "list", "-T", template]);
    insta::assert_snapshot!(output, @"
    default: 504e3d8c1bcd
    second: 058f604dffcd
    [EOF]
    ");
}

#[test]
fn test_list_workspaces_template_root() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../secondary"])
        .success();

    let template = r#"name ++ ": " ++ root ++ "\n""#;
    let output = main_dir.run_jj(["workspace", "list", "-T", template]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: $TEST_ENV/main
    second: $TEST_ENV/secondary
    [EOF]
    ");

    let template = r#"name ++ ": " ++ if(root, root ++ " " ++ root.relative()) ++ "\n""#;
    let output = main_dir.run_jj(["workspace", "list", "-T", template]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: $TEST_ENV/main .
    second: $TEST_ENV/secondary ../secondary
    [EOF]
    ");

    let template = r#"name ++ ": " ++ if(root, root.relative()) ++ "\n""#;
    let output = main_dir.run_jj(["workspace", "list", "-T", template]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: .
    second: ../secondary
    [EOF]
    ");
}

#[test]
fn test_list_workspaces_template_root_unavailable() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../secondary"])
        .success();
    std::fs::remove_dir_all(test_env.env_root().join("secondary")).unwrap();

    let template = r#"name ++ ": " ++ root ++ "\n""#;
    let output = main_dir.run_jj(["workspace", "list", "-T", template]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: $TEST_ENV/main
    second: 
    [EOF]
    ");

    let template = r#"name ++ ": " ++ if(root, root.relative()) ++ "\n""#;
    let output = main_dir.run_jj(["workspace", "list", "-T", template]);
    insta::assert_snapshot!(output, @"
    default: .
    second: 
    [EOF]
    ");

    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output, @"
    default: . qpvuntsm e8849ae1 (empty) (no description set)
    second: uuqppmxq 94f41578 (empty) (no description set)
    [EOF]
    ");
}

/// Test getting the workspace root from primary and secondary workspaces
#[test]
fn test_workspaces_root() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    let secondary_dir = test_env.work_dir("secondary");

    let output = main_dir.run_jj(["workspace", "root"]);
    insta::assert_snapshot!(output, @"
    $TEST_ENV/main
    [EOF]
    ");
    let main_subdir_dir = main_dir.create_dir("subdir");
    let output = main_subdir_dir.run_jj(["workspace", "root"]);
    insta::assert_snapshot!(output, @"
    $TEST_ENV/main
    [EOF]
    ");

    main_dir
        .run_jj(["workspace", "add", "--name", "secondary", "../secondary"])
        .success();
    // Explicitly request root of 'secondary' workspace from main workspace
    let output = main_dir.run_jj(["workspace", "root", "--name", "secondary"]);
    insta::assert_snapshot!(output, @"
    $TEST_ENV/secondary
    [EOF]
    ");
    let output = secondary_dir.run_jj(["workspace", "root"]);
    insta::assert_snapshot!(output, @"
    $TEST_ENV/secondary
    [EOF]
    ");
    let secondary_subdir_dir = secondary_dir.create_dir("subdir");
    let output = secondary_subdir_dir.run_jj(["workspace", "root"]);
    insta::assert_snapshot!(output, @"
    $TEST_ENV/secondary
    [EOF]
    ");
}

#[test]
fn test_workspaces_relative_path() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();

    let repo_file = test_env.env_root().join("secondary/.jj/repo");
    let repo_path_bytes = std::fs::read(&repo_file)?;
    let stored_path = String::from_utf8(repo_path_bytes)?;
    assert_eq!(stored_path, "../../main/.jj/repo");

    let secondary_dir = test_env.work_dir("secondary");
    let output = secondary_dir.run_jj(["status"]);
    insta::assert_snapshot!(output, @"
    The working copy has no changes.
    Working copy  (@) : uuqppmxq 94f41578 (empty) (no description set)
    Parent commit (@-): zzzzzzzz 00000000 (empty) (no description set)
    [EOF]
    ");

    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . qpvuntsm e8849ae1 (empty) (no description set)
    secondary: ../secondary uuqppmxq 94f41578 (empty) (no description set)
    [EOF]
    ");

    let output = secondary_dir.run_jj(["workspace", "root"]);
    insta::assert_snapshot!(output, @"
    $TEST_ENV/secondary
    [EOF]
    ");

    let secondary_subdir = secondary_dir.create_dir("subdir");
    let output = secondary_subdir.run_jj(["workspace", "root"]);
    insta::assert_snapshot!(output, @"
    $TEST_ENV/secondary
    [EOF]
    ");

    let output = main_dir.run_jj(["workspace", "root", "--name", "secondary"]);
    insta::assert_snapshot!(output, @"
    $TEST_ENV/secondary
    [EOF]
    ");
    Ok(())
}

#[test]
fn test_workspaces_root_unavailable() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();

    std::fs::remove_dir_all(test_env.env_root().join("secondary"))?;

    let output = main_dir.run_jj(["workspace", "root", "--name", "secondary"]);
    insta::assert_snapshot!(output.normalize_backslash().strip_stderr_last_line(), @"
    ------- stderr -------
    Error: Cannot resolve absolute workspace path: $TEST_ENV/main/.jj/repo/../../../secondary
    [EOF]
    [exit status: 1]
    ");
    Ok(())
}

#[test]
fn test_debug_snapshot() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    work_dir.write_file("file", "contents");
    work_dir.run_jj(["debug", "snapshot"]).success();
    let output = work_dir.run_jj(["op", "log"]);
    insta::assert_snapshot!(output, @"
    @  d870c9898b59 test-username@host.example.com default@ 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  snapshot working copy
    │  args: jj debug snapshot
    ○  f63ee16f9553 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ○  000000000000 root()
    [EOF]
    ");
    work_dir.run_jj(["describe", "-m", "initial"]).success();
    let output = work_dir.run_jj(["op", "log"]);
    insta::assert_snapshot!(output, @"
    @  fd7a8fca455a test-username@host.example.com default@ 2001-02-03 04:05:10.000 +07:00 - 2001-02-03 04:05:10.000 +07:00
    │  describe commit 006bd1130b84e90ab082adeabd7409270d5a86da
    │  args: jj describe -m initial
    ○  d870c9898b59 test-username@host.example.com default@ 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    │  snapshot working copy
    │  args: jj debug snapshot
    ○  f63ee16f9553 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    │  add workspace 'default'
    ○  000000000000 root()
    [EOF]
    ");
}

#[test]
fn test_workspaces_rename_nothing_changed() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    let output = main_dir.run_jj(["workspace", "rename", "default"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Nothing changed.
    [EOF]
    ");
}

#[test]
fn test_workspaces_rename_new_workspace_name_already_used() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../secondary"])
        .success();
    let output = main_dir.run_jj(["workspace", "rename", "second"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: Failed to rename a workspace
    Caused by: Workspace second already exists
    [EOF]
    [exit status: 1]
    ");
}

#[test]
fn test_workspaces_rename_forgotten_workspace() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../secondary"])
        .success();
    main_dir.run_jj(["workspace", "forget", "second"]).success();
    let secondary_dir = test_env.work_dir("secondary");
    let output = secondary_dir.run_jj(["workspace", "rename", "third"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: The current workspace 'second' is not tracked in the repo.
    [EOF]
    [exit status: 1]
    ");
}

#[test]
fn test_workspaces_rename_workspace() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");
    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../secondary"])
        .success();
    let secondary_dir = test_env.work_dir("secondary");

    // Both workspaces show up when we list them
    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . qpvuntsm e8849ae1 (empty) (no description set)
    second: ../secondary uuqppmxq 94f41578 (empty) (no description set)
    [EOF]
    ");

    let output = secondary_dir.run_jj(["workspace", "rename", "third"]);
    insta::assert_snapshot!(output, @"");

    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output.normalize_backslash(), @"
    default: . qpvuntsm e8849ae1 (empty) (no description set)
    third: ../secondary uuqppmxq 94f41578 (empty) (no description set)
    [EOF]
    ");

    // Can see the working-copy commit in each workspace in the log output.
    insta::assert_snapshot!(get_log_output(&main_dir), @"
    @  e8849ae12c70 default@
    │ ○  94f41578a9e1 third@
    ├─╯
    ◆  000000000000
    [EOF]
    ");
    insta::assert_snapshot!(get_log_output(&secondary_dir), @"
    @  94f41578a9e1 third@
    │ ○  e8849ae12c70 default@
    ├─╯
    ◆  000000000000
    [EOF]
    ");

    // The new workspace root is recorded and accessible
    let output = main_dir.run_jj(["workspace", "root", "--name", "secondary"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: No such workspace: secondary
    [EOF]
    [exit status: 1]
    ");
    let output = main_dir.run_jj(["workspace", "root", "--name", "third"]);
    insta::assert_snapshot!(output, @"
    $TEST_ENV/secondary
    [EOF]
    ");
}

#[test]
fn test_workspaces_rename_workspace_from_before_workspace_store() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "main"]).success();
    let main_dir = test_env.work_dir("main");

    main_dir.remove_dir_all(".jj/repo/workspace_store");

    let output = main_dir.run_jj(["workspace", "rename", "third"]);
    insta::assert_snapshot!(output, @"");

    let output = main_dir.run_jj(["workspace", "list"]);
    insta::assert_snapshot!(output, @"
    third: qpvuntsm e8849ae1 (empty) (no description set)
    [EOF]
    ");

    // The workspace root is not in the store
    let output = main_dir.run_jj(["workspace", "root", "--name", "third"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Error: Workspace has no recorded path: third
    [EOF]
    [exit status: 1]
    ");

    let output = main_dir.run_jj(["workspace", "list", "-T", r#"name ++ ": " ++ root ++ "\n""#]);
    insta::assert_snapshot!(output, @"
    third: 
    [EOF]
    ");
}

#[must_use]
fn get_log_output(work_dir: &TestWorkDir) -> CommandOutput {
    let template = r#"
    separate(" ",
      commit_id.short(),
      bookmarks,
      working_copies,
      if(divergent, "(divergent)"),
      surround('"', '"', description.first_line()),
    )
    "#;
    work_dir.run_jj(["log", "-T", template, "-r", "all()"])
}

/// Test that `jj workspace add` creates `.jj/.gitignore` in the new workspace
/// so that git doesn't track jj's internal state.
#[test]
fn test_workspace_add_creates_jj_gitignore() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file", "contents");
    main_dir.run_jj(["commit", "-m", "initial"]).success();

    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../secondary"])
        .success();
    let secondary_dir = test_env.work_dir("secondary");

    // .jj/.gitignore should exist with content "/*\n"
    let gitignore_path = secondary_dir.root().join(".jj").join(".gitignore");
    assert!(
        gitignore_path.exists(),
        ".jj/.gitignore should exist in the new workspace"
    );
    let content = std::fs::read_to_string(&gitignore_path)?;
    assert_eq!(
        content, "/*\n",
        ".jj/.gitignore should contain '/*\\n' to ignore all files in .jj"
    );

    Ok(())
}

/// Test that `git status` is clean after `jj workspace add`.
///
/// The workspace should have a proper git worktree (not just a gitdir pointer)
/// with its own index, and `git update-index --refresh` should have been run
/// so that git doesn't show spurious modifications.
#[test]
fn test_workspace_add_git_status_clean() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file", "contents");
    main_dir.run_jj(["commit", "-m", "initial"]).success();

    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../secondary"])
        .success();
    let secondary_dir = test_env.work_dir("secondary");

    // The workspace should have a .git file (linked worktree), not a bare
    // gitdir: pointer. A linked worktree's .git file contains "gitdir: <path>"
    // pointing to the main repo's worktree directory.
    let dot_git = secondary_dir.root().join(".git");
    assert!(dot_git.exists(), ".git should exist in the new workspace");

    // git status should be clean (no modifications).
    let output = std::process::Command::new("git")
        .current_dir(secondary_dir.root())
        .args(["status", "--porcelain=v1"])
        .output()?;
    assert!(
        output.status.success(),
        "git status should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Filter out .jj/ entries — those are ignored by .jj/.gitignore.
    let non_ignored: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.contains(".jj/"))
        .collect();
    assert!(
        non_ignored.is_empty(),
        "git status should be clean (ignoring .jj/), but found: {non_ignored:?}"
    );

    Ok(())
}

/// Test that `jj workspace add` creates a proper git worktree with its own
/// index, not just a `gitdir:` pointer file that shares the main repo's index.
#[test]
fn test_workspace_add_creates_git_worktree() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file", "contents");
    main_dir.run_jj(["commit", "-m", "initial"]).success();

    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../secondary"])
        .success();

    // The main repo should have a worktree registered for "secondary".
    let output = std::process::Command::new("git")
        .current_dir(main_dir.root())
        .args(["worktree", "list", "--porcelain"])
        .output()?;
    assert!(output.status.success());
    let worktree_list = String::from_utf8_lossy(&output.stdout);
    assert!(
        worktree_list.contains("secondary"),
        "git worktree list should contain the secondary workspace, got: {worktree_list}"
    );

    // The secondary workspace should have its own index file (not sharing
    // the main repo's index). This is stored inside the worktree's git dir
    // under the main repo's .git/worktrees/<name>/index.
    // In a colocated repo, .git is a directory, not a file. The worktree's
    // .git file points to <main>/.git/worktrees/<name>.
    let secondary_dot_git =
        std::fs::read_to_string(test_env.work_dir("secondary").root().join(".git"))?;
    assert!(
        secondary_dot_git.starts_with("gitdir: "),
        "secondary .git should be a gitdir pointer, got: {secondary_dot_git}"
    );
    let worktree_gitdir = secondary_dot_git
        .strip_prefix("gitdir: ")
        .unwrap()
        .trim();
    // The worktree gitdir should be under the main repo's .git/worktrees/
    assert!(
        worktree_gitdir.contains("worktrees"),
        "worktree gitdir should be under .git/worktrees/, got: {worktree_gitdir}"
    );

    // After jj checks out files, the index should exist and be valid.
    // Running git status (which requires a valid index) should succeed.
    let output = std::process::Command::new("git")
        .current_dir(test_env.work_dir("secondary").root())
        .args(["status", "--porcelain=v1"])
        .output()?;
    assert!(
        output.status.success(),
        "git status should succeed in the worktree, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the index exists by listing files.
    let output = std::process::Command::new("git")
        .current_dir(test_env.work_dir("secondary").root())
        .args(["ls-files"])
        .output()?;
    assert!(output.status.success());
    let files = String::from_utf8_lossy(&output.stdout);
    assert!(
        files.contains("file"),
        "git ls-files should list 'file' from the checked-out tree, got: {files}"
    );

    Ok(())
}

/// Test that `jj workspace add` creates a .git pointer in a non-colocated repo.
///
/// In a non-colocated repo (created with `jj git init` without `--colocate`),
/// the git backend is a bare repo. `git worktree add` from a bare repo may
/// fail, so jj falls back to writing a `gitdir:` pointer. Either way, the
/// workspace should function correctly: files should be checked out and
/// `git` commands should be able to see the repo.
#[test]
fn test_workspace_add_non_colocated_git_worktree() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "main"])
        .success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file", "contents");
    main_dir.run_jj(["commit", "-m", "initial"]).success();

    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../secondary"])
        .success();
    let secondary_dir = test_env.work_dir("secondary");

    // The secondary workspace should have a .git file pointing to the git repo.
    let dot_git = secondary_dir.root().join(".git");
    assert!(dot_git.exists(), ".git should exist in secondary workspace");

    let dot_git_content = std::fs::read_to_string(&dot_git)?;
    assert!(
        dot_git_content.starts_with("gitdir: "),
        "secondary .git should be a gitdir pointer, got: {dot_git_content}"
    );

    // Files should be checked out in the secondary workspace.
    assert!(
        secondary_dir.root().join("file").exists(),
        "file should be checked out in secondary workspace"
    );
    let content = secondary_dir.read_file("file");
    assert_eq!(
        content, "contents",
        "file content should match in secondary workspace"
    );

    // jj should see the workspace as clean (no pending changes).
    let output = secondary_dir.run_jj(["diff", "--summary"]);
    assert!(
        output.stdout.is_empty(),
        "secondary workspace should be clean, got: {output}"
    );

    Ok(())
}

/// Test that `jj edit` in a linked worktree (non-colocated workspace) syncs
/// the worktree's git HEAD and index, so `git status` shows correct state.
#[test]
fn test_workspace_edit_syncs_git_head() {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");

    // Create commits A, B, C
    main_dir.write_file("f1.txt", "f1");
    main_dir.run_jj(["describe", "-m", "A"]).success();
    main_dir.run_jj(["new", "-m", "B"]).success();
    main_dir.write_file("f2.txt", "f2");
    main_dir.run_jj(["describe", "-m", "B"]).success();
    main_dir.run_jj(["new", "-m", "C"]).success();
    main_dir.write_file("f3.txt", "f3");
    main_dir.run_jj(["describe", "-m", "C"]).success();

    // Create a workspace
    main_dir
        .run_jj(["workspace", "add", "--name", "ws", "../ws"])
        .success();
    let ws_dir = test_env.work_dir("ws");

    // Get change IDs for A, B, C
    let log_output = ws_dir.run_jj([
        "log",
        "-T",
        "change_id.short() ++ \" \" ++ description.first_line() ++ \"\n\"",
        "--no-graph",
    ]);
    let lines: Vec<&str> = log_output.stdout.normalized().lines().collect();
    let commit_c = lines[1].split_whitespace().next().unwrap();
    let commit_b = lines[2].split_whitespace().next().unwrap();
    let commit_a = lines[3].split_whitespace().next().unwrap();

    // Helper to run git in the workspace, returns (stdout, exit_code)
    let git_rev_parse_head = || {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(ws_dir.root())
            .output()
            .unwrap();
        (
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
            output.status.code().unwrap_or(-1),
        )
    };
    let git_status_short = || {
        std::process::Command::new("git")
            .args(["status", "--short"])
            .current_dir(ws_dir.root())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };

    // Edit to C — HEAD should point to B (C's parent)
    ws_dir.run_jj(["edit", commit_c]).success();
    let (head, code) = git_rev_parse_head();
    let status = git_status_short();
    assert_eq!(code, 0, "HEAD should be valid after edit to C, got: {head}");
    assert!(
        status.contains("f3.txt"),
        "git status should show f3.txt, got: {status}"
    );

    // Edit to B — HEAD should point to A (B's parent)
    ws_dir.run_jj(["edit", commit_b]).success();
    let (head, code) = git_rev_parse_head();
    let status = git_status_short();
    assert_eq!(code, 0, "HEAD should be valid after edit to B, got: {head}");
    assert!(
        status.contains("f2.txt"),
        "git status should show f2.txt, got: {status}"
    );

    // Edit to A — HEAD should be unborn (A's parent is root)
    ws_dir.run_jj(["edit", commit_a]).success();
    let (head, code) = git_rev_parse_head();
    let status = git_status_short();
    assert_ne!(
        code, 0,
        "HEAD should be unborn after edit to A (root parent), got: {head}"
    );
    assert!(
        status.contains("f1.txt"),
        "git status should show f1.txt, got: {status}"
    );
}

/// Rewriting a linked workspace from another workspace must not make the
/// linked worktree's stale Git HEAD look like an external `git checkout`.
#[test_case(false, false; "same working-copy tree")]
#[test_case(true, false; "manual stale recovery")]
#[test_case(true, true; "automatic stale recovery")]
fn test_linked_workspace_rebase_from_other_workspace_preserves_rewrite(
    destination_changes_tree: bool,
    auto_update_stale: bool,
) -> TestResult {
    let test_env = TestEnvironment::default();
    if auto_update_stale {
        test_env.add_config("snapshot.auto-update-stale = true\n");
    }
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    let workspace_a = test_env.work_dir("A");
    let workspace_b = test_env.work_dir("B");

    // Ensure `workspace add` can create proper linked Git worktrees.
    main_dir.write_file("base", "base\n");
    main_dir.run_jj(["commit", "-m", "base"]).success();
    main_dir
        .run_jj(["workspace", "add", "--name", "A", "../A"])
        .success();
    main_dir
        .run_jj(["workspace", "add", "--name", "B", "../B"])
        .success();

    if destination_changes_tree {
        workspace_a.write_file("a", "a\n");
    }
    workspace_a.run_jj(["commit", "-m", "A target"]).success();
    workspace_b.write_file("b", "b\n");
    workspace_b.run_jj(["commit", "-m", "B commit"]).success();

    let commit_id = |work_dir: &TestWorkDir, revision: &str| {
        work_dir
            .run_jj([
                "log",
                "--ignore-working-copy",
                "--no-graph",
                "-r",
                revision,
                "-T",
                "commit_id",
            ])
            .success()
            .stdout
            .normalized()
            .trim()
            .to_owned()
    };
    let default_wc_id = commit_id(&workspace_a, "default@");
    let old_b_parent_id = commit_id(&workspace_a, "B@-");

    workspace_a.run_jj(["rebase", "-b", "B@", "-o", "@"]).success();
    let rebased_b_parent_id = commit_id(&workspace_a, "B@-");
    assert_ne!(old_b_parent_id, rebased_b_parent_id);
    assert_eq!(commit_id(&workspace_a, "default@"), default_wc_id);

    // A normal command must not import B's old per-worktree Git HEAD. If the
    // destination changed the tree, the command should report a stale working
    // copy; otherwise it can synchronize the metadata automatically.
    let output = workspace_b.run_jj(["status"]);
    assert!(
        !output
            .stderr
            .normalized()
            .contains("Reset the working copy parent to the new Git HEAD"),
        "stale Git HEAD should not be imported: {output}"
    );
    if destination_changes_tree && !auto_update_stale {
        assert!(!output.status.success(), "command should fail: {output}");
        assert!(
            output.stderr.normalized().contains("working copy is stale"),
            "unexpected error: {output}"
        );
        workspace_b.run_jj(["workspace", "update-stale"]).success();
    } else {
        output.success();
    }
    assert_eq!(commit_id(&workspace_b, "@-"), rebased_b_parent_id);
    let divergent = workspace_b
        .run_jj([
            "log",
            "--ignore-working-copy",
            "--no-graph",
            "-r",
            "divergent()",
            "-T",
            "commit_id ++ \"\\n\"",
        ])
        .success();
    assert!(
        divergent.stdout.is_empty(),
        "rewrite should not become divergent: {divergent}"
    );

    let git_head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace_b.root())
        .output()?;
    assert!(git_head.status.success());
    assert_eq!(
        String::from_utf8(git_head.stdout)?.trim(),
        rebased_b_parent_id
    );

    Ok(())
}

/// Rewriting the parent of a linked workspace's working-copy commit must also
/// refresh that worktree's private Git index. This is easy to miss because the
/// worktree HEAD can be correct while `git status` still compares against an
/// index left behind by an earlier checkout.
#[test]
fn test_workspace_restore_into_then_new_syncs_git_index() {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");

    main_dir.write_file("file", "base\n");
    main_dir.run_jj(["commit", "-m", "base"]).success();
    main_dir
        .run_jj(["workspace", "add", "--name", "ws", "../ws"])
        .success();
    let ws_dir = test_env.work_dir("ws");

    ws_dir.write_file("file", "updated\n");
    ws_dir.run_jj(["restore", "--into", "@-"]).success();
    ws_dir.run_jj(["new"]).success();

    let head_tree = std::process::Command::new("git")
        .args(["rev-parse", "HEAD^{tree}"])
        .current_dir(ws_dir.root())
        .output()
        .unwrap();
    assert!(head_tree.status.success());
    let index_tree = std::process::Command::new("git")
        .args(["write-tree"])
        .current_dir(ws_dir.root())
        .output()
        .unwrap();
    assert!(index_tree.status.success());
    assert_eq!(
        String::from_utf8_lossy(&index_tree.stdout).trim(),
        String::from_utf8_lossy(&head_tree.stdout).trim(),
        "linked worktree index should match HEAD after restore --into and new"
    );

    let status = std::process::Command::new("git")
        .args(["status", "--porcelain=v1"])
        .current_dir(ws_dir.root())
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(String::from_utf8_lossy(&status.stdout), "");
}

#[test]
fn test_linked_workspace_locked_index_does_not_move_git_head() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    main_dir.write_file("file", "base\n");
    main_dir.run_jj(["commit", "-m", "base"]).success();
    main_dir
        .run_jj(["workspace", "add", "--name", "ws", "../ws"])
        .success();
    let ws_dir = test_env.work_dir("ws");

    // Create a target whose tree differs from the linked workspace's current
    // parent, so switching to it must update the Git index.
    main_dir.write_file("file", "target\n");
    main_dir.run_jj(["commit", "-m", "target"]).success();
    main_dir
        .run_jj(["bookmark", "create", "-r", "@-", "target"])
        .success();

    let old_head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(ws_dir.root())
        .output()?;
    let index_lock = std::process::Command::new("git")
        .args(["rev-parse", "--git-path", "index.lock"])
        .current_dir(ws_dir.root())
        .output()?;
    let index_lock = String::from_utf8(index_lock.stdout)?.trim().to_owned();
    std::fs::write(&index_lock, [])?;

    let output = ws_dir.run_jj(["new", "-r", "target"]);
    assert!(!output.status.success(), "command should fail: {output}");
    assert!(
        output
            .stderr
            .normalized()
            .contains("Failed to update the linked Git worktree index: fatal: Unable to create"),
        "unexpected error: {output}"
    );

    let new_head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(ws_dir.root())
        .output()?;
    assert_eq!(new_head.stdout, old_head.stdout, "Git HEAD must not move");

    std::fs::remove_file(index_lock)?;
    Ok(())
}

/// Metadata-only working-copy changes must not refresh every tracked file in
/// the linked worktree index. In particular, `new` and abandoning that empty
/// commit have identical trees and should not invoke clean filters.
#[test]
fn test_linked_workspace_empty_new_and_abandon_skip_clean_filter() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    main_dir.write_file("tracked.filter", "contents\n");
    main_dir.write_file(".gitattributes", "*.filter filter=fail\n");
    main_dir.run_jj(["commit", "-m", "base"]).success();
    main_dir
        .run_jj(["workspace", "add", "--name", "ws", "../ws"])
        .success();
    let ws_dir = test_env.work_dir("ws");

    // If `git add -u` runs, this clean filter changes the index blob. The
    // linked-worktree index sync uses `git read-tree`, which doesn't invoke
    // clean filters, so the index must remain identical to HEAD.
    let run_git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(ws_dir.root())
            .output()
            .unwrap()
    };
    assert!(
        run_git(&["config", "filter.fail.clean", "printf filtered"])
            .status
            .success()
    );
    assert!(
        run_git(&["config", "filter.fail.required", "true"])
            .status
            .success()
    );

    ws_dir.run_jj(["new"]).success();
    assert!(run_git(&["diff", "--cached", "--quiet"]).status.success());
    ws_dir.run_jj(["abandon", "@"]).success();
    assert!(run_git(&["diff", "--cached", "--quiet"]).status.success());
    Ok(())
}

/// Refreshing the linked-worktree index after checkout must be limited to the
/// paths actually touched by that checkout. An unrestricted `git add -u`
/// would clean every entry whose stat data was reset by `git read-tree`.
#[test]
fn test_linked_workspace_checkout_refreshes_only_changed_paths() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    let clean_marker = test_env.env_root().join("unrelated-clean-ran");
    let clean_script = test_env.env_root().join("count-clean.sh");
    std::fs::write(
        &clean_script,
        format!(
            "count=0\nif test -f {0}; then count=$(cat {0}); fi\nexpr \"$count\" + 1 > {0}\ncat\n",
            clean_marker.display()
        ),
    )?;
    let clean_command = format!("sh {}", clean_script.display());
    let config_filter = std::process::Command::new("git")
        .args(["config", "filter.fail.clean", &clean_command])
        .current_dir(main_dir.root())
        .output()?;
    assert!(config_filter.status.success());
    let config_required = std::process::Command::new("git")
        .args(["config", "filter.fail.required", "true"])
        .current_dir(main_dir.root())
        .output()?;
    assert!(config_required.status.success());
    main_dir.write_file("tracked.filter", "contents\n");
    main_dir.write_file("changed.txt", "base\n");
    main_dir.write_file(".gitattributes", "*.filter filter=fail\n");
    main_dir.run_jj(["commit", "-m", "base"]).success();
    main_dir
        .run_jj(["workspace", "add", "--name", "ws", "../ws"])
        .success();
    let ws_dir = test_env.work_dir("ws");

    main_dir.write_file("changed.txt", "target\n");
    main_dir.run_jj(["commit", "-m", "target"]).success();
    main_dir
        .run_jj(["bookmark", "create", "-r", "@-", "target"])
        .success();

    let run_git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(ws_dir.root())
            .output()
            .unwrap()
    };
    // Establish clean file-state metadata. Move the tree-state mtime into the
    // future so the next snapshot deterministically considers tracked.filter
    // clean instead of re-reading it when the mtimes happen to be equal.
    ws_dir.run_jj(["status"]).success();
    assert!(
        clean_marker.exists(),
        "test setup should have invoked the clean filter"
    );
    let tracked_mtime = std::fs::metadata(ws_dir.root().join("tracked.filter"))?.modified()?;
    let tree_state = std::fs::File::options()
        .write(true)
        .open(ws_dir.root().join(".jj/working_copy/tree_state"))?;
    tree_state.set_modified(tracked_mtime + std::time::Duration::from_secs(10))?;
    std::fs::remove_file(&clean_marker)?;

    // If post-checkout `git add -u` is unrestricted, it will clean the
    // unchanged filtered path and recreate this marker.
    ws_dir.run_jj(["new", "-r", "target"]).success();
    assert_eq!(ws_dir.read_file("changed.txt"), "target\n");
    assert!(
        !clean_marker.exists(),
        "checkout must not clean an unchanged filtered path"
    );
    let cached_diff = run_git(&["diff", "--cached", "--name-status"]);
    assert!(
        cached_diff.status.success() && cached_diff.stdout.is_empty(),
        "unchanged filtered paths must not be cleaned into the index: {}",
        String::from_utf8_lossy(&cached_diff.stdout)
    );
    Ok(())
}
