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

#![expect(missing_docs)]

use std::path::Path;
use std::path::PathBuf;

use crate::backend::CommitId;
use crate::object_id::ObjectId as _;
use crate::submodule_store::SubmoduleStore;
use crate::submodule_store::SubmoduleStoreError;

#[derive(Debug)]
pub struct DefaultSubmoduleStore {
    store_path: PathBuf,
    git_executable: PathBuf,
}

impl DefaultSubmoduleStore {
    pub fn load(store_path: &Path) -> Self {
        Self {
            store_path: store_path.to_path_buf(),
            git_executable: PathBuf::from("git"),
        }
    }

    pub fn init(store_path: &Path) -> Self {
        std::fs::create_dir_all(store_path).ok();
        Self {
            store_path: store_path.to_path_buf(),
            git_executable: PathBuf::from("git"),
        }
    }

    pub fn name() -> &'static str {
        "default"
    }

    fn submodules_dir(&self) -> PathBuf {
        self.store_path.join("submodules")
    }

    fn bare_repo_dir(&self, name: &str) -> PathBuf {
        self.submodules_dir().join(name)
    }

    /// Validates a submodule name to prevent path traversal.
    /// A valid submodule name must not contain path separators or `..`.
    fn validate_submodule_name(name: &str) -> Result<(), SubmoduleStoreError> {
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name == ".."
            || name.contains(std::path::MAIN_SEPARATOR)
        {
            return Err(SubmoduleStoreError::Other(format!(
                "invalid submodule name: {name:?}"
            )));
        }
        Ok(())
    }
}

impl SubmoduleStore for DefaultSubmoduleStore {
    fn name(&self) -> &str {
        Self::name()
    }

    fn ensure_bare_repo(&self, name: &str, url: &str) -> Result<(), SubmoduleStoreError> {
        Self::validate_submodule_name(name)?;
        let target_dir = self.bare_repo_dir(name);
        if target_dir.exists() {
            return Ok(());
        }
        let parent = self.submodules_dir();
        std::fs::create_dir_all(&parent).map_err(|e| {
            SubmoduleStoreError::Other(format!("failed to create submodule store dir: {e}"))
        })?;
        let output = std::process::Command::new(&self.git_executable)
            .arg("clone")
            .arg("--bare")
            .arg("--")
            .arg(url)
            .arg(&target_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| {
                SubmoduleStoreError::Other(format!("failed to run git clone: {e}"))
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SubmoduleStoreError::Other(format!(
                "git clone failed: {stderr}"
            )));
        }
        Ok(())
    }

    fn fetch(&self, name: &str, url: &str) -> Result<(), SubmoduleStoreError> {
        Self::validate_submodule_name(name)?;
        let target_dir = self.bare_repo_dir(name);
        if !target_dir.exists() {
            return self.ensure_bare_repo(name, url);
        }
        let target_dir_str = target_dir.to_str().ok_or_else(|| {
            SubmoduleStoreError::Other(format!(
                "submodule bare repo path contains non-UTF-8 characters: {}",
                target_dir.display()
            ))
        })?;
        let output = std::process::Command::new(&self.git_executable)
            .args([
                "--git-dir",
                target_dir_str,
                "fetch",
                "--",
                url,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| {
                SubmoduleStoreError::Other(format!("failed to run git fetch: {e}"))
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SubmoduleStoreError::Other(format!(
                "git fetch failed: {stderr}"
            )));
        }
        Ok(())
    }

    fn checkout(
        &self,
        name: &str,
        commit_id: &CommitId,
        working_copy_path: &Path,
    ) -> Result<bool, SubmoduleStoreError> {
        Self::validate_submodule_name(name)?;
        let bare_dir = self.bare_repo_dir(name);
        if !bare_dir.exists() {
            return Ok(false);
        }
        let commit_hex = commit_id.hex().clone();
        let bare_dir_str = bare_dir.to_str().ok_or_else(|| {
            SubmoduleStoreError::Other(format!(
                "submodule bare repo path contains non-UTF-8 characters: {}",
                bare_dir.display()
            ))
        })?;
        let working_copy_str = working_copy_path.to_str().ok_or_else(|| {
            SubmoduleStoreError::Other(format!(
                "submodule working copy path contains non-UTF-8 characters: {}",
                working_copy_path.display()
            ))
        })?;
        let output = std::process::Command::new(&self.git_executable)
            .args([
                "--git-dir",
                bare_dir_str,
                "worktree",
                "add",
                "-f",
                "--",
                working_copy_str,
                &commit_hex,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| {
                SubmoduleStoreError::Other(format!("failed to run git worktree add: {e}"))
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(
                name = name,
                commit = %commit_hex,
                stderr = %stderr,
                "git worktree add failed for submodule"
            );
        }
        Ok(output.status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_submodule_name_valid() {
        assert!(DefaultSubmoduleStore::validate_submodule_name("foo").is_ok());
        assert!(DefaultSubmoduleStore::validate_submodule_name("my-submodule").is_ok());
        assert!(DefaultSubmoduleStore::validate_submodule_name("sub.module").is_ok());
    }

    #[test]
    fn test_validate_submodule_name_rejects_traversal() {
        assert!(DefaultSubmoduleStore::validate_submodule_name("").is_err());
        assert!(DefaultSubmoduleStore::validate_submodule_name("..").is_err());
        assert!(DefaultSubmoduleStore::validate_submodule_name("../foo").is_err());
        assert!(DefaultSubmoduleStore::validate_submodule_name("foo/../bar").is_err());
        assert!(DefaultSubmoduleStore::validate_submodule_name("foo/bar").is_err());
        assert!(DefaultSubmoduleStore::validate_submodule_name(r"foo\bar").is_err());
    }
}