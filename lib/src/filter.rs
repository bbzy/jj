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
// See the License for the specific language governing permissions and
// limitations under the License.

//! Git filter driver support for clean/smudge operations.
//!
//! Filter drivers (such as Git LFS) are defined in Git config under
//! `[filter "<name>"]` sections. When `.gitattributes` assigns
//! `filter=<name>` to a path, the corresponding clean/smudge command
//! is executed during working-copy snapshot and checkout.
//!
//! Two modes are supported:
//! - **Per-file mode** (`filter.<name>.clean` / `filter.<name>.smudge`):
//!   spawns a new process for each file. Simple but slow for large repos.
//! - **Process mode** (`filter.<name>.process`): uses the Git filter process
//!   protocol — a single long-running process handles all files via a
//!   pkt-line based protocol. This is the preferred mode for LFS and other
//!   expensive filters.

use std::collections::HashMap;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;

use crate::store::Store;

// ---------------------------------------------------------------------------
// Pkt-line protocol
// ---------------------------------------------------------------------------

/// Maximum data payload per pkt-line (65520 total - 4 byte header).
const PKT_LINE_MAX_DATA: usize = 65516;

/// Maximum total pkt-line length including the 4-byte header.
const PKT_LINE_MAX_LEN: usize = PKT_LINE_MAX_DATA + 4;

/// Writes a single pkt-line containing `data`.
///
/// The pkt-line format is: 4-byte hex length (including the 4 bytes
/// themselves) followed by the data payload.
fn write_pkt_line(writer: &mut impl Write, data: &[u8]) -> std::io::Result<()> {
    let len = data.len() + 4;
    assert!(
        len <= PKT_LINE_MAX_LEN,
        "pkt-line data too large: {len} > {PKT_LINE_MAX_LEN}"
    );
    // Write header and data separately — safe because stdin is BufWriter,
    // which batches writes and flushes explicitly.
    write!(writer, "{len:04x}")?;
    writer.write_all(data)
}

/// Writes a flush packet (`0000`).
fn write_flush(writer: &mut impl Write) -> std::io::Result<()> {
    writer.write_all(b"0000")
}

/// Reads a single pkt-line, returning `Ok(Some(data))` for a data line,
/// or `Ok(None)` for a flush packet.
fn read_pkt_line(reader: &mut impl BufRead) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "filter process closed connection during pkt-line header",
            ));
        }
        Err(e) => return Err(e),
    }
    let len_str = std::str::from_utf8(&len_buf).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid pkt-line length: non-UTF-8 {len_buf:?}"),
        )
    })?;
    let len = u16::from_str_radix(len_str, 16).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid pkt-line length: {len_str}"),
        )
    })? as usize;
    if len == 0 {
        return Ok(None);
    }
    if len < 4 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid pkt-line length: {len} < 4"),
        ));
    }
    let data_len = len - 4;
    let mut data = vec![0u8; data_len];
    reader.read_exact(&mut data)?;
    Ok(Some(data))
}

// ---------------------------------------------------------------------------
// FilterProcess — long-running process protocol
// ---------------------------------------------------------------------------

/// A long-running filter process using the Git filter process protocol.
///
/// The process is started once and reused for all subsequent clean/smudge
/// operations. Communication uses the pkt-line format over stdin/stdout.
struct FilterProcess {
    child: std::process::Child,
    stdin: BufWriter<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
}

impl FilterProcess {
    /// Starts the filter process and performs the protocol handshake.
    fn start(command: &str, current_dir: Option<&Path>) -> Result<Self, FilterError> {
        let (program, args) = split_cmd(command);
        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Inherit stderr so the user sees filter diagnostics (e.g. LFS
            // warnings). We do NOT pipe stderr because if the process writes
            // a lot to stderr and we don't drain it, the OS pipe buffer fills
            // up and the process blocks indefinitely. This differs from
            // per-file mode (run_filter_cmd) which captures stderr for error
            // messages, but the long-running process needs stderr open for its
            // entire lifetime, making draining impractical.
            .stderr(Stdio::inherit());
        if let Some(dir) = current_dir {
            cmd.current_dir(dir);
        }

        let mut child = cmd.spawn().map_err(|e| FilterError::CommandFailed {
            command: command.to_string(),
            message: e.to_string(),
        })?;
        let stdin = BufWriter::new(child.stdin.take().expect("stdin should be piped"));
        let stdout = BufReader::new(child.stdout.take().expect("stdout should be piped"));

        let mut proc = Self {
            child,
            stdin,
            stdout,
        };
        proc.handshake(command)?;
        Ok(proc)
    }

    /// Performs the Git filter process protocol handshake.
    ///
    /// The protocol is (client = jj, server = e.g. git-lfs):
    /// 1. Client sends `git-filter-client` (single pkt-line)
    /// 2. Client sends supported versions as a list: `version=2` + flush
    /// 3. Server responds with a list: `git-filter-server`, `version=2` + flush
    /// 4. Client sends its capabilities as a list: `capability=clean`,
    ///    `capability=smudge` + flush
    /// 5. Server responds with selected capabilities as a list + flush
    fn handshake(&mut self, command: &str) -> Result<(), FilterError> {
        // Step 1: send welcome message
        write_pkt_line(&mut self.stdin, b"git-filter-client")
            .map_err(|e| io_error("handshake write welcome", command, e))?;
        // Step 2: send supported versions (list = pkt-lines + flush)
        write_pkt_line(&mut self.stdin, b"version=2")
            .map_err(|e| io_error("handshake write version", command, e))?;
        write_flush(&mut self.stdin)
            .map_err(|e| io_error("handshake write flush", command, e))?;
        self.stdin
            .flush()
            .map_err(|e| io_error("handshake flush", command, e))?;

        // Step 3: read server response (list = pkt-lines until flush)
        // Server sends: "git-filter-server", "version=2", flush
        // Note: pkt-line text data may include a trailing newline.
        let mut server_info = Vec::new();
        while let Some(line) = read_pkt_line(&mut self.stdout)
            .map_err(|e| io_error("handshake read server info", command, e))?
        {
            server_info.push(String::from_utf8_lossy(&line).trim().to_string());
        }
        if !server_info.iter().any(|s| s == "git-filter-server") {
            return Err(FilterError::CommandFailed {
                command: command.to_string(),
                message: format!(
                    "filter process did not identify as git-filter-server: {server_info:?}"
                ),
            });
        }
        if !server_info.iter().any(|s| s == "version=2") {
            return Err(FilterError::CommandFailed {
                command: command.to_string(),
                message: format!("filter process does not support version=2: {server_info:?}"),
            });
        }

        // Step 4: send client capabilities (list = pkt-lines + flush)
        write_pkt_line(&mut self.stdin, b"capability=clean")
            .map_err(|e| io_error("handshake write cap clean", command, e))?;
        write_pkt_line(&mut self.stdin, b"capability=smudge")
            .map_err(|e| io_error("handshake write cap smudge", command, e))?;
        write_flush(&mut self.stdin)
            .map_err(|e| io_error("handshake write cap flush", command, e))?;
        self.stdin
            .flush()
            .map_err(|e| io_error("handshake cap flush", command, e))?;

        // Step 5: read server's selected capabilities (list until flush)
        let mut capabilities = Vec::new();
        while let Some(line) = read_pkt_line(&mut self.stdout)
            .map_err(|e| io_error("handshake read capability", command, e))?
        {
            capabilities.push(String::from_utf8_lossy(&line).trim().to_string());
        }

        // Validate that the server selected the capabilities we need.
        if !capabilities.iter().any(|s| s == "capability=clean") {
            return Err(FilterError::CommandFailed {
                command: command.to_string(),
                message: format!(
                    "filter process did not select capability=clean: {capabilities:?}"
                ),
            });
        }
        if !capabilities.iter().any(|s| s == "capability=smudge") {
            return Err(FilterError::CommandFailed {
                command: command.to_string(),
                message: format!(
                    "filter process did not select capability=smudge: {capabilities:?}"
                ),
            });
        }

        tracing::debug!(
            command = command,
            server_info = ?server_info,
            capabilities = ?capabilities,
            "filter process handshake completed"
        );

        Ok(())
    }

    /// Filters content through the long-running process.
    ///
    /// The Git filter process protocol is a two-phase exchange per file:
    ///
    /// **Phase 1 — header:**
    /// 1. Client sends `command=<clean|smudge>`, `pathname=<path>` as
    ///    pkt-lines, then a flush-pkt.
    /// 2. Server responds with `status=success` (or `status=error`) as a
    ///    pkt-line, then a flush-pkt.
    ///
    /// **Phase 2 — content (only if status is success):**
    /// 3. Client sends the content as pkt-line chunks, then a flush-pkt.
    /// 4. Server responds with the filtered content as pkt-line chunks,
    ///    then a flush-pkt.
    /// 5. Server sends a final `status=success` as a pkt-line, then a
    ///    flush-pkt.
    fn filter(
        &mut self,
        command: &str,
        pathname: &str,
        content: &[u8],
    ) -> Result<Vec<u8>, FilterError> {
        // Phase 1: send headers + flush
        let cmd_line = format!("command={command}");
        write_pkt_line(&mut self.stdin, cmd_line.as_bytes())
            .map_err(|e| io_error("write command", command, e))?;
        let path_line = format!("pathname={pathname}");
        write_pkt_line(&mut self.stdin, path_line.as_bytes())
            .map_err(|e| io_error("write pathname", command, e))?;
        write_flush(&mut self.stdin)
            .map_err(|e| io_error("write header flush", command, e))?;
        self.stdin
            .flush()
            .map_err(|e| io_error("flush after headers", command, e))?;

        // Phase 1: read status + flush
        let status_line = read_pkt_line(&mut self.stdout)
            .map_err(|e| io_error("read status", command, e))?
            .ok_or_else(|| FilterError::CommandFailed {
                command: command.to_string(),
                message: "filter process sent flush instead of status".to_string(),
            })?;
        let status = String::from_utf8_lossy(&status_line);
        if status.trim() != "status=success" {
            // Read optional error message until flush
            let mut err_msg = String::new();
            while let Some(msg) = read_pkt_line(&mut self.stdout)
                .map_err(|e| io_error("read error message", command, e))?
            {
                if err_msg.is_empty() {
                    err_msg = String::from_utf8_lossy(&msg).to_string();
                }
            }
            return Err(FilterError::CommandFailed {
                command: command.to_string(),
                message: if err_msg.is_empty() {
                    format!("filter process returned {status}")
                } else {
                    format!("filter process returned {status}: {err_msg}")
                },
            });
        }
        // Consume any remaining header-response data until flush
        while read_pkt_line(&mut self.stdout)
            .map_err(|e| io_error("read status flush", command, e))?
            .is_some()
        {}

        // Phase 2: send content + flush
        for chunk in content.chunks(PKT_LINE_MAX_DATA) {
            write_pkt_line(&mut self.stdin, chunk)
                .map_err(|e| io_error("write content", command, e))?;
        }
        write_flush(&mut self.stdin)
            .map_err(|e| io_error("write content flush", command, e))?;
        self.stdin
            .flush()
            .map_err(|e| io_error("flush after content", command, e))?;

        // Phase 2: read filtered content until flush
        let mut result = Vec::new();
        while let Some(chunk) = read_pkt_line(&mut self.stdout)
            .map_err(|e| io_error("read content", command, e))?
        {
            result.extend_from_slice(&chunk);
        }

        // Phase 2: read final status + flush
        let final_status_line = read_pkt_line(&mut self.stdout)
            .map_err(|e| io_error("read final status", command, e))?
            .ok_or_else(|| FilterError::CommandFailed {
                command: command.to_string(),
                message: "filter process sent flush instead of final status".to_string(),
            })?;
        let final_status = String::from_utf8_lossy(&final_status_line);
        tracing::debug!(
            command = command,
            result_len = result.len(),
            "filter process completed"
        );
        if final_status.trim() != "status=success" {
            // Consume remaining data until flush
            while read_pkt_line(&mut self.stdout)
                .map_err(|e| io_error("read final error flush", command, e))?
                .is_some()
            {}
            return Err(FilterError::CommandFailed {
                command: command.to_string(),
                message: format!("filter process final status: {final_status}"),
            });
        }
        // Consume the flush-pkt after the final status
        while read_pkt_line(&mut self.stdout)
            .map_err(|e| io_error("read final flush", command, e))?
            .is_some()
        {}

        Ok(result)
    }
}

fn io_error(context: &str, command: &str, e: std::io::Error) -> FilterError {
    FilterError::CommandFailed {
        command: command.to_string(),
        message: format!("{context}: {e}"),
    }
}

impl Drop for FilterProcess {
    fn drop(&mut self) {
        // Don't flush stdin — if the process is blocked writing to stdout
        // (which we're not reading), flush() could hang on a full pipe buffer.
        // Just kill the process and wait for it to exit.
        if let Err(err) = self.child.kill() {
            tracing::debug!(error = %err, "failed to kill filter process");
        }
        if let Err(err) = self.child.wait() {
            tracing::debug!(error = %err, "failed to wait for filter process");
        }
    }
}

// ---------------------------------------------------------------------------
// FilterDriver — unified interface for per-file and process modes
// ---------------------------------------------------------------------------

/// A configured filter driver parsed from Git config.
///
/// Supports two modes:
/// - **Per-file mode**: `clean_command` / `smudge_command` spawn a new
///   process per file (the traditional Git filter protocol).
/// - **Process mode**: `process_command` starts a single long-running
///   process using the Git filter process protocol (pkt-line based).
///   Falls back to per-file mode if the process fails to start.
pub struct FilterDriver {
    /// The clean command from `filter.<name>.clean`.
    pub clean_command: Option<String>,
    /// The smudge command from `filter.<name>.smudge`.
    pub smudge_command: Option<String>,
    /// The process command from `filter.<name>.process`.
    pub process_command: Option<String>,
    /// Whether the filter is required (`filter.<name>.required`).
    ///
    /// When true, a filter failure is a hard error. When false (the default),
    /// a smudge failure falls back to writing the raw stored content, matching
    /// Git's behavior for non-required filters.
    pub required: bool,
    /// The workspace root directory, used as `current_dir` for filter processes.
    /// This matches Git's behavior of running filter commands with cwd set to
    /// the repository root.
    pub workspace_root: Option<PathBuf>,
    /// Lazily-initialized long-running process, when `process_command` is set.
    ///
    /// Stored as `Ok(proc)` once the process is started, or `Err(err)` if
    /// startup failed (to avoid repeated spawn attempts on a broken command).
    process: Mutex<Option<Result<FilterProcess, FilterError>>>,
}

impl std::fmt::Debug for FilterDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilterDriver")
            .field("clean_command", &self.clean_command)
            .field("smudge_command", &self.smudge_command)
            .field("process_command", &self.process_command)
            .field("required", &self.required)
            .finish()
    }
}

impl FilterDriver {
    /// Creates a new filter driver with the given commands.
    #[cfg(test)]
    pub fn new(
        clean_command: Option<String>,
        smudge_command: Option<String>,
        process_command: Option<String>,
    ) -> Self {
        Self::new_with_required(clean_command, smudge_command, process_command, false)
    }

    /// Creates a new filter driver with an explicit `required` flag.
    pub fn new_with_required(
        clean_command: Option<String>,
        smudge_command: Option<String>,
        process_command: Option<String>,
        required: bool,
    ) -> Self {
        Self {
            clean_command,
            smudge_command,
            process_command,
            required,
            workspace_root: None,
            process: Mutex::new(None),
        }
    }

    /// Returns true if this driver can perform clean operations
    /// (either via process mode or per-file clean command).
    pub fn has_clean(&self) -> bool {
        self.process_command.is_some() || self.clean_command.is_some()
    }

    /// Sets the workspace root directory used as `current_dir` for filter
    /// processes. This should be called before any `clean`/`smudge` calls.
    pub fn set_workspace_root(&mut self, root: PathBuf) {
        self.workspace_root = Some(root);
    }

    /// Returns true if this driver can perform smudge operations
    /// (either via process mode or per-file smudge command).
    pub fn has_smudge(&self) -> bool {
        self.process_command.is_some() || self.smudge_command.is_some()
    }

    /// Runs the clean filter on a file, returning the cleaned content.
    ///
    /// If a process command is configured, uses the long-running process.
    /// Otherwise, falls back to spawning a per-file clean command.
    ///
    /// `disk_path` is used for `%f` substitution in per-file mode.
    /// `repo_path` is sent as `pathname=` in process mode (repo-relative).
    pub fn clean(&self, file_content: &[u8], disk_path: &Path, repo_path: &str) -> Result<Vec<u8>, FilterError> {
        if self.process_command.is_some() {
            match self.filter_via_process("clean", repo_path, file_content) {
                Ok(result) => return Ok(result),
                Err(err) => {
                    if self.clean_command.is_none() {
                        return Err(err);
                    }
                }
            }
        }
        let cmd = self
            .clean_command
            .as_ref()
            .ok_or(FilterError::NoCleanCommand)?;
        run_filter_cmd(cmd, disk_path, Some(file_content), self.workspace_root.as_deref())
    }

    /// Runs the smudge filter on pointer content, returning the real content.
    ///
    /// If a process command is configured, uses the long-running process.
    /// Otherwise, falls back to spawning a per-file smudge command.
    ///
    /// `disk_path` is used for `%f` substitution in per-file mode.
    /// `repo_path` is sent as `pathname=` in process mode (repo-relative).
    pub fn smudge(&self, pointer_content: &[u8], disk_path: &Path, repo_path: &str) -> Result<Vec<u8>, FilterError> {
        if self.process_command.is_some() {
            match self.filter_via_process("smudge", repo_path, pointer_content) {
                Ok(result) => return Ok(result),
                Err(err) => {
                    if self.smudge_command.is_none() {
                        return Err(err);
                    }
                }
            }
        }
        let cmd = self
            .smudge_command
            .as_ref()
            .ok_or(FilterError::NoSmudgeCommand)?;
        run_filter_cmd(cmd, disk_path, Some(pointer_content), self.workspace_root.as_deref())
    }

    /// Sends a clean/smudge request through the long-running process.
    ///
    /// Lazily starts the process on first use. If the process fails to
    /// start or has died since the last call, it is restarted. If the
    /// restart also fails, returns an error so the caller can fall back
    /// to per-file mode.
    fn filter_via_process(
        &self,
        command: &str,
        repo_path: &str,
        content: &[u8],
    ) -> Result<Vec<u8>, FilterError> {
        let process_cmd = self.process_command.as_ref().unwrap();
        let mut guard = self.process.lock().map_err(|_| FilterError::CommandFailed {
            command: process_cmd.clone(),
            message: "filter process mutex poisoned".to_string(),
        })?;
        if guard.is_none() {
            match FilterProcess::start(process_cmd, self.workspace_root.as_deref()) {
                Ok(proc) => *guard = Some(Ok(proc)),
                Err(err) => {
                    // Cache the startup failure to avoid repeated spawn
                    // attempts on a broken command (e.g. not found).
                    tracing::debug!(
                        command = command,
                        error = %err,
                        "filter process failed to start"
                    );
                    *guard = Some(Err(err.clone()));
                    return Err(err);
                }
            }
        }
        match guard.as_ref().unwrap() {
            Ok(_) => {}
            Err(err) => return Err(err.clone()),
        }

        let proc = guard.as_mut().unwrap().as_mut().unwrap();
        match proc.filter(command, repo_path, content) {
            Ok(result) => Ok(result),
            Err(err) => {
                tracing::debug!(
                    command = command,
                    error = %err,
                    "filter process failed, falling back to per-file mode"
                );
                // Process may have died. Reset so next call can restart it.
                *guard = None;
                Err(err)
            }
        }
    }
}

/// Substitutes `%f` in the command template with the given path and runs it.
///
/// The template is split into program + args **before** substitution, so
/// paths containing spaces are preserved as a single argument. This differs
/// from Git which uses `/bin/sh -c`, but avoids shell injection risks.
fn run_filter_cmd(
    cmd_template: &str,
    path: &Path,
    stdin_content: Option<&[u8]>,
    current_dir: Option<&Path>,
) -> Result<Vec<u8>, FilterError> {
    let (program, mut args) = split_cmd(cmd_template);
    let path_str = path.to_string_lossy();
    for arg in &mut args {
        if arg.contains("%f") {
            *arg = arg.replace("%f", &path_str);
        }
    }
    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .stdin(if stdin_content.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = current_dir {
        cmd.current_dir(dir);
    }

    let mut child = cmd.spawn().map_err(|e| FilterError::CommandFailed {
        command: cmd_template.to_string(),
        message: e.to_string(),
    })?;

    // Write stdin on a separate thread so that stdout/stderr can be drained
    // by `wait_with_output` while the child is still reading input.
    let stdin_thread = stdin_content.map(|content| {
        let mut stdin = child.stdin.take().expect("stdin should be piped");
        let content = content.to_vec();
        std::thread::spawn(move || stdin.write_all(&content))
    });

    let output = child
        .wait_with_output()
        .map_err(|e| FilterError::CommandFailed {
            command: cmd_template.to_string(),
            message: e.to_string(),
        })?;

    if let Some(handle) = stdin_thread {
        let write_result = handle.join().map_err(|_| FilterError::CommandFailed {
            command: cmd_template.to_string(),
            message: "stdin writer thread panicked".to_string(),
        })?;
        write_result.map_err(|e| FilterError::CommandFailed {
            command: cmd_template.to_string(),
            message: e.to_string(),
        })?;
    }

    if !output.status.success() {
        return Err(FilterError::CommandFailed {
            command: cmd_template.to_string(),
            message: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    Ok(output.stdout)
}

/// Runs a command string (no path substitution), optionally piping stdin.
#[cfg(test)]
fn run_cmd(cmd_str: &str, stdin_content: Option<&[u8]>) -> Result<Vec<u8>, FilterError> {
    let (program, args) = split_cmd(cmd_str);
    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .stdin(if stdin_content.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| FilterError::CommandFailed {
        command: cmd_str.to_string(),
        message: e.to_string(),
    })?;

    let stdin_thread = stdin_content.map(|content| {
        let mut stdin = child.stdin.take().expect("stdin should be piped");
        let content = content.to_vec();
        std::thread::spawn(move || stdin.write_all(&content))
    });

    let output = child
        .wait_with_output()
        .map_err(|e| FilterError::CommandFailed {
            command: cmd_str.to_string(),
            message: e.to_string(),
        })?;

    if let Some(handle) = stdin_thread {
        let write_result = handle.join().map_err(|_| FilterError::CommandFailed {
            command: cmd_str.to_string(),
            message: "stdin writer thread panicked".to_string(),
        })?;
        write_result.map_err(|e| FilterError::CommandFailed {
            command: cmd_str.to_string(),
            message: e.to_string(),
        })?;
    }

    if !output.status.success() {
        return Err(FilterError::CommandFailed {
            command: cmd_str.to_string(),
            message: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    Ok(output.stdout)
}

/// Simple whitespace-splitting that respects no quoting. This
/// matches how Git interprets filter driver commands.
fn split_cmd(cmd: &str) -> (String, Vec<String>) {
    let mut parts = cmd.split_whitespace();
    let program = parts.next().unwrap_or(cmd).to_string();
    let args: Vec<String> = parts.map(ToString::to_string).collect();
    (program, args)
}

// ---------------------------------------------------------------------------
// FilterDriverCache
// ---------------------------------------------------------------------------

/// Lazy cache for filter drivers loaded from Git config.
pub(crate) struct FilterDriverCache {
    store: Arc<Store>,
    workspace_root: Option<PathBuf>,
    cache: Mutex<HashMap<String, Option<Arc<FilterDriver>>>>,
}

impl FilterDriverCache {
    pub fn with_workspace_root(store: Arc<Store>, workspace_root: PathBuf) -> Self {
        Self {
            store,
            workspace_root: Some(workspace_root),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Looks up a filter driver by name from Git config, caching the result.
    #[cfg(feature = "git")]
    pub fn get(&self, filter_name: &str) -> Option<Arc<FilterDriver>> {
        {
            let cache = self.cache.lock().ok()?;
            if let Some(driver) = cache.get(filter_name) {
                return driver.clone();
            }
        }

        let driver = load_filter_driver_from_gitconfig(
            &self.store,
            filter_name,
            self.workspace_root.as_deref(),
        );

        let mut cache = self.cache.lock().ok()?;
        cache
            .entry(filter_name.to_string())
            .or_insert_with(|| driver)
            .clone()
    }

    #[cfg(not(feature = "git"))]
    pub fn get(&self, _filter_name: &str) -> Option<Arc<FilterDriver>> {
        None
    }
}

/// Loads a single filter driver from the Git config.
///
/// Reads `filter.<name>.process`, `filter.<name>.clean`, and
/// `filter.<name>.smudge`. If `process` is not set but the filter is
/// `lfs` with `clean`/`smudge` commands, automatically uses
/// `git-lfs filter-process` as the process command, since git-lfs
/// supports the process protocol even when not explicitly configured.
#[cfg(feature = "git")]
fn load_filter_driver_from_gitconfig(
    store: &Store,
    filter_name: &str,
    workspace_root: Option<&Path>,
) -> Option<Arc<FilterDriver>> {
    use crate::git::get_git_repo;

    use bstr::BStr;

    let git_repo = get_git_repo(store).ok()?;
    let config = git_repo.config_snapshot();

    let process = config
        .string_by("filter", Some(BStr::new(filter_name)), "process")
        .map(|v| v.to_string());
    let clean = config
        .string_by("filter", Some(BStr::new(filter_name)), "clean")
        .map(|v| v.to_string());
    let smudge = config
        .string_by("filter", Some(BStr::new(filter_name)), "smudge")
        .map(|v| v.to_string());
    let required = config
        .boolean_by("filter", Some(BStr::new(filter_name)), "required")
        .ok()
        .flatten()
        .unwrap_or(false);

    // If no process command is configured but clean/smudge are, check if
    // we can auto-detect a process command. Git LFS supports the process
    // protocol via `git-lfs filter-process` even when not explicitly
    // configured in git config.
    let process = process.or_else(|| {
        if clean.is_some() || smudge.is_some() {
            auto_detect_process_command(
                filter_name,
                clean.as_deref(),
                smudge.as_deref(),
            )
        } else {
            None
        }
    });

    if process.is_some() || clean.is_some() || smudge.is_some() {
        let mut driver = FilterDriver::new_with_required(
            clean, smudge, process, required,
        );
        if let Some(root) = workspace_root {
            driver.set_workspace_root(root.to_path_buf());
        }
        Some(Arc::new(driver))
    } else {
        None
    }
}

/// Auto-detects a process command for common filter drivers.
///
/// For `lfs` filters with `git-lfs clean/smudge` commands, returns
/// `git-lfs filter-process` which implements the Git filter process
/// protocol and is much more efficient than per-file spawning.
#[cfg(feature = "git")]
fn auto_detect_process_command(
    filter_name: &str,
    clean: Option<&str>,
    smudge: Option<&str>,
) -> Option<String> {
    // Only auto-detect for the "lfs" filter
    if filter_name != "lfs" {
        return None;
    }
    // Check if the clean/smudge commands use git-lfs
    let is_git_lfs = |cmd: Option<&str>| {
        cmd.is_some_and(|c| c.starts_with("git-lfs") || c.contains("git lfs"))
    };
    if is_git_lfs(clean) || is_git_lfs(smudge) {
        tracing::debug!(
            filter_name,
            "auto-detected git-lfs process mode; using 'git-lfs filter-process' instead of per-file spawning"
        );
        Some("git-lfs filter-process".to_string())
    } else {
        None
    }
}

/// Errors from filter operations.
#[derive(Clone, Debug, thiserror::Error)]
pub enum FilterError {
    /// No clean command is configured for this filter.
    #[error("No clean command configured")]
    NoCleanCommand,
    /// No smudge command is configured for this filter.
    #[error("No smudge command configured")]
    NoSmudgeCommand,
    /// Running the filter command failed.
    #[error("Filter command `{command}` failed: {message}")]
    CommandFailed {
        /// The command that was run.
        command: String,
        /// The error message.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared temp directory for filter test scripts, created once per test run.
    fn test_temp_dir() -> &'static std::path::Path {
        static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        DIR.get_or_init(|| {
            let dir = tempfile::tempdir().expect("failed to create test temp dir");
            let path = dir.path().to_path_buf();
            std::mem::forget(dir);
            path
        })
    }

    // --- pkt-line tests ---

    #[test]
    fn test_write_read_pkt_line() {
        let mut buf = Vec::new();
        write_pkt_line(&mut buf, b"hello").unwrap();
        write_flush(&mut buf).unwrap();

        let mut reader = BufReader::new(&buf[..]);
        let line = read_pkt_line(&mut reader).unwrap();
        assert_eq!(line, Some(b"hello".to_vec()));
        let flush = read_pkt_line(&mut reader).unwrap();
        assert_eq!(flush, None);
    }

    #[test]
    fn test_write_read_pkt_line_empty_data() {
        let mut buf = Vec::new();
        write_pkt_line(&mut buf, b"").unwrap();
        write_flush(&mut buf).unwrap();

        let mut reader = BufReader::new(&buf[..]);
        let line = read_pkt_line(&mut reader).unwrap();
        assert_eq!(line, Some(Vec::new()));
        let flush = read_pkt_line(&mut reader).unwrap();
        assert_eq!(flush, None);
    }

    #[test]
    fn test_write_read_large_pkt_line() {
        let data = vec![b'x'; PKT_LINE_MAX_DATA];
        let mut buf = Vec::new();
        write_pkt_line(&mut buf, &data).unwrap();
        write_flush(&mut buf).unwrap();

        let mut reader = BufReader::new(&buf[..]);
        let line = read_pkt_line(&mut reader).unwrap();
        assert_eq!(line, Some(data));
    }

    #[test]
    fn test_write_read_pkt_line_boundary_plus_one() {
        // Content exactly at the boundary should be one pkt-line;
        // one byte over should be split into two chunks.
        let mut buf = Vec::new();
        // Exactly PKT_LINE_MAX_DATA
        let data_at_boundary = vec![b'A'; PKT_LINE_MAX_DATA];
        write_pkt_line(&mut buf, &data_at_boundary).unwrap();
        // One byte over → should still be a single pkt-line (fits within limit)
        let data_one_over = vec![b'B'; PKT_LINE_MAX_DATA + 1];
        assert!(
            data_one_over.len() + 4 > PKT_LINE_MAX_LEN,
            "data + header should exceed max pkt-line length"
        );
        // The write_pkt_line function itself can't handle data > PKT_LINE_MAX_DATA,
        // so the caller (filter()) chunks it. Test that chunking works:
        let content = vec![b'C'; PKT_LINE_MAX_DATA + 1];
        for chunk in content.chunks(PKT_LINE_MAX_DATA) {
            write_pkt_line(&mut buf, chunk).unwrap();
        }
        write_flush(&mut buf).unwrap();

        let mut reader = BufReader::new(&buf[..]);
        // First: boundary-sized pkt-line
        let line1 = read_pkt_line(&mut reader).unwrap().unwrap();
        assert_eq!(line1, data_at_boundary);
        // Second: first chunk of the over-boundary content
        let line2 = read_pkt_line(&mut reader).unwrap().unwrap();
        assert_eq!(line2.len(), PKT_LINE_MAX_DATA);
        assert_eq!(&line2[..], &content[..PKT_LINE_MAX_DATA]);
        // Third: remaining 1 byte
        let line3 = read_pkt_line(&mut reader).unwrap().unwrap();
        assert_eq!(line3, vec![b'C'; 1]);
        // Flush
        let flush = read_pkt_line(&mut reader).unwrap();
        assert_eq!(flush, None);
    }

    #[test]
    fn test_read_pkt_line_invalid_length() {
        let buf = b"GGGGhello";
        let mut reader = BufReader::new(&buf[..]);
        let result = read_pkt_line(&mut reader);
        assert!(result.is_err());
    }

    // --- split_cmd / substitute_path tests ---

    #[test]
    fn test_split_cmd_simple() {
        let (prog, args) = split_cmd("git-lfs clean -- %f");
        assert_eq!(prog, "git-lfs");
        assert_eq!(args, ["clean", "--", "%f"]);
    }

    #[test]
    fn test_split_cmd_no_args() {
        let (prog, args) = split_cmd("cat");
        assert_eq!(prog, "cat");
        assert!(args.is_empty());
    }

    #[test]
    fn test_split_cmd_empty() {
        let (prog, args) = split_cmd("");
        assert_eq!(prog, "");
        assert!(args.is_empty());
    }

    #[test]
    fn test_split_cmd_whitespace_only() {
        let (prog, args) = split_cmd("   ");
        assert_eq!(prog, "   ");
        assert!(args.is_empty());
    }

    #[test]
    fn test_substitute_path_in_args() {
        let path = std::path::Path::new("/home/user/file.bin");
        let (program, mut args) = split_cmd("git-lfs clean -- %f");
        let path_str = path.to_string_lossy();
        for arg in &mut args {
            if arg.contains("%f") {
                *arg = arg.replace("%f", &path_str);
            }
        }
        assert_eq!(program, "git-lfs");
        assert_eq!(args, vec!["clean", "--", "/home/user/file.bin"]);
    }

    #[test]
    fn test_substitute_path_multiple_in_args() {
        let path = std::path::Path::new("/tmp/foo.bin");
        let (_program, mut args) = split_cmd("cmd %f %f");
        let path_str = path.to_string_lossy();
        for arg in &mut args {
            if arg.contains("%f") {
                *arg = arg.replace("%f", &path_str);
            }
        }
        assert_eq!(args, vec!["/tmp/foo.bin", "/tmp/foo.bin"]);
    }

    #[test]
    fn test_substitute_path_no_placeholder_in_args() {
        let (_program, mut args) = split_cmd("cat");
        let path = std::path::Path::new("/tmp/foo.bin");
        let path_str = path.to_string_lossy();
        for arg in &mut args {
            if arg.contains("%f") {
                *arg = arg.replace("%f", &path_str);
            }
        }
        assert!(args.is_empty());
    }

    #[test]
    fn test_substitute_path_with_spaces() {
        // Path with spaces should be preserved as a single argument
        let path = std::path::Path::new("/home/user/my file.bin");
        let (program, mut args) = split_cmd("git-lfs clean -- %f");
        let path_str = path.to_string_lossy();
        for arg in &mut args {
            if arg.contains("%f") {
                *arg = arg.replace("%f", &path_str);
            }
        }
        assert_eq!(program, "git-lfs");
        assert_eq!(args, vec!["clean", "--", "/home/user/my file.bin"]);
    }

    // --- per-file FilterDriver tests ---

    #[test]
    fn test_filter_driver_no_clean_command() {
        let driver = FilterDriver::new(None, None, None);
        let result = driver.clean(b"test", Path::new("/test"), "test");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FilterError::NoCleanCommand));
    }

    #[test]
    fn test_filter_driver_no_smudge_command() {
        let driver = FilterDriver::new(None, None, None);
        let result = driver.smudge(b"test", Path::new("/test"), "test");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FilterError::NoSmudgeCommand));
    }

    #[test]
    fn test_filter_driver_clean_with_cat() {
        let driver = FilterDriver::new(Some("cat %f".to_string()), None, None);

        let file_path = test_temp_dir().join("test.txt");
        std::fs::write(&file_path, b"hello world").unwrap();

        let result = driver.clean(b"hello world", &file_path, "hello.txt").unwrap();
        assert_eq!(result, b"hello world");
    }

    #[test]
    fn test_filter_driver_smudge_with_cat() {
        let driver = FilterDriver::new(None, Some("cat".to_string()), None);

        let result = driver
            .smudge(b"pointer content", Path::new("/test"), "test")
            .unwrap();
        assert_eq!(result, b"pointer content");
    }

    #[test]
    fn test_filter_driver_clean_command_not_found() {
        let driver = FilterDriver::new(Some("nonexistent_command_12345 %f".to_string()), None, None);

        let result = driver.clean(b"test", Path::new("/test"), "test");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FilterError::CommandFailed { .. }
        ));
    }

    #[test]
    fn test_has_clean_has_smudge() {
        let driver = FilterDriver::new(Some("cat".to_string()), None, None);
        assert!(driver.has_clean());
        assert!(!driver.has_smudge());

        let driver = FilterDriver::new(None, Some("cat".to_string()), None);
        assert!(!driver.has_clean());
        assert!(driver.has_smudge());

        let driver = FilterDriver::new(None, None, Some("cat".to_string()));
        assert!(driver.has_clean());
        assert!(driver.has_smudge());

        let driver = FilterDriver::new(None, None, None);
        assert!(!driver.has_clean());
        assert!(!driver.has_smudge());
    }

    // --- run_cmd tests ---

    #[test]
    fn test_run_cmd_echo() {
        let result = run_cmd("echo hello", None).unwrap();
        assert_eq!(String::from_utf8_lossy(&result).trim(), "hello");
    }

    #[test]
    fn test_run_cmd_with_stdin() {
        let result = run_cmd("cat", Some(b"hello from stdin")).unwrap();
        assert_eq!(result, b"hello from stdin");
    }

    #[test]
    fn test_run_cmd_failure() {
        let result = run_cmd("false", None);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FilterError::CommandFailed { .. }
        ));
    }

    #[test]
    fn test_run_cmd_large_stdin_no_deadlock() {
        let content = vec![b'x'; 10 * 1024 * 1024]; // 10 MiB
        let result = run_cmd("cat", Some(&content)).unwrap();
        assert_eq!(result.len(), content.len());
        assert_eq!(result, content);
    }

    #[test]
    fn test_run_cmd_empty_command() {
        let result = run_cmd("", None);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FilterError::CommandFailed { .. }
        ));
    }

    // --- FilterProcess integration tests ---
    //
    // These tests use a Python script that implements the Git filter
    // process protocol as an identity filter (returns input unchanged).
    // Python is used because it handles binary I/O correctly, unlike
    // shell scripts which struggle with binary pkt-line data.

    /// Python script that implements the Git filter process protocol.
    /// Acts as an identity filter: returns input content unchanged.
    ///
    /// Implements the two-phase exchange per file:
    /// 1. Read headers + flush → send status + flush
    /// 2. Read content + flush → send filtered content + flush → send final status + flush
    const FILTER_PROCESS_PYTHON: &str = r#"
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
    # Split large data into chunks of at most 65516 bytes
    CHUNK = 65516
    for i in range(0, len(data), CHUNK):
        chunk = data[i:i+CHUNK]
        length = len(chunk) + 4
        sys.stdout.buffer.write(f"{length:04x}".encode() + chunk)
    sys.stdout.buffer.flush()

def write_flush():
    sys.stdout.buffer.write(b"0000")
    sys.stdout.buffer.flush()

# Handshake: client sends "git-filter-client", then versions list
read_pkt_line()  # git-filter-client
# Read versions list until flush
while True:
    line = read_pkt_line()
    if line is None:
        break
# Respond with server info list
write_pkt_line(b"git-filter-server")
write_pkt_line(b"version=2")
write_flush()
# Read client capabilities list until flush
while True:
    line = read_pkt_line()
    if line is None:
        break
# Respond with selected capabilities
write_pkt_line(b"capability=clean")
write_pkt_line(b"capability=smudge")
write_flush()

# Main loop: two-phase exchange per file
while True:
    # Phase 1: read headers until flush
    command = read_pkt_line()
    if command is None:
        break
    # Read remaining headers (pathname, etc.) until flush
    while True:
        line = read_pkt_line()
        if line is None:
            break
    # Respond with status
    write_pkt_line(b"status=success")
    write_flush()

    # Phase 2: read content until flush
    content = b""
    while True:
        chunk = read_pkt_line()
        if chunk is None:
            break
        content += chunk
    # Respond with filtered content (identity: return input unchanged)
    if content:
        write_pkt_line(content)
    write_flush()
    # Send final status
    write_pkt_line(b"status=success")
    write_flush()
"#;

    /// Writes the Python filter process script to a temp file and returns its path.
    fn write_test_filter_script() -> std::path::PathBuf {
        let script_path = test_temp_dir().join("filter_process.py");
        std::fs::write(&script_path, FILTER_PROCESS_PYTHON).unwrap();
        script_path
    }

    /// Returns the Python 3 command, panicking if Python is not available.
    /// Filter process tests require Python to run the mock filter process.
    fn require_python() -> String {
        let python = std::env::var("PYTHON3").unwrap_or_else(|_| "python3".to_string());
        let result = std::process::Command::new(&python)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match result {
            Ok(status) if status.success() => python,
            Ok(status) => panic!(
                "Python 3 is required for filter process tests: `{python} --version` exited with {status}"
            ),
            Err(e) => panic!(
                "Python 3 is required for filter process tests: failed to run `{python}`: {e}"
            ),
        }
    }

    #[test]
    fn test_filter_process_clean_identity() {
        let python = require_python();
        let script_path = write_test_filter_script();
        let driver = FilterDriver::new(
            None,
            None,
            Some(format!("{python} {}", script_path.display())),
        );

        let content = b"hello world";
        let result = driver.clean(content, Path::new("/test/file.bin"), "file.bin");
        assert_eq!(result.expect("filter process clean failed"), content);
    }

    #[test]
    fn test_filter_process_smudge_identity() {
        let python = require_python();
        let script_path = write_test_filter_script();
        let driver = FilterDriver::new(
            None,
            None,
            Some(format!("{python} {}", script_path.display())),
        );

        let content = b"smudge me";
        let result = driver.smudge(content, Path::new("/test/file.bin"), "file.bin");
        assert_eq!(result.expect("filter process smudge failed"), content);
    }

    #[test]
    fn test_filter_process_multiple_calls_reuse() {
        let python = require_python();
        let script_path = write_test_filter_script();
        let driver = FilterDriver::new(
            None,
            None,
            Some(format!("{python} {}", script_path.display())),
        );

        // First call should start the process; second call should reuse it
        let content1 = b"first file content";
        let content2 = b"second file content";

        let result1 = driver.clean(content1, Path::new("/test/file1.bin"), "file1.bin");
        let result2 = driver.clean(content2, Path::new("/test/file2.bin"), "file2.bin");

        assert_eq!(result1.expect("first clean failed"), content1);
        assert_eq!(result2.expect("second clean failed"), content2);
    }

    #[test]
    fn test_filter_process_large_content() {
        let python = require_python();
        let script_path = write_test_filter_script();
        let driver = FilterDriver::new(
            None,
            None,
            Some(format!("{python} {}", script_path.display())),
        );

        // Content larger than a single pkt-line (65516 bytes)
        let content = vec![b'x'; 200_000];
        let result = driver.clean(&content, Path::new("/test/large.bin"), "large.bin");
        assert_eq!(result.expect("filter process large content failed"), content);
    }

    #[test]
    fn test_filter_process_empty_content() {
        let python = require_python();
        let script_path = write_test_filter_script();
        let driver = FilterDriver::new(
            None,
            None,
            Some(format!("{python} {}", script_path.display())),
        );

        let result = driver.clean(b"", Path::new("/test/empty.bin"), "empty.bin");
        assert_eq!(result.expect("filter process empty content failed"), b"");
    }

    #[test]
    fn test_filter_process_fallback_on_failure() {
        // Process command that doesn't exist — should fall back to per-file mode
        let driver = FilterDriver::new(
            Some("cat".to_string()),
            None,
            Some("nonexistent_filter_process_12345".to_string()),
        );

        // Should fall back to per-file cat command
        let result = driver.clean(b"hello", Path::new("/test"), "test").unwrap();
        assert_eq!(result, b"hello");
    }

    #[test]
    fn test_filter_process_smudge_fallback_on_failure() {
        let driver = FilterDriver::new(
            None,
            Some("cat".to_string()),
            Some("nonexistent_filter_process_12345".to_string()),
        );

        let result = driver.smudge(b"hello", Path::new("/test"), "test").unwrap();
        assert_eq!(result, b"hello");
    }

    #[test]
    fn test_filter_process_error_no_fallback() {
        // Process command fails and no per-file clean command to fall back to.
        // Should return the process error, not NoCleanCommand.
        let driver = FilterDriver::new(
            None,
            None,
            Some("nonexistent_filter_process_12345".to_string()),
        );

        let result = driver.clean(b"hello", Path::new("/test"), "test");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            FilterError::CommandFailed { message, .. } => {
                assert!(
                    message.contains("nonexistent_filter_process_12345")
                        || message.contains("No such file"),
                    "error should mention the failed command, got: {message}"
                );
            }
            FilterError::NoCleanCommand => {
                panic!("should not return NoCleanCommand when process_command was configured");
            }
            _ => panic!("unexpected error: {err}"),
        }
    }

    #[test]
    fn test_filter_process_smudge_error_no_fallback() {
        // Process command fails and no per-file smudge command to fall back to.
        let driver = FilterDriver::new(
            None,
            None,
            Some("nonexistent_filter_process_12345".to_string()),
        );

        let result = driver.smudge(b"hello", Path::new("/test"), "test");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            FilterError::CommandFailed { message, .. } => {
                assert!(
                    message.contains("nonexistent_filter_process_12345")
                        || message.contains("No such file"),
                    "error should mention the failed command, got: {message}"
                );
            }
            FilterError::NoSmudgeCommand => {
                panic!("should not return NoSmudgeCommand when process_command was configured");
            }
            _ => panic!("unexpected error: {err}"),
        }
    }

    #[test]
    fn test_filter_process_startup_failure_cached() {
        // When the process command doesn't exist, the startup failure should
        // be cached so subsequent calls don't repeatedly try to spawn.
        let driver = FilterDriver::new(
            Some("cat".to_string()),
            None,
            Some("nonexistent_filter_process_12345".to_string()),
        );

        // First call: tries to start process, fails, falls back to cat.
        let result1 = driver.clean(b"hello", Path::new("/test/file1.bin"), "file1.bin").unwrap();
        assert_eq!(result1, b"hello");

        // Second call: should use cached failure, not try to spawn again.
        let result2 = driver.clean(b"world", Path::new("/test/file2.bin"), "file2.bin").unwrap();
        assert_eq!(result2, b"world");

        // Verify the cached error is returned (not NoCleanCommand) when
        // there's no per-file fallback.
        let driver_no_fallback = FilterDriver::new(
            None,
            None,
            Some("nonexistent_filter_process_12345".to_string()),
        );
        let err1 = driver_no_fallback
            .clean(b"a", Path::new("/test/a"), "a")
            .unwrap_err();
        let err2 = driver_no_fallback
            .clean(b"b", Path::new("/test/b"), "b")
            .unwrap_err();
        // Both errors should be CommandFailed (cached), not NoCleanCommand.
        assert!(matches!(err1, FilterError::CommandFailed { .. }));
        assert!(matches!(err2, FilterError::CommandFailed { .. }));
    }

    #[test]
    fn test_filter_process_crash_recovery() {
        // Use a script that exits after the first request, simulating a crash.
        // The driver should reset the process and fall back to per-file mode.
        let python = require_python();
        let script_content = r#"
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
    length = len(data) + 4
    sys.stdout.buffer.write(f"{length:04x}".encode() + data)
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

# Handle one request (two-phase), then exit (simulating crash)
# Phase 1: read headers until flush
command = read_pkt_line()
if command is None:
    sys.exit(0)
while True:
    line = read_pkt_line()
    if line is None:
        break
write_pkt_line(b"status=success")
write_flush()
# Phase 2: read content until flush
content = b""
while True:
    chunk = read_pkt_line()
    if chunk is None:
        break
    content += chunk
if content:
    write_pkt_line(content)
write_flush()
write_pkt_line(b"status=success")
write_flush()
# Exit immediately after first request
sys.exit(0)
"#;
        let dir = test_temp_dir();
        let script_path = dir.join("crash_filter.py");
        std::fs::write(&script_path, script_content).unwrap();

        let driver = FilterDriver::new(
            Some("cat".to_string()),
            None,
            Some(format!("{python} {}", script_path.display())),
        );

        // First call should succeed (process handles it)
        let result1 = driver.clean(b"first", Path::new("/test/file1.bin"), "file1.bin");
        assert_eq!(result1.expect("first clean via process failed"), b"first");

        // Second call should fail (process exited), fall back to per-file cat
        let result2 = driver.clean(b"second", Path::new("/test/file2.bin"), "file2.bin").unwrap();
        assert_eq!(result2, b"second");
    }

    #[test]
    fn test_filter_process_error_status() {
        // Use a Python script that returns status=error in phase 1.
        // The driver should fall back to per-file mode.
        let python = require_python();
        let script_content = r#"
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
    length = len(data) + 4
    sys.stdout.buffer.write(f"{length:04x}".encode() + data)
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

# Main loop: always return error in phase 1
while True:
    command = read_pkt_line()
    if command is None:
        break
    while True:
        line = read_pkt_line()
        if line is None:
            break
    write_pkt_line(b"status=error")
    write_pkt_line(b"simulated filter error")
    write_flush()
"#;
        let dir = test_temp_dir();
        let script_path = dir.join("error_filter.py");
        std::fs::write(&script_path, script_content).unwrap();

        let driver = FilterDriver::new(
            Some("cat".to_string()),
            None,
            Some(format!("{python} {}", script_path.display())),
        );

        // Process returns error → should fall back to per-file cat
        let result = driver.clean(b"fallback content", Path::new("/test/file.bin"), "file.bin");
        assert_eq!(result.expect("fallback to per-file mode failed"), b"fallback content");
    }

    // --- auto_detect_process_command tests ---

    #[test]
    fn test_auto_detect_lfs_process() {
        let result = auto_detect_process_command(
            "lfs",
            Some("git-lfs clean -- %f"),
            Some("git-lfs smudge -- %f"),
        );
        assert_eq!(result.as_deref(), Some("git-lfs filter-process"));
    }

    #[test]
    fn test_auto_detect_lfs_process_only_clean() {
        let result =
            auto_detect_process_command("lfs", Some("git-lfs clean -- %f"), None);
        assert_eq!(result.as_deref(), Some("git-lfs filter-process"));
    }

    #[test]
    fn test_auto_detect_non_lfs_filter() {
        let result = auto_detect_process_command(
            "custom",
            Some("my-tool clean"),
            Some("my-tool smudge"),
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_auto_detect_lfs_with_non_git_lfs_commands() {
        let result = auto_detect_process_command("lfs", Some("my-custom-clean"), None);
        assert!(result.is_none());
    }

    #[test]
    fn test_auto_detect_lfs_no_commands() {
        let result = auto_detect_process_command("lfs", None, None);
        assert!(result.is_none());
    }
}
