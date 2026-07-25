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

use testutils::TestResult;

use crate::common::TestEnvironment;

/// Configures a filter driver in the colocated Git repo's config.
fn add_filter_config(
    work_dir: &crate::common::TestWorkDir,
    filter_name: &str,
    clean: &str,
    smudge: &str,
) {
    let config_path = work_dir.root().join(".git/config");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str("\n[filter \"");
    config.push_str(filter_name);
    config.push_str("\"]\n\tclean = ");
    config.push_str(clean);
    config.push_str("\n\tsmudge = ");
    config.push_str(smudge);
    config.push('\n');
    std::fs::write(&config_path, config).unwrap();
}

/// Configures a process-mode filter driver in the colocated Git repo's config.
fn add_process_filter_config(
    work_dir: &crate::common::TestWorkDir,
    filter_name: &str,
    process_cmd: &str,
) {
    let config_path = work_dir.root().join(".git/config");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str("\n[filter \"");
    config.push_str(filter_name);
    config.push_str("\"]\n\tprocess = ");
    config.push_str(process_cmd);
    config.push('\n');
    std::fs::write(&config_path, config).unwrap();
}

/// Writes a Python script that implements the Git filter process protocol
/// as a filter that replaces 'o' with '0' on clean and '0' with 'o' on smudge.
///
/// Returns the command string to put in git config (e.g. "python3 /path/script.py").
fn write_process_filter_script(work_dir: &crate::common::TestWorkDir) -> String {
    let python = std::env::var("PYTHON3").unwrap_or_else(|_| "python3".to_string());
    let script = r#"
import sys

def read_exact(n):
    data = b""
    while len(data) < n:
        chunk = sys.stdin.buffer.read(n - len(data))
        if not chunk:
            break
        data += chunk
    return data

def read_pkt_line():
    header = read_exact(4)
    if len(header) < 4:
        return None
    length = int(header, 16)
    if length == 0:
        return None
    data_len = length - 4
    if data_len > 0:
        return read_exact(data_len)
    return b""

def write_pkt_line(data):
    CHUNK = 65516
    for i in range(0, len(data), CHUNK):
        chunk = data[i:i+CHUNK]
        length = len(chunk) + 4
        sys.stdout.buffer.write(f"{length:04x}".encode() + chunk)
    sys.stdout.buffer.flush()

def write_flush():
    sys.stdout.buffer.write(b"0000")
    sys.stdout.buffer.flush()

# Handshake
read_pkt_line()  # git-filter-client
while True:
    line = read_pkt_line()
    if line is None:
        break
write_pkt_line(b"git-filter-server")
write_pkt_line(b"version=2")
write_flush()
while True:
    line = read_pkt_line()
    if line is None:
        break
write_pkt_line(b"capability=clean")
write_pkt_line(b"capability=smudge")
write_flush()

# Main loop: two-phase exchange
while True:
    command = read_pkt_line()
    if command is None:
        break
    cmd = command.decode().strip()
    while True:
        line = read_pkt_line()
        if line is None:
            break
    write_pkt_line(b"status=success")
    write_flush()

    content = b""
    while True:
        chunk = read_pkt_line()
        if chunk is None:
            break
        content += chunk

    if cmd == "command=clean":
        result = content.replace(b"o", b"0")
    elif cmd == "command=smudge":
        result = content.replace(b"0", b"o")
    else:
        result = content

    if result:
        write_pkt_line(result)
    write_flush()
    write_pkt_line(b"status=success")
    write_flush()
"#;
    let script_path = work_dir.root().join(".git").join("process_filter.py");
    std::fs::write(&script_path, script).unwrap();
    format!("{python} {}", script_path.display())
}

/// Test that a clean/smudge filter works end-to-end with jj CLI commands.
///
/// The filter uses `tr` to transform content: clean replaces 'o' with '0',
/// smudge replaces '0' with 'o'.
#[test]
fn test_filter_clean_smudge_roundtrip() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    add_filter_config(&work_dir, "testfilter", "tr o 0", "tr 0 o");

    work_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    work_dir.write_file("data.bin", "hello world from jj\n");

    // Snapshot — clean filter should run.
    work_dir.run_jj(["st"]).success();

    // The stored content should be the cleaned version (o->0).
    let output = work_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    hell0 w0rld fr0m jj
    [EOF]
    ");

    // Commit the file.
    work_dir
        .run_jj(["commit", "-m", "add filtered file"])
        .success();

    // Remove the file and snapshot.
    work_dir.remove_file("data.bin");
    work_dir.run_jj(["st"]).success();

    // Checkout the commit with the file — smudge filter should run.
    work_dir.run_jj(["new", "@-"]).success();

    // The disk content should be the smudged version (0->o).
    let disk_content = work_dir.read_file("data.bin");
    assert_eq!(
        disk_content, "hello world from jj\n",
        "smudge filter should have restored original content"
    );

    // Working copy should be clean.
    let output = work_dir.run_jj(["diff", "--summary"]);
    assert!(output.stdout.is_empty(), "working copy should be clean");

    Ok(())
}

/// Test that files without a configured filter driver are stored as-is.
#[test]
fn test_filter_no_driver_configured() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    // Write .gitattributes with filter=lfs but don't configure the driver.
    work_dir.write_file(".gitattributes", "*.bin filter=lfs\n");
    work_dir.write_file("data.bin", "hello world\n");

    work_dir.run_jj(["st"]).success();

    // Content should be stored as-is (no clean filter applied).
    let output = work_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    hello world
    [EOF]
    ");

    Ok(())
}

/// Test that non-matching files are not affected by the filter.
#[test]
fn test_filter_non_matching_file() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    add_filter_config(&work_dir, "testfilter", "tr o 0", "tr 0 o");

    // .gitattributes only matches *.bin, not *.txt.
    work_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    work_dir.write_file("data.txt", "hello world\n");

    work_dir.run_jj(["st"]).success();

    // .txt file should not be filtered.
    let output = work_dir.run_jj(["file", "show", "data.txt"]);
    insta::assert_snapshot!(output, @"
    hello world
    [EOF]
    ");

    Ok(())
}

/// Test that the filter works across `jj git clone`.
///
/// Note: Git config (filter drivers) is NOT cloned by `git clone` — this is
/// standard Git behavior. The filter must be configured separately in the
/// clone. This test verifies that after configuring the filter, the smudge
/// filter runs during checkout.
#[test]
fn test_filter_clone_roundtrip() -> TestResult {
    let test_env = TestEnvironment::default();

    // Create source repo with filter.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "source"])
        .success();
    let source_dir = test_env.work_dir("source");
    add_filter_config(&source_dir, "testfilter", "tr o 0", "tr 0 o");
    source_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    source_dir.write_file("data.bin", "hello world\n");
    source_dir
        .run_jj(["commit", "-m", "add filtered file"])
        .success();
    source_dir
        .run_jj(["bookmark", "set", "main", "-r", "@-"])
        .success();

    // Clone the repo.
    test_env
        .run_jj_in(
            ".",
            [
                "git",
                "clone",
                "--colocate",
                &format!("{}/source", test_env.env_root().display()),
                "clone",
            ],
        )
        .success();
    let clone_dir = test_env.work_dir("clone");

    // Git config is not cloned — configure the filter in the clone.
    add_filter_config(&clone_dir, "testfilter", "tr o 0", "tr 0 o");

    // The clone already has data.bin on disk (from the initial checkout
    // during clone, which happened before we configured the filter). Force
    // a re-checkout by creating a new commit on the parent (which has the
    // file), triggering smudge during checkout.
    clone_dir.remove_file("data.bin");
    assert!(
        !clone_dir.root().join("data.bin").exists(),
        "data.bin should be absent before re-checkout"
    );
    clone_dir.run_jj(["st"]).success();
    clone_dir
        .run_jj(["new", "--ignore-immutable", "@-"])
        .success();

    // The smudge filter should run during checkout.
    let disk_content = clone_dir.read_file("data.bin");
    assert_eq!(
        disk_content, "hello world\n",
        "smudge filter should have restored original content after clone"
    );

    // The stored content should still be the cleaned version.
    let output = clone_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    hell0 w0rld
    [EOF]
    ");

    Ok(())
}

/// Test that a filter using `%f` path substitution works.
#[test]
fn test_filter_path_substitution() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    // Use `cat %f` as clean — reads the file directly (equivalent to no-op
    // since it reads from the file path, not stdin).
    // Use `echo smudged` as smudge — writes fixed content.
    add_filter_config(&work_dir, "pathfilter", "cat %f", "echo smudged");

    work_dir.write_file(".gitattributes", "*.bin filter=pathfilter\n");
    work_dir.write_file("data.bin", "original content\n");

    work_dir.run_jj(["st"]).success();

    // Clean filter (`cat %f`) should store the original content.
    let output = work_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    original content
    [EOF]
    ");

    // Commit, remove, and checkout to test smudge.
    work_dir.run_jj(["commit", "-m", "add file"]).success();
    work_dir.remove_file("data.bin");
    work_dir.run_jj(["st"]).success();
    work_dir.run_jj(["new", "@-"]).success();

    // Smudge filter (`echo smudged`) should write "smudged\n".
    let disk_content = work_dir.read_file("data.bin");
    assert_eq!(disk_content, "smudged\n");

    Ok(())
}

/// Test that the filter works correctly when switching between commits.
#[test]
fn test_filter_checkout_switch() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    add_filter_config(&work_dir, "testfilter", "tr o 0", "tr 0 o");

    work_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");

    // Create commit A with data.bin.
    work_dir.write_file("data.bin", "hello from A\n");
    work_dir.run_jj(["describe", "-m", "commit A"]).success();
    work_dir.run_jj(["new"]).success();

    // Create commit B with different content.
    work_dir.write_file("data.bin", "hello from B\n");
    work_dir.run_jj(["describe", "-m", "commit B"]).success();

    // Switch back to A — smudge should restore "hello from A".
    work_dir.run_jj(["edit", "@-"]).success();
    let disk_content = work_dir.read_file("data.bin");
    assert_eq!(
        disk_content, "hello from A\n",
        "smudge should restore A's content when switching to A"
    );

    // Switch back to B — smudge should restore "hello from B".
    work_dir.run_jj(["edit", "@+"]).success();
    let disk_content = work_dir.read_file("data.bin");
    assert_eq!(
        disk_content, "hello from B\n",
        "smudge should restore B's content when switching to B"
    );

    Ok(())
}

/// Test that a failing clean filter (non-zero exit) causes a snapshot error.
#[test]
fn test_filter_clean_failure() {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    // `false` always exits with non-zero status.
    add_filter_config(&work_dir, "badfilter", "false", "cat");
    work_dir.write_file(".gitattributes", "*.bin filter=badfilter\n");
    work_dir.write_file("data.bin", "hello world\n");

    // Snapshot should fail.
    let output = work_dir.run_jj(["st"]);
    assert!(
        !output.status.success(),
        "jj st should fail when clean filter fails"
    );
    let stderr = output.stderr.to_string();
    assert!(
        stderr.contains("badfilter") || stderr.contains("false") || stderr.contains("filter"),
        "stderr should mention the filter or command, got: {stderr}"
    );
    assert!(
        !stderr.contains("Internal error"),
        "filter failure should not be classified as Internal error, got: {stderr}"
    );
}

/// Test that a clean/smudge filter works with a `.gitattributes` file in a
/// subdirectory, not just at the repo root.
#[test]
fn test_filter_nested_gitattributes() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    add_filter_config(&work_dir, "testfilter", "tr o 0", "tr 0 o");

    // Root .gitattributes does NOT match .bin files.
    work_dir.write_file(".gitattributes", "*.txt text\n");
    // Subdirectory .gitattributes matches *.bin.
    work_dir.write_file("subdir/.gitattributes", "*.bin filter=testfilter\n");

    // Root .bin file should NOT be filtered.
    work_dir.write_file("root_file.bin", "hello world\n");
    // Subdir .bin file SHOULD be filtered.
    work_dir.write_file("subdir/data.bin", "hello world\n");

    work_dir.run_jj(["st"]).success();

    // Root file: no filter applied.
    let output = work_dir.run_jj(["file", "show", "root_file.bin"]);
    insta::assert_snapshot!(output, @"
    hello world
    [EOF]
    ");

    // Subdir file: clean filter applied (o -> 0).
    let output = work_dir.run_jj(["file", "show", "subdir/data.bin"]);
    insta::assert_snapshot!(output, @"
    hell0 w0rld
    [EOF]
    ");

    Ok(())
}

/// Test that a smudge filter applies on checkout when `.gitattributes` is
/// only in a subdirectory.
#[test]
fn test_filter_nested_gitattributes_checkout() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    add_filter_config(&work_dir, "testfilter", "tr o 0", "tr 0 o");

    work_dir.write_file("subdir/.gitattributes", "*.bin filter=testfilter\n");
    work_dir.write_file("subdir/data.bin", "hello world\n");

    work_dir.run_jj(["st"]).success();
    work_dir
        .run_jj(["commit", "-m", "add filtered file"])
        .success();

    // Remove file and checkout to test smudge.
    work_dir.remove_file("subdir/data.bin");
    work_dir.run_jj(["st"]).success();
    work_dir.run_jj(["new", "@-"]).success();

    let disk_content = work_dir.read_file("subdir/data.bin");
    assert_eq!(
        disk_content, "hello world\n",
        "smudge filter should restore content for subdir file"
    );

    Ok(())
}

/// Test that clean/smudge filters work correctly across multiple workspaces.
///
/// A filter configured in the main workspace should also apply when a second
/// workspace is created and checks out the same files. The filter driver
/// config lives in the shared Git repo, so both workspaces see it.
#[test]
fn test_filter_in_multiple_workspaces() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");

    add_filter_config(&main_dir, "testfilter", "tr o 0", "tr 0 o");

    main_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    main_dir.write_file("data.bin", "hello world\n");

    // Snapshot and commit in main workspace.
    main_dir.run_jj(["st"]).success();
    main_dir
        .run_jj(["commit", "-m", "add filtered file"])
        .success();

    // Verify stored content is cleaned.
    let output = main_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    hell0 w0rld
    [EOF]
    ");

    // Create a second workspace.
    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../second"])
        .success();
    let second_dir = test_env.work_dir("second");

    // The second workspace should have the file checked out with smudge applied.
    let disk_content = second_dir.read_file("data.bin");
    assert_eq!(
        disk_content, "hello world\n",
        "smudge filter should restore content in second workspace"
    );

    // Modifying the file in the second workspace should apply clean filter on snapshot.
    second_dir.write_file("data.bin", "foo bar\n");
    second_dir.run_jj(["st"]).success();

    let output = second_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    f00 bar
    [EOF]
    ");

    Ok(())
}

/// Test that LFS filters and submodules coexist correctly in a workspace.
///
/// This is the "full stack" test: a repo with both filtered files and a
/// submodule, verified in both the main workspace and a second workspace.
#[test]
fn test_filter_and_submodule_in_workspace() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.submodule.auto-update = true");

    // Create a submodule repo.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submod"])
        .success();
    let submod_dir = test_env.work_dir("submod");
    submod_dir.write_file("subfile", "sub content\n");
    submod_dir
        .run_jj(["commit", "-m", "submodule init"])
        .success();

    // Create the main repo with a filter and a submodule.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");

    add_filter_config(&main_dir, "testfilter", "tr o 0", "tr 0 o");
    main_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    main_dir.write_file("data.bin", "hello world\n");

    // Snapshot and commit the filtered file via jj.
    main_dir.run_jj(["st"]).success();
    main_dir
        .run_jj(["commit", "-m", "add filtered file"])
        .success();

    // Verify stored content is cleaned.
    let output = main_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    hell0 w0rld
    [EOF]
    ");

    // Add submodule via git.
    let output = main_dir.run_jj([
        "util",
        "exec",
        "--",
        "git",
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        &format!("{}/submod", test_env.env_root().display()),
        "sub",
    ]);
    assert!(
        output.status.success(),
        "git submodule add failed: {output}"
    );

    // Commit the submodule via git.
    let output = main_dir.run_jj([
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
        "add submodule",
    ]);
    assert!(output.status.success(), "git commit failed: {output}");

    // Import the git commit into jj.
    main_dir.run_jj(["st"]).success();

    // Create a second workspace.
    main_dir
        .run_jj(["workspace", "add", "--name", "second", "../second"])
        .success();
    let second_dir = test_env.work_dir("second");

    // LFS file should be smudged in second workspace.
    let disk_content = second_dir.read_file("data.bin");
    let disk_str = String::from_utf8_lossy(&disk_content);
    assert_eq!(
        disk_str, "hello world\n",
        "smudge filter should restore content in second workspace"
    );

    // Submodule directory should exist but be empty.
    assert!(
        second_dir.root().join("sub").is_dir(),
        "submodule directory should exist in second workspace"
    );
    // The submodule directory should be populated via the submodule store
    // in non-colocated workspaces.
    let entries: Vec<_> = std::fs::read_dir(second_dir.root().join("sub"))
        .unwrap()
        .collect();
    assert!(
        !entries.is_empty(),
        "submodule directory should be populated in non-colocated second workspace"
    );

    // .gitmodules should be present.
    let gitmodules = second_dir.read_file(".gitmodules");
    let gm_str = String::from_utf8_lossy(&gitmodules);
    assert!(
        gm_str.contains("[submodule \"sub\"]"),
        ".gitmodules should be present, got: {gm_str}"
    );

    // Modifying the filtered file in second workspace should apply clean filter.
    second_dir.write_file("data.bin", "foo bar\n");
    second_dir.run_jj(["st"]).success();
    let output = second_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    f00 bar
    [EOF]
    ");

    Ok(())
}

/// Test that a subdirectory `.gitattributes` can unset a filter set by the
/// root `.gitattributes`.
#[test]
fn test_filter_override_unset_in_subdir() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    add_filter_config(&work_dir, "testfilter", "tr o 0", "tr 0 o");

    // Root .gitattributes applies filter to all .bin files.
    work_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    // Subdir .gitattributes unsets the filter for .bin files.
    work_dir.write_file("nofilter/.gitattributes", "*.bin -filter\n");

    // Root file should be filtered.
    work_dir.write_file("root.bin", "hello world\n");
    // Subdir file should NOT be filtered.
    work_dir.write_file("nofilter/data.bin", "hello world\n");

    work_dir.run_jj(["st"]).success();

    // Root file: clean filter applied (o -> 0).
    let output = work_dir.run_jj(["file", "show", "root.bin"]);
    insta::assert_snapshot!(output, @"
    hell0 w0rld
    [EOF]
    ");

    // Subdir file: no filter, content stored as-is.
    let output = work_dir.run_jj(["file", "show", "nofilter/data.bin"]);
    insta::assert_snapshot!(output, @"
    hello world
    [EOF]
    ");

    Ok(())
}

/// Test that a failing smudge filter during checkout falls back to writing
/// the raw stored content instead of blocking checkout. This is important
/// for LFS where objects may not be in the local cache (e.g. fetchexclude).
#[test]
fn test_filter_smudge_failure() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    // Configure a filter with a failing smudge command.
    add_filter_config(&work_dir, "testfilter", "cat", "false");

    work_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    work_dir.write_file("data.bin", "hello world\n");

    // Snapshot with clean filter (cat = identity).
    work_dir.run_jj(["st"]).success();
    work_dir
        .run_jj(["commit", "-m", "add filtered file"])
        .success();

    // Remove file and force re-checkout. Smudge filter (false) will fail,
    // but checkout should succeed by falling back to the raw stored content.
    work_dir.remove_file("data.bin");
    work_dir.run_jj(["st"]).success();
    let output = work_dir.run_jj(["new", "@-"]);
    assert!(
        output.status.success(),
        "checkout should succeed even when smudge filter fails, got: {output}"
    );

    // The file should contain the raw stored content (pre-smudge), which
    // is the same as what the clean filter produced ("hello world\n").
    let content = work_dir.read_file("data.bin");
    let content_str = String::from_utf8_lossy(&content);
    assert!(
        content_str.contains("hello world"),
        "file should contain raw stored content, got: {content_str}"
    );

    Ok(())
}

/// Test that clean/smudge filters work correctly with EOL conversion.
///
/// The conversion order should match Git: snapshot applies clean filter
/// then EOL normalization (CRLF→LF), checkout applies EOL conversion
/// (LF→CRLF) then smudge filter.
#[test]
fn test_filter_and_eol_combined() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    add_filter_config(&work_dir, "testfilter", "tr o 0", "tr 0 o");

    // Both filter and eol attributes on the same file.
    work_dir.write_file(".gitattributes", "*.bin filter=testfilter eol=crlf\n");

    // Write CRLF content — clean filter (o→0) then EOL (CRLF→LF) on snapshot.
    work_dir.write_file("data.bin", "hello\r\nworld\r\n");

    let eol_config = "working-copy.eol-conversion='input-output'";
    work_dir.run_jj(["st", "--config", eol_config]).success();

    // Stored content: clean applied (o→0), then CRLF→LF.
    let output = work_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    hell0
    w0rld
    [EOF]
    ");

    // Commit, remove, and re-checkout to test smudge + EOL on checkout.
    work_dir
        .run_jj(["commit", "-m", "add filtered file", "--config", eol_config])
        .success();
    work_dir.remove_file("data.bin");
    work_dir.run_jj(["st", "--config", eol_config]).success();
    work_dir
        .run_jj(["new", "@-", "--config", eol_config])
        .success();

    // Checkout order: EOL (LF→CRLF) then smudge (0→o).
    let disk_content = work_dir.read_file("data.bin");
    let disk_str = String::from_utf8_lossy(&disk_content);
    assert_eq!(
        disk_str, "hello\r\nworld\r\n",
        "smudge filter should restore 0 to o, then EOL should convert LF to CRLF"
    );

    Ok(())
}

/// Test that modifying `.gitattributes` on disk (without committing) takes
/// effect immediately on the next snapshot.
#[test]
fn test_filter_gitattributes_modified_on_disk() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    add_filter_config(&work_dir, "testfilter", "tr o 0", "tr 0 o");

    // Initially, no .gitattributes — file stored as-is.
    work_dir.write_file("data.bin", "hello world\n");
    work_dir.run_jj(["st"]).success();
    let output = work_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    hello world
    [EOF]
    ");

    // Now add .gitattributes on disk (not committed yet).
    work_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    // Re-write the file to trigger re-snapshot.
    work_dir.write_file("data.bin", "hello world\n");
    work_dir.run_jj(["st"]).success();

    // The clean filter should now be applied.
    let output = work_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    hell0 w0rld
    [EOF]
    ");

    Ok(())
}

/// Test that filter smudge and submodule populate both work during the same
/// checkout in a colocated repo. This verifies the two features don't
/// interfere when active simultaneously.
#[test]
fn test_filter_and_submodule_combined_checkout() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.submodule.auto-update = true");

    // Create a submodule repo with a file (no filter — filter config is
    // not cloned by git submodule update --init, so we keep it simple).
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submod"])
        .success();
    let submod_dir = test_env.work_dir("submod");
    submod_dir.write_file("subfile.txt", "sub content\n");
    submod_dir
        .run_jj(["commit", "-m", "submodule init"])
        .success();

    // Create the main repo with its own filtered file.
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");
    add_filter_config(&main_dir, "testfilter", "tr o 0", "tr 0 o");
    main_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    main_dir.write_file("data.bin", "hello world\n");
    main_dir.run_jj(["st"]).success();
    main_dir
        .run_jj(["commit", "-m", "add filtered file"])
        .success();
    main_dir
        .run_jj(["bookmark", "create", "nofilter"])
        .success();

    // Add submodule via git.
    main_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/submod", test_env.env_root().display()),
            "sub",
        ])
        .success();
    main_dir
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
            "add submodule",
        ])
        .success();
    main_dir.run_jj(["st"]).success();
    main_dir.run_jj(["bookmark", "create", "withsub"]).success();

    // Switch to the commit without submodule.
    main_dir.run_jj(["new", "nofilter"]).success();
    // Remove the submodule directory and filtered file.
    let _ = std::fs::remove_dir_all(main_dir.root().join("sub")).ok();
    let _ = std::fs::remove_file(main_dir.root().join("data.bin")).ok();

    // Switch back — both smudge filter and submodule populate should run.
    let output = main_dir.run_jj(["new", "withsub"]);
    output.success();

    // Filtered file: smudge should have restored original content.
    let disk_content = main_dir.read_file("data.bin");
    let disk_str = String::from_utf8_lossy(&disk_content);
    assert_eq!(
        disk_str, "hello world\n",
        "smudge filter should restore content during combined checkout"
    );

    // Submodule: should be populated.
    let subfile = main_dir.root().join("sub").join("subfile.txt");
    assert!(
        subfile.exists(),
        "submodule file should exist after combined checkout"
    );
    let content = std::fs::read_to_string(&subfile).unwrap();
    assert_eq!(
        content, "sub content\n",
        "submodule content should be correct after combined checkout"
    );

    Ok(())
}

/// Test that the clean filter is re-applied when a conflict is resolved
/// during snapshot.
///
/// When a file with a conflict is resolved in the working copy (conflict
/// markers removed), the snapshot process detects the resolution. However,
/// the resolved content was read without the clean filter (only EOL
/// conversion was applied). The code must re-apply the clean filter to
/// ensure the stored content is consistent with other filtered files.
#[test]
fn test_filter_conflict_resolution_reapplies_clean() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    // Use a simple filter: clean uppercases, smudge lowercases.
    add_filter_config(&work_dir, "casefilter", "tr a-z A-Z", "tr A-Z a-z");
    work_dir.write_file(".gitattributes", "*.bin filter=casefilter\n");

    // Create initial commit with filtered content.
    work_dir.write_file("data.bin", "hello world\n");
    work_dir.run_jj(["describe", "-m", "base"]).success();

    // Verify clean filter was applied: stored content should be uppercase.
    let stored = work_dir.run_jj(["file", "show", "data.bin"]);
    assert!(
        stored.stdout.to_string().contains("HELLO WORLD"),
        "clean filter should uppercase content, got: {}",
        stored.stdout
    );

    // Create two divergent commits that modify the file differently.
    work_dir
        .run_jj(["bookmark", "create", "-r@", "a"])
        .success();
    work_dir.run_jj(["new", "@-"]).success();
    work_dir.write_file("data.bin", "hello earth\n");
    work_dir
        .run_jj(["bookmark", "create", "-r@", "b"])
        .success();

    // Merge a and b — this creates a conflict.
    work_dir.run_jj(["new", "a", "b"]).success();

    // The working copy should now have conflict markers.
    let disk = work_dir.read_file("data.bin");
    let disk_str = String::from_utf8_lossy(&disk);
    assert!(
        disk_str.contains("<<<<<<<") || disk_str.contains("+++++++"),
        "working copy should have conflict markers, got: {disk_str}"
    );

    // Resolve the conflict by writing a clean file (no conflict markers).
    // Write lowercase content — the clean filter should uppercase it.
    work_dir.write_file("data.bin", "resolved content\n");

    // Snapshot — this triggers the conflict resolution + clean filter
    // re-application code path.
    work_dir.run_jj(["st"]).success();

    // The stored content should have the clean filter applied (uppercase).
    let stored = work_dir.run_jj(["file", "show", "data.bin", "-r", "@"]);
    let stored_str = stored.stdout.to_string();
    assert!(
        stored_str.contains("RESOLVED CONTENT"),
        "clean filter should be re-applied to resolved conflict content, got: {stored_str}"
    );
    assert!(
        !stored_str.contains("resolved content"),
        "unfiltered content should not appear in store, got: {stored_str}"
    );

    Ok(())
}

/// Test that the smudge filter is correctly applied when recovering a stale
/// workspace via `jj workspace update-stale`.
///
/// This tests the interaction between workspace staleness recovery and filter
/// drivers: when the working copy is restored during recovery, the smudge
/// filter must be applied to the checked-out files.
#[test]
fn test_filter_stale_workspace_recovery() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "main"])
        .success();
    let main_dir = test_env.work_dir("main");

    // Set up a filter: clean replaces 'o' with '0', smudge reverses it.
    add_filter_config(&main_dir, "testfilter", "tr o 0", "tr 0 o");
    main_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    main_dir.write_file("data.bin", "hello world\n");
    main_dir.run_jj(["describe", "-m", "base"]).success();
    main_dir.run_jj(["new"]).success();

    // Create a second workspace.
    main_dir
        .run_jj(["workspace", "add", "../secondary"])
        .success();
    let secondary_dir = test_env.work_dir("secondary");

    // In the secondary workspace, change the filtered file and squash into
    // the parent commit. This rewrites the commit the main workspace's
    // working copy points to, making it stale.
    secondary_dir.write_file("data.bin", "hello from secondary\n");
    secondary_dir.run_jj(["squash"]).success();

    // Export to git so the main workspace detects staleness via git ref changes.
    secondary_dir.run_jj(["git", "export"]).success();

    // The main workspace is now stale — its working copy commit has been
    // rewritten by the squash.
    let output = main_dir.run_jj(["st"]);
    let combined = format!("{output}");
    assert!(
        combined.contains("stale") || combined.contains("update-stale"),
        "main workspace should be stale, got: {combined}"
    );

    // Recover the stale workspace. The smudge filter should be applied to
    // the checked-out file content.
    let output = main_dir.run_jj(["workspace", "update-stale"]);
    let combined = format!("{output}");
    assert!(
        combined.contains("Updated working copy"),
        "update-stale should recover the workspace, got: {combined}"
    );

    // The working copy file should have the smudge filter applied.
    // The stored content has '0' instead of 'o' (clean filter), and the
    // smudge filter should restore 'o'.
    let disk_content = main_dir.read_file("data.bin");
    let disk_str = String::from_utf8_lossy(&disk_content);
    assert!(
        disk_str.contains("hello from secondary"),
        "smudge filter should restore content during stale recovery, got: {disk_str}"
    );
    assert!(
        !disk_str.contains("hell0 fr0m sec0ndary"),
        "unsmudged content should not appear on disk, got: {disk_str}"
    );

    Ok(())
}

/// Test that smudge filter failure falls back to writing raw stored content,
/// and that jj diff / git status remain consistent through repeated
/// smudge/fallback cycles. This simulates the LFS fetchexclude scenario
/// where objects may or may not be in the local cache.
#[test]
fn test_filter_smudge_fallback_consistency() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    // Use a smudge command that conditionally fails: if /tmp/jj_smudge_fail
    // exists, smudge fails (simulating missing LFS object); otherwise smudge
    // is identity (simulating object in cache).
    let smudge_cmd = "test -f /tmp/jj_smudge_fail && false || cat";
    add_filter_config(&work_dir, "testfilter", "cat", smudge_cmd);

    work_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    work_dir.write_file("data.bin", "hello world\n");

    // Snapshot with clean filter.
    work_dir.run_jj(["st"]).success();
    work_dir
        .run_jj(["commit", "-m", "add filtered file"])
        .success();

    // Scenario 1: smudge succeeds (no fail flag).
    std::fs::remove_file("/tmp/jj_smudge_fail").ok();
    work_dir.remove_file("data.bin");
    work_dir.run_jj(["st"]).success();
    work_dir.run_jj(["new", "@-"]).success();
    let content_bytes = work_dir.read_file("data.bin");
    let content = String::from_utf8_lossy(&content_bytes);
    assert!(
        content.contains("hello world"),
        "smudge should produce real content, got: {content}"
    );

    // jj diff and git status should be clean.
    work_dir.run_jj(["st"]).success();
    let git_status = std::process::Command::new("git")
        .args(["status", "--short"])
        .current_dir(work_dir.root())
        .output()
        .unwrap();
    assert!(
        git_status.stdout.is_empty(),
        "git status should be clean after smudge, got: {}",
        String::from_utf8_lossy(&git_status.stdout)
    );

    // Scenario 2: smudge fails (fail flag set) — should fall back to raw content.
    std::fs::write("/tmp/jj_smudge_fail", "1").unwrap();
    work_dir.remove_file("data.bin");
    work_dir.run_jj(["st"]).success();
    work_dir.run_jj(["new", "@-"]).success();
    let content_bytes = work_dir.read_file("data.bin");
    let content = String::from_utf8_lossy(&content_bytes);
    assert!(
        content.contains("hello world"),
        "fallback should write raw stored content, got: {content}"
    );

    // jj diff and git status should still be clean.
    work_dir.run_jj(["st"]).success();
    let git_status = std::process::Command::new("git")
        .args(["status", "--short"])
        .current_dir(work_dir.root())
        .output()
        .unwrap();
    assert!(
        git_status.stdout.is_empty(),
        "git status should be clean after fallback, got: {}",
        String::from_utf8_lossy(&git_status.stdout)
    );

    // Scenario 3: smudge succeeds again (fail flag removed).
    std::fs::remove_file("/tmp/jj_smudge_fail").unwrap();
    work_dir.remove_file("data.bin");
    work_dir.run_jj(["st"]).success();
    work_dir.run_jj(["new", "@-"]).success();
    let content_bytes = work_dir.read_file("data.bin");
    let content = String::from_utf8_lossy(&content_bytes);
    assert!(
        content.contains("hello world"),
        "smudge should produce real content after cache restored, got: {content}"
    );

    work_dir.run_jj(["st"]).success();
    let git_status = std::process::Command::new("git")
        .args(["status", "--short"])
        .current_dir(work_dir.root())
        .output()
        .unwrap();
    assert!(
        git_status.stdout.is_empty(),
        "git status should be clean after re-smudge, got: {}",
        String::from_utf8_lossy(&git_status.stdout)
    );

    std::fs::remove_file("/tmp/jj_smudge_fail").ok();
    Ok(())
}

/// Test that a process-mode filter works end-to-end with jj CLI commands.
///
/// Uses a Python script implementing the Git filter process protocol.
/// Clean replaces 'o' with '0', smudge replaces '0' with 'o' — same
/// transformation as the per-file test, but via the long-running process.
#[test]
fn test_filter_process_mode_roundtrip() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    let process_cmd = write_process_filter_script(&work_dir);
    add_process_filter_config(&work_dir, "testfilter", &process_cmd);

    work_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    work_dir.write_file("data.bin", "hello world from jj\n");

    // Snapshot — clean filter should run via process.
    work_dir.run_jj(["st"]).success();

    // The stored content should be the cleaned version (o->0).
    let output = work_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    hell0 w0rld fr0m jj
    [EOF]
    ");

    // Commit the file.
    work_dir
        .run_jj(["commit", "-m", "add filtered file via process"])
        .success();

    // Remove the file and snapshot.
    work_dir.remove_file("data.bin");
    work_dir.run_jj(["st"]).success();

    // Checkout the commit — smudge filter should run via process.
    work_dir.run_jj(["new", "@-"]).success();

    // The disk content should be the smudged version (0->o).
    let disk_content = work_dir.read_file("data.bin");
    assert_eq!(
        disk_content, "hello world from jj\n",
        "smudge filter should restore original content via process mode"
    );

    // Working copy should be clean.
    let output = work_dir.run_jj(["diff", "--summary"]);
    assert!(output.stdout.is_empty(), "working copy should be clean");

    Ok(())
}

/// Test that process mode handles multiple files in a single snapshot/checkout
/// without the process getting out of sync.
#[test]
fn test_filter_process_mode_multiple_files() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    let process_cmd = write_process_filter_script(&work_dir);
    add_process_filter_config(&work_dir, "testfilter", &process_cmd);

    work_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");

    // Write multiple files to exercise the long-running process.
    work_dir.write_file("file1.bin", "hello world one\n");
    work_dir.write_file("file2.bin", "hello world two\n");
    work_dir.write_file("file3.bin", "hello world three\n");

    // Snapshot — clean filter runs for each file via the same process.
    work_dir.run_jj(["st"]).success();

    // Verify all files were cleaned correctly.
    let output = work_dir.run_jj(["file", "show", "file1.bin"]);
    insta::assert_snapshot!(output, @"
    hell0 w0rld 0ne
    [EOF]
    ");
    let output = work_dir.run_jj(["file", "show", "file2.bin"]);
    insta::assert_snapshot!(output, @"
    hell0 w0rld tw0
    [EOF]
    ");
    let output = work_dir.run_jj(["file", "show", "file3.bin"]);
    insta::assert_snapshot!(output, @"
    hell0 w0rld three
    [EOF]
    ");

    // Commit, remove all, and re-checkout to test smudge on all files.
    work_dir
        .run_jj(["commit", "-m", "add multiple filtered files"])
        .success();
    work_dir.remove_file("file1.bin");
    work_dir.remove_file("file2.bin");
    work_dir.remove_file("file3.bin");
    work_dir.run_jj(["st"]).success();
    work_dir.run_jj(["new", "@-"]).success();

    // All files should be smudged correctly.
    assert_eq!(
        work_dir.read_file("file1.bin"),
        "hello world one\n",
        "file1 should be smudged"
    );
    assert_eq!(
        work_dir.read_file("file2.bin"),
        "hello world two\n",
        "file2 should be smudged"
    );
    assert_eq!(
        work_dir.read_file("file3.bin"),
        "hello world three\n",
        "file3 should be smudged"
    );

    Ok(())
}

/// Test that process mode falls back to per-file mode when the process
/// command fails to start (e.g. command not found).
#[test]
fn test_filter_process_mode_fallback() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    // Configure both process (nonexistent) and per-file (working) commands.
    // The driver should try process mode first, fail, then fall back to per-file.
    add_filter_config(&work_dir, "testfilter", "tr o 0", "tr 0 o");
    add_process_filter_config(&work_dir, "testfilter", "nonexistent_process_cmd_12345");

    work_dir.write_file(".gitattributes", "*.bin filter=testfilter\n");
    work_dir.write_file("data.bin", "hello world\n");

    // Snapshot should succeed via per-file fallback.
    work_dir.run_jj(["st"]).success();

    let output = work_dir.run_jj(["file", "show", "data.bin"]);
    insta::assert_snapshot!(output, @"
    hell0 w0rld
    [EOF]
    ");

    // Commit, remove, and checkout — smudge should also fall back.
    work_dir
        .run_jj(["commit", "-m", "add filtered file with fallback"])
        .success();
    work_dir.remove_file("data.bin");
    work_dir.run_jj(["st"]).success();
    work_dir.run_jj(["new", "@-"]).success();

    let disk_content = work_dir.read_file("data.bin");
    assert_eq!(
        disk_content, "hello world\n",
        "smudge should work via per-file fallback"
    );

    Ok(())
}
