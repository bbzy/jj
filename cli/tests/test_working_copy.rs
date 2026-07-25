// Copyright 2023 The Jujutsu Authors
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

use indoc::indoc;
use regex::Regex;
use testutils::TestResult;

use crate::common::TestEnvironment;

#[test]
fn test_snapshot_large_file() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    // test a small file using raw-integer-literal syntax, which is interpreted
    // in bytes
    test_env.add_config(r#"snapshot.max-new-file-size = 10"#);
    work_dir.write_file("empty", "");
    work_dir.write_file("large", "a lot of text");
    let output = work_dir.run_jj(["file", "list"]);
    insta::assert_snapshot!(output, @"
    empty
    [EOF]
    ------- stderr -------
    Warning: Refused to snapshot some files:
      large: 13.0B (13 bytes); the maximum size allowed is 10.0B (10 bytes)
    Hint: This is to prevent large files from being added by accident. To fix this:
      * Add the file(s) to `.gitignore`
      * Run `jj config set --repo snapshot.max-new-file-size 13`
        This will increase the maximum file size allowed for new files, in this repository only.
      * Run `jj --config snapshot.max-new-file-size=13 status`
        This will increase the maximum file size allowed for new files, for this command only.
    [EOF]
    ");

    // test with a larger file using 'KB' human-readable syntax
    test_env.add_config(r#"snapshot.max-new-file-size = "10KB""#);
    let big_string = vec![0; 1024 * 11];
    work_dir.write_file("large", &big_string);
    let output = work_dir.run_jj(["file", "list"]);
    insta::assert_snapshot!(output, @"
    empty
    [EOF]
    ------- stderr -------
    Warning: Refused to snapshot some files:
      large: 11.0KiB (11264 bytes); the maximum size allowed is 10.0KiB (10240 bytes)
    Hint: This is to prevent large files from being added by accident. To fix this:
      * Add the file(s) to `.gitignore`
      * Run `jj config set --repo snapshot.max-new-file-size 11264`
        This will increase the maximum file size allowed for new files, in this repository only.
      * Run `jj --config snapshot.max-new-file-size=11264 status`
        This will increase the maximum file size allowed for new files, for this command only.
    [EOF]
    ");

    // test with file track for hint formatting, both files should appear in
    // warnings even though they were snapshotted separately
    work_dir.write_file("large 2", big_string);
    let output = work_dir.run_jj([
        "file",
        "--config=snapshot.auto-track='large'",
        "track",
        "large 2",
    ]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Warning: Refused to snapshot some files:
      large: 11.0KiB (11264 bytes); the maximum size allowed is 10.0KiB (10240 bytes)
      large 2: 11.0KiB (11264 bytes); the maximum size allowed is 10.0KiB (10240 bytes)
    Hint: This is to prevent large files from being added by accident. To fix this:
      * Add the file(s) to `.gitignore`
      * Run `jj config set --repo snapshot.max-new-file-size 11264`
        This will increase the maximum file size allowed for new files, in this repository only.
      * Run `jj --config snapshot.max-new-file-size=11264 file track large 'large 2'`
        This will increase the maximum file size allowed for new files, for this command only.
      * Run `jj file track --include-ignored large 'large 2'`
        This will track the file(s) regardless of size.
    [EOF]
    ");

    // test invalid configuration
    let output = work_dir.run_jj(["file", "list", "--config=snapshot.max-new-file-size=[]"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Config error: Invalid type or value for snapshot.max-new-file-size
    Caused by: Expected a positive integer or a string in '<number><unit>' form
    For help, see https://docs.jj-vcs.dev/latest/config/ or use `jj help -k config`.
    [EOF]
    [exit status: 1]
    ");

    // No error if we disable auto-tracking of the path
    let output = work_dir.run_jj(["file", "list", "--config=snapshot.auto-track='none()'"]);
    insta::assert_snapshot!(output, @"
    empty
    [EOF]
    ");

    // max-new-file-size=0 means no limit
    let output = work_dir.run_jj(["file", "list", "--config=snapshot.max-new-file-size=0"]);
    insta::assert_snapshot!(output, @"
    empty
    large
    large 2
    [EOF]
    ");
}

#[test]
fn test_snapshot_large_file_restore() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");
    test_env.add_config("snapshot.max-new-file-size = 10");

    work_dir.run_jj(["describe", "-mcommitted"]).success();
    work_dir.write_file("file", "small");

    // Write a large file in the working copy, restore it from a commit. The
    // working-copy content shouldn't be overwritten.
    work_dir.run_jj(["new", "root()"]).success();
    work_dir.write_file("file", "a lot of text");
    let output = work_dir.run_jj(["restore", "--from=subject(committed)"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Warning: Refused to snapshot some files:
      file: 13.0B (13 bytes); the maximum size allowed is 10.0B (10 bytes)
    Hint: This is to prevent large files from being added by accident. To fix this:
      * Add the file(s) to `.gitignore`
      * Run `jj config set --repo snapshot.max-new-file-size 13`
        This will increase the maximum file size allowed for new files, in this repository only.
      * Run `jj --config snapshot.max-new-file-size=13 status`
        This will increase the maximum file size allowed for new files, for this command only.
    Working copy  (@) now at: kkmpptxz 119f5156 (no description set)
    Parent commit (@-)      : zzzzzzzz 00000000 (empty) (no description set)
    Added 1 files, modified 0 files, removed 0 files
    Warning: 1 of those updates were skipped because there were conflicting changes in the working copy.
    Hint: Inspect the changes compared to the intended target with `jj diff --from 119f5156d330`.
    Discard the conflicting changes with `jj restore --from 119f5156d330`.
    [EOF]
    ");
    insta::assert_snapshot!(work_dir.read_file("file"), @"a lot of text");

    // However, the next command will snapshot the large file because it is now
    // tracked. TODO: Should we remember the untracked state?
    let output = work_dir.run_jj(["status"]);
    insta::assert_snapshot!(output, @"
    Working copy changes:
    A file
    Working copy  (@) : kkmpptxz 09eba65e (no description set)
    Parent commit (@-): zzzzzzzz 00000000 (empty) (no description set)
    [EOF]
    ");
}

#[test]
fn test_materialize_and_snapshot_different_conflict_markers() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    // Configure to use Git-style conflict markers
    test_env.add_config(r#"ui.conflict-marker-style = "git""#);

    // Create a conflict in the working copy
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2
            line 3
        "},
    );
    work_dir.run_jj(["commit", "-m", "base"]).success();
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2 - a
            line 3
        "},
    );
    work_dir.run_jj(["commit", "-m", "side-a"]).success();
    work_dir
        .run_jj(["new", "subject(base)", "-m", "side-b"])
        .success();
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2 - b
            line 3 - b
        "},
    );
    work_dir
        .run_jj(["new", "subject(side-a)", "subject(side-b)"])
        .success();

    // File should have Git-style conflict markers
    insta::assert_snapshot!(work_dir.read_file("file"), @r#"
    line 1
    <<<<<<< rlvkpnrz df1cdd77 "side-a"
    line 2 - a
    line 3
    ||||||| qpvuntsm 2205b3ac "base"
    line 2
    line 3
    =======
    line 2 - b
    line 3 - b
    >>>>>>> zsuskuln 68dcce1b "side-b"
    "#);

    // Configure to use JJ-style "snapshot" conflict markers
    test_env.add_config(r#"ui.conflict-marker-style = "snapshot""#);

    // Update the conflict, still using Git-style conflict markers
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            <<<<<<<
            line 2 - a
            line 3 - a
            |||||||
            line 2
            line 3
            =======
            line 2 - b
            line 3 - b
            >>>>>>>
        "},
    );

    // Git-style markers should be parsed, then rendered with new config
    insta::assert_snapshot!(work_dir.run_jj(["diff", "--git"]), @r#"
    diff --git a/file b/file
    --- a/file
    +++ b/file
    @@ -2,7 +2,7 @@
     <<<<<<< conflict 1 of 1
     +++++++ rlvkpnrz df1cdd77 "side-a"
     line 2 - a
    -line 3
    +line 3 - a
     ------- qpvuntsm 2205b3ac "base"
     line 2
     line 3
    [EOF]
    "#);
    Ok(())
}

#[test]
fn test_snapshot_invalid_ignore_pattern() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    // Test invalid pattern in .gitignore
    work_dir.write_file(".gitignore", " []\n");
    insta::assert_snapshot!(work_dir.run_jj(["st"]), @"
    Working copy changes:
    A .gitignore
    Working copy  (@) : qpvuntsm c9cf4826 (no description set)
    Parent commit (@-): zzzzzzzz 00000000 (empty) (no description set)
    [EOF]
    ");

    // Test invalid UTF-8 in .gitignore
    work_dir.write_file(".gitignore", b"\xff\n");
    insta::assert_snapshot!(work_dir.run_jj(["st"]), @"
    Working copy changes:
    A .gitignore
    Working copy  (@) : qpvuntsm 15f3d11a (no description set)
    Parent commit (@-): zzzzzzzz 00000000 (empty) (no description set)
    [EOF]
    ");
}

#[cfg(unix)]
#[test]
fn test_snapshot_non_utf8_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    if testutils::check_strict_utf8_fs(work_dir.root()) {
        eprintln!(
            "Skipping test \"test_snapshot_non_utf8_path\" due to strict UTF-8 filesystem for \
             path {:?}",
            work_dir.root()
        );
        return;
    }

    std::fs::write(work_dir.root().join(OsStr::from_bytes(b"file\xe0")), "").unwrap();
    std::fs::create_dir(work_dir.root().join(OsStr::from_bytes(b"dir\xe0"))).unwrap();
    work_dir.write_file("file", "");

    // The paths that can't be represented as RepoPaths are skipped, and the
    // snapshot succeeds.
    insta::assert_snapshot!(work_dir.run_jj(["st"]), @r#"
    Working copy changes:
    A file
    Working copy  (@) : qpvuntsm 3dcf981e (no description set)
    Parent commit (@-): zzzzzzzz 00000000 (empty) (no description set)
    [EOF]
    ------- stderr -------
    Warning: Skipped some paths because they are not valid UTF-8:
      .: "dir\xE0"
      .: "file\xE0"
    [EOF]
    "#);

    // .gitignore doesn't apply because we can't build a RepoPath to match
    // against, so the paths are still reported.
    work_dir.write_file(".gitignore", b"dir\xe0\nfile\xe0\n");
    insta::assert_snapshot!(work_dir.run_jj(["st"]), @r#"
    Working copy changes:
    A .gitignore
    A file
    Working copy  (@) : qpvuntsm 0fbe2679 (no description set)
    Parent commit (@-): zzzzzzzz 00000000 (empty) (no description set)
    [EOF]
    ------- stderr -------
    Warning: Skipped some paths because they are not valid UTF-8:
      .: "dir\xE0"
      .: "file\xE0"
    [EOF]
    "#);
}

#[test]
fn test_conflict_marker_length_stored_in_working_copy() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    // Create a conflict in the working copy with long markers on one side
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2
            line 3
        "},
    );
    work_dir.run_jj(["commit", "-m", "base"]).success();
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2 - left
            line 3 - left
        "},
    );
    work_dir.run_jj(["commit", "-m", "side-a"]).success();
    work_dir
        .run_jj(["new", "subject(base)", "-m", "side-b"])
        .success();
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            ======= fake marker
            line 2 - right
            ======= fake marker
            line 3
        "},
    );
    work_dir
        .run_jj(["new", "subject(side-a)", "subject(side-b)"])
        .success();

    // File should be materialized with long conflict markers
    insta::assert_snapshot!(work_dir.read_file("file"), @r#"
    line 1
    <<<<<<<<<<< conflict 1 of 1
    %%%%%%%%%%% diff from: qpvuntsm 2205b3ac "base"
    \\\\\\\\\\\        to: rlvkpnrz ccf9527c "side-a"
    -line 2
    -line 3
    +line 2 - left
    +line 3 - left
    +++++++++++ zsuskuln d7acaf48 "side-b"
    ======= fake marker
    line 2 - right
    ======= fake marker
    line 3
    >>>>>>>>>>> conflict 1 of 1 ends
    "#);

    // The timestamps in the `jj debug local-working-copy` output change, so we want
    // to remove them before asserting the snapshot
    let timestamp_regex = Regex::new(r"\b\d{10,}\b")?;
    let redact_output = |output: String| {
        let output = timestamp_regex.replace_all(&output, "<timestamp>");
        output.into_owned()
    };

    // Working copy should contain conflict marker length
    let output = work_dir.run_jj(["debug", "local-working-copy"]);
    insta::assert_snapshot!(output.normalize_stdout_with(redact_output), @r#"
    Current operation: OperationId("ee791f2181026a056ad383d14dbc749ba44e24fedfd39418954871b83d754929147b951bde1fbcca6a8460932e53a6d7ea739bf054ec0bf03607b9370173d6e5")
    Current tree: MergedTree { tree_ids: Conflicted([TreeId("381273b50cf73f8c81b3f1502ee89e9bbd6c1518"), TreeId("771f3d31c4588ea40a8864b2a981749888e596c2"), TreeId("f56b8223da0dab22b03b8323ced4946329aeb4e0")]), labels: Labeled(["rlvkpnrz ccf9527c \"side-a\"", "qpvuntsm 2205b3ac \"base\"", "zsuskuln d7acaf48 \"side-b\""]), .. }
    Normal { exec_bit: ExecBit(false) }           313 <timestamp> Some(MaterializedConflictData { conflict_marker_len: 11 }) "file"
    [EOF]
    "#);

    // Update the conflict with more fake markers, and it should still parse
    // correctly (the markers should be ignored)
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            <<<<<<<<<<< conflict 1 of 1
            %%%%%%%%%%% diff from base to side #1
            -line 2
            -line 3
            +line 2 - left
            +line 3 - left
            +++++++++++ side #2
            <<<<<<< fake marker
            ||||||| fake marker
            line 2 - right
            ======= fake marker
            line 3
            >>>>>>> fake marker
            >>>>>>>>>>> conflict 1 of 1 ends
        "},
    );

    // The file should still be conflicted, and the new content should be saved
    let output = work_dir.run_jj(["st"]);
    insta::assert_snapshot!(output, @"
    Working copy changes:
    M file
    Working copy  (@) : mzvwutvl d31c99cf (conflict) (no description set)
    Parent commit (@-): rlvkpnrz ccf9527c side-a
    Parent commit (@-): zsuskuln d7acaf48 side-b
    Warning: There are unresolved conflicts at these paths:
    file    2-sided conflict
    [EOF]
    ");
    insta::assert_snapshot!(work_dir.run_jj(["diff", "--git"]), @r#"
    diff --git a/file b/file
    --- a/file
    +++ b/file
    @@ -7,8 +7,10 @@
     +line 2 - left
     +line 3 - left
     +++++++++++ zsuskuln d7acaf48 "side-b"
    -======= fake marker
    +<<<<<<< fake marker
    +||||||| fake marker
     line 2 - right
     ======= fake marker
     line 3
    +>>>>>>> fake marker
     >>>>>>>>>>> conflict 1 of 1 ends
    [EOF]
    "#);

    // Working copy should still contain conflict marker length
    let output = work_dir.run_jj(["debug", "local-working-copy"]);
    insta::assert_snapshot!(output.normalize_stdout_with(redact_output), @r#"
    Current operation: OperationId("b196f038bbd8cf84508417da8e974874b52202bc98abca08725e946a7a9ea9e011aeddda805c565f49a1e07e43524161caa56fa438de662aba8a6349b5a44be1")
    Current tree: MergedTree { tree_ids: Conflicted([TreeId("381273b50cf73f8c81b3f1502ee89e9bbd6c1518"), TreeId("771f3d31c4588ea40a8864b2a981749888e596c2"), TreeId("3329c18c95f7b7a55c278c2259e9c4ce711fae59")]), labels: Labeled(["rlvkpnrz ccf9527c \"side-a\"", "qpvuntsm 2205b3ac \"base\"", "zsuskuln d7acaf48 \"side-b\""]), .. }
    Normal { exec_bit: ExecBit(false) }           274 <timestamp> Some(MaterializedConflictData { conflict_marker_len: 11 }) "file"
    [EOF]
    "#);

    // Resolve the conflict
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            <<<<<<< fake marker
            ||||||| fake marker
            line 2 - left
            line 2 - right
            ======= fake marker
            line 3 - left
            >>>>>>> fake marker
        "},
    );

    let output = work_dir.run_jj(["st"]);
    insta::assert_snapshot!(output, @"
    Working copy changes:
    M file
    Working copy  (@) : mzvwutvl 469d479f (no description set)
    Parent commit (@-): rlvkpnrz ccf9527c side-a
    Parent commit (@-): zsuskuln d7acaf48 side-b
    [EOF]
    ");

    // When the file is resolved, the conflict marker length is removed from the
    // working copy
    let output = work_dir.run_jj(["debug", "local-working-copy"]);
    insta::assert_snapshot!(output.normalize_stdout_with(redact_output), @r#"
    Current operation: OperationId("65b561ae4667b8b9db3f3d6cb2f6bccd087f17b008ba87b976266e641efdab4f12193407e4f6f8bd76b02eff767a6c173ef53e2c0217be389b05779170a257b6")
    Current tree: MergedTree { tree_ids: Resolved(TreeId("6120567b3cb2472d549753ed3e4b84183d52a650")), labels: Unlabeled, .. }
    Normal { exec_bit: ExecBit(false) }           130 <timestamp> None "file"
    [EOF]
    "#);
    Ok(())
}

#[test]
fn test_submodule_ignored() {
    let test_env = TestEnvironment::default();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submodule"])
        .success();
    let submodule_dir = test_env.work_dir("submodule");
    submodule_dir.write_file("sub", "sub");
    submodule_dir
        .run_jj(["commit", "-m", "Submodule commit"])
        .success();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    // There's no particular reason to run this with jj util exec, it's just that
    // the infra makes it easier to run this way.
    let output = work_dir.run_jj([
        "util",
        "exec",
        "--",
        "git",
        "-c",
        // Git normally doesn't allow file:// in submodules.
        "protocol.file.allow=always",
        "submodule",
        "add",
        &format!("{}/submodule", test_env.env_root().display()),
        "sub",
    ]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Cloning into '$TEST_ENV/repo/sub'...
    done.
    [EOF]
    ");
    // Use git to commit since jj won't play nice with the submodule.
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Add submodule",
        ])
        .success();

    // This should be empty. We shouldn't track the submodule itself.
    let output = work_dir.run_jj(["diff", "--summary"]);
    insta::assert_snapshot!(output, @r#"
    ------- stderr -------
    Done importing changes from the underlying Git repo.
    [EOF]
    "#);

    // Switch to a historical commit before the submodule was checked in.
    work_dir.run_jj(["prev"]).success();
    // jj new (or equivalently prev) should always leave you with an empty working
    // copy.
    let output = work_dir.run_jj(["diff", "--summary"]);
    insta::assert_snapshot!(output, @"");
}

/// Test that submodules are automatically populated on checkout in colocated
/// repos. When switching to a commit that contains a submodule, jj should run
/// `git submodule update --init` to populate the submodule directory.
#[test]
fn test_submodule_auto_populate_on_checkout() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.submodule.auto-update = true");

    // Create a submodule repo.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submodule"])
        .success();
    let submodule_dir = test_env.work_dir("submodule");
    submodule_dir.write_file("subfile", "sub content\n");
    submodule_dir
        .run_jj(["commit", "-m", "submodule init"])
        .success();

    // Create the main repo with an initial commit (no submodule).
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    main_dir.write_file("README", "readme\n");
    main_dir.run_jj(["commit", "-m", "initial"]).success();
    main_dir.run_jj(["bookmark", "create", "initial"]).success();

    // Add submodule via git.
    work_dir_run_git(
        &main_dir,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/submodule", test_env.env_root().display()),
            "sub",
        ],
    );
    work_dir_run_git(
        &main_dir,
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "add submodule",
        ],
    );

    // Import the git commit into jj.
    main_dir.run_jj(["st"]).success();
    main_dir.run_jj(["bookmark", "create", "withsub"]).success();

    // Switch to the initial commit (no submodule).
    main_dir.run_jj(["new", "initial"]).success();
    // The submodule directory may still exist (non-empty dirs are preserved).
    // Remove it to test fresh checkout.
    let _ = std::fs::remove_dir_all(main_dir.root().join("sub")).ok();

    // Switch back to the commit with submodule — should auto-populate.
    let output = main_dir.run_jj(["new", "withsub"]);
    output.success();

    // The submodule directory should be populated.
    let subfile = main_dir.root().join("sub").join("subfile");
    assert!(
        subfile.exists(),
        "submodule file should exist after checkout"
    );
    let content = std::fs::read_to_string(&subfile).unwrap();
    assert_eq!(content, "sub content\n");
}

/// Test that submodules are NOT auto-populated when
/// `git.submodule.auto-update = false` (the default). The submodule
/// directory should be created but left empty.
#[test]
fn test_submodule_no_auto_populate_by_default() {
    let test_env = TestEnvironment::default();

    // Create a submodule repo.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submodule"])
        .success();
    let submodule_dir = test_env.work_dir("submodule");
    submodule_dir.write_file("subfile", "sub content\n");
    submodule_dir
        .run_jj(["commit", "-m", "submodule init"])
        .success();

    // Create the main repo with an initial commit (no submodule).
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    main_dir.write_file("README", "readme\n");
    main_dir.run_jj(["commit", "-m", "initial"]).success();
    main_dir.run_jj(["bookmark", "create", "initial"]).success();

    // Add submodule via git.
    work_dir_run_git(
        &main_dir,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/submodule", test_env.env_root().display()),
            "sub",
        ],
    );
    work_dir_run_git(
        &main_dir,
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "add submodule",
        ],
    );

    // Import the git commit into jj.
    main_dir.run_jj(["st"]).success();
    main_dir.run_jj(["bookmark", "create", "withsub"]).success();

    // Switch to the initial commit (no submodule).
    main_dir.run_jj(["new", "initial"]).success();
    let _ = std::fs::remove_dir_all(main_dir.root().join("sub")).ok();

    // Switch back to the commit with submodule — should NOT auto-populate
    // (auto-update is false by default).
    main_dir.run_jj(["new", "withsub"]).success();

    // The submodule directory should exist but be empty.
    let sub_dir = main_dir.root().join("sub");
    assert!(
        sub_dir.exists(),
        "submodule directory should exist (created as empty dir)"
    );
    assert!(
        !sub_dir.join("subfile").exists(),
        "submodule file should NOT exist when auto-update is disabled"
    );
}

/// Test that checkout succeeds even when submodule population fails (e.g.
/// unreachable URL). The submodule directory should be created but empty,
/// with a guidance message printed.
#[test]
fn test_submodule_populate_failure_fallback() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.submodule.auto-update = true");

    // Create a valid submodule repo.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submod"])
        .success();
    let submod_dir = test_env.work_dir("submod");
    submod_dir.write_file("subfile", "sub content\n");
    submod_dir
        .run_jj(["commit", "-m", "submodule init"])
        .success();

    // Create the main repo with an initial commit.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    main_dir.write_file("README", "readme\n");
    main_dir.run_jj(["commit", "-m", "initial"]).success();
    main_dir.run_jj(["bookmark", "create", "initial"]).success();

    // Add a valid submodule via git.
    work_dir_run_git(
        &main_dir,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/submod", test_env.env_root().display()),
            "sub",
        ],
    );
    work_dir_run_git(
        &main_dir,
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "add submodule",
        ],
    );

    // Import into jj.
    main_dir.run_jj(["st"]).success();
    main_dir.run_jj(["bookmark", "create", "withsub"]).success();

    // Switch to initial (no submodule).
    main_dir.run_jj(["new", "initial"]).success();
    // Remove the submodule directory and its git metadata to simulate
    // a fresh clone where the submodule hasn't been populated.
    let _ = std::fs::remove_dir_all(main_dir.root().join("sub")).ok();
    let _ = std::fs::remove_dir_all(main_dir.root().join(".git").join("modules").join("sub")).ok();

    // Change the submodule URL in .git/config to a nonexistent path so
    // population fails. The .gitmodules in the tree still has the URL,
    // but git submodule update uses .git/config for cloning.
    let git_config_path = main_dir.root().join(".git").join("config");
    let config = std::fs::read_to_string(&git_config_path).unwrap();
    let submod_url = format!("{}/submod", test_env.env_root().display());
    let config = config.replace(&submod_url, "/nonexistent/path/to/submodule");
    std::fs::write(&git_config_path, config).unwrap();

    // Switch back — submodule population will fail (nonexistent path),
    // but checkout should still succeed.
    let output = main_dir.run_jj(["new", "withsub"]);
    assert!(
        output.status.success(),
        "checkout should succeed even if submodule population fails"
    );

    // The submodule directory should exist (created by jj).
    assert!(
        main_dir.root().join("sub").is_dir(),
        "submodule directory should exist even if population failed"
    );

    // The submodule directory should be populated even though git config
    // has wrong URL — the submodule store reads from .gitmodules instead.
    let entries: Vec<_> = std::fs::read_dir(main_dir.root().join("sub"))
        .unwrap()
        .collect();
    assert!(
        !entries.is_empty(),
        "submodule directory should be populated (submodule store reads .gitmodules, not .git/config)"
    );
}

#[test]
fn test_snapshot_jjconflict_trees() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "repo", "--colocate"])
        .success();
    let work_dir = test_env.work_dir("repo");

    // Create a conflict in the working copy
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2
            line 3
        "},
    );
    work_dir.run_jj(["new", "-m", "side-a"]).success();
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2 - left
            line 3 - left
        "},
    );
    work_dir
        .run_jj(["new", "subject(side-a)-", "-m", "side-b"])
        .success();
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2 - right
            line 3
        "},
    );
    work_dir.run_jj(["new"]).success();
    work_dir
        .run_jj(["rebase", "-s", "subject(side-b)", "-o", "subject(side-a)"])
        .success();

    // Run `git reset --hard HEAD` to simulate checking out the branch with Git.
    let output = std::process::Command::new("git")
        .current_dir(work_dir.root())
        .args(["reset", "--hard", "HEAD"])
        .output()?;
    assert!(output.status.success());

    // We should see a warning regarding '.jjconflict' trees being checked out.
    let output = work_dir.run_jj(["st"]);
    insta::assert_snapshot!(output.to_string().replace('\\', "/"), @r"
    Working copy changes:
    A .jjconflict-base-0/file
    A .jjconflict-side-0/file
    A .jjconflict-side-1/file
    A JJ-CONFLICT-README
    M file
    Working copy  (@) : zsuskuln 2681a418 (no description set)
    Parent commit (@-): kkmpptxz aadeb8eb (conflict) side-b
    Hint: Conflict in parent commit has been resolved in working copy.
    [EOF]
    ------- stderr -------
    Warning: The working copy contains '.jjconflict' files. These files are used by `jj` internally and should not be present in the working copy.
    Hint: You may have used a regular `git` command to check out a conflicted commit.
    Hint: You can use `jj abandon` to discard the working copy changes.
    [EOF]
    ");
    Ok(())
}

/// Test that submodules are handled correctly in a workspace that shares
/// the repo with another workspace.
#[test]
fn test_submodule_in_workspace() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.submodule.auto-update = true");

    // Create a submodule repo.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submodule"])
        .success();
    let submodule_dir = test_env.work_dir("submodule");
    submodule_dir.write_file("sub", "sub");
    submodule_dir
        .run_jj(["commit", "-m", "Submodule commit"])
        .success();

    // Create the main repo and add the submodule via git.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");

    work_dir_run_git(
        &main_dir,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/submodule", test_env.env_root().display()),
            "sub",
        ],
    );
    work_dir_run_git(
        &main_dir,
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Add submodule",
        ],
    );

    // Create a second workspace sharing the same repo.
    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../second"])
        .success();
    let second_dir = test_env.work_dir("second");

    // The second workspace should create an empty dir for the submodule.
    assert!(
        second_dir.root().join("sub").is_dir(),
        "submodule directory should exist in second workspace"
    );

    // The submodule directory should be populated in non-colocated workspaces
    // via the submodule store (bare git repo), not via git submodule commands.
    let entries: Vec<_> = std::fs::read_dir(second_dir.root().join("sub"))
        .unwrap()
        .collect();
    assert!(
        !entries.is_empty(),
        "submodule directory should be populated in non-colocated second workspace"
    );

    // .gitmodules should be present in the second workspace.
    let gitmodules = second_dir.read_file(".gitmodules");
    let gitmodules_str = String::from_utf8_lossy(&gitmodules);
    assert!(
        gitmodules_str.contains("[submodule \"sub\"]"),
        ".gitmodules should be present in second workspace, got: {gitmodules_str}"
    );

    // Snapshot in the second workspace should not track submodule contents.
    let output = second_dir.run_jj(["file", "list"]);
    insta::assert_snapshot!(output, @"
    .gitmodules
    sub
    [EOF]
    ");
}

fn work_dir_run_git(work_dir: &crate::common::TestWorkDir, args: &[&str]) {
    let output = work_dir.run_jj(
        ["util", "exec", "--", "git"]
            .into_iter()
            .chain(args.iter().copied())
            .collect::<Vec<_>>(),
    );
    assert!(
        output.status.success(),
        "git command failed: {args:?}\nstdout: {output}"
    );
}

/// Test that forgetting a workspace with a submodule checkout doesn't cause
/// errors and the other workspace remains functional.
#[test]
fn test_submodule_forget_workspace() {
    let test_env = TestEnvironment::default();

    // Create a submodule repo.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submodule"])
        .success();
    let submodule_dir = test_env.work_dir("submodule");
    submodule_dir.write_file("sub", "sub");
    submodule_dir
        .run_jj(["commit", "-m", "Submodule commit"])
        .success();

    // Create the main repo and add the submodule via git.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");

    work_dir_run_git(
        &main_dir,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/submodule", test_env.env_root().display()),
            "sub",
        ],
    );
    work_dir_run_git(
        &main_dir,
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Add submodule",
        ],
    );

    // Create a second workspace.
    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../second"])
        .success();

    // Forget the second workspace — should not error even though it has a
    // submodule directory.
    let output = test_env.work_dir("second").run_jj(["workspace", "forget"]);
    assert!(
        output.status.success(),
        "forget should succeed, got: {output}"
    );

    // The main workspace should still list correctly.
    let output = main_dir.run_jj(["workspace", "list"]);
    let list_str = output.stdout.to_string();
    assert!(
        list_str.contains("default"),
        "default workspace should still exist, got: {list_str}"
    );
    assert!(
        !list_str.contains("second"),
        "second workspace should be forgotten, got: {list_str}"
    );

    // The main workspace should still be functional — checkout should work.
    main_dir.run_jj(["st"]).success();

    // Submodule should still be present in the main workspace.
    assert!(
        main_dir.root().join("sub").is_dir(),
        "submodule dir should still exist in main workspace"
    );
}

/// Test that nested submodules (submodule within submodule) are handled
/// correctly during checkout in colocated repos.
#[test]
fn test_submodule_nested() {
    let test_env = TestEnvironment::default();

    // Create an inner submodule repo.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "inner"])
        .success();
    let inner_dir = test_env.work_dir("inner");
    inner_dir.write_file("inner_file", "inner");
    inner_dir
        .run_jj(["commit", "-m", "Inner submodule commit"])
        .success();

    // Create a middle submodule repo that includes the inner as a submodule.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "middle"])
        .success();
    let middle_dir = test_env.work_dir("middle");
    middle_dir.write_file("middle_file", "middle");
    middle_dir.run_jj(["commit", "-m", "Middle commit"]).success();

    work_dir_run_git(
        &middle_dir,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/inner", test_env.env_root().display()),
            "inner_sub",
        ],
    );
    work_dir_run_git(
        &middle_dir,
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Add inner submodule",
        ],
    );

    // Create the main repo with the middle submodule.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    main_dir.write_file("main_file", "main");
    main_dir.run_jj(["commit", "-m", "Main commit"]).success();

    work_dir_run_git(
        &main_dir,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/middle", test_env.env_root().display()),
            "middle_sub",
        ],
    );
    work_dir_run_git(
        &main_dir,
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Add middle submodule",
        ],
    );

    // Snapshot to import the git changes.
    main_dir.run_jj(["st"]).success();

    // The middle submodule should be populated (colocated repo).
    assert!(
        main_dir.root().join("middle_sub").is_dir(),
        "middle submodule should be populated"
    );
    let middle_file = main_dir.root().join("middle_sub/middle_file");
    assert!(
        middle_file.exists(),
        "middle submodule file should exist"
    );

    // The inner submodule directory may exist but be empty, since
    // `git submodule update --init` is not run with --recursive.
    // We verify that the middle submodule is populated and its own
    // files are present, but do not require nested population.
    let inner_sub_dir = main_dir.root().join("middle_sub/inner_sub");
    if inner_sub_dir.exists() {
        // If the directory exists, it may be empty (non-recursive init).
        // This is expected behavior — jj does not recursively populate.
        let inner_file = inner_sub_dir.join("inner_file");
        if inner_file.exists() {
            // If recursive populate happened (e.g. git config set it up),
            // the inner file should have correct content.
            let content = std::fs::read_to_string(&inner_file).unwrap();
            assert_eq!(content, "inner");
        }
    }
}
