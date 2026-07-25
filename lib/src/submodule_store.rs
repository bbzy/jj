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

use std::fmt::Debug;

use crate::backend::CommitId;

#[derive(Debug, thiserror::Error)]
pub enum SubmoduleStoreError {
    #[error("Submodule action failed: {0}")]
    Other(String),
}

pub trait SubmoduleStore: Send + Sync + Debug {
    fn name(&self) -> &str;

    fn ensure_bare_repo(&self, name: &str, url: &str) -> Result<(), SubmoduleStoreError>;

    fn fetch(&self, name: &str, url: &str) -> Result<(), SubmoduleStoreError>;

    fn checkout(
        &self,
        name: &str,
        commit_id: &CommitId,
        working_copy_path: &std::path::Path,
    ) -> Result<bool, SubmoduleStoreError>;
}