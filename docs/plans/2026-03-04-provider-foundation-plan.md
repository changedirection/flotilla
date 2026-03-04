# Provider Foundation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Create the trait definitions, shared data types, correlation engine, provider registry, and discovery stubs — all additive, no existing code modified.

**Architecture:** New `src/providers/` module tree with async traits behind `async-trait`, an `IndexMap`-based registry, and a union-find correlation engine. Everything compiles alongside existing code without touching it.

**Tech Stack:** Rust 2021, async-trait, indexmap, tokio, serde

---

### Task 1: Add dependencies

**Files:**
- Modify: `Cargo.toml`

**Step 1: Add async-trait and indexmap to Cargo.toml**

Add after the `toml = "0.8"` line:

```toml
async-trait = "0.1"
indexmap = "2"
```

**Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add async-trait and indexmap dependencies"
```

---

### Task 2: Create shared types and CorrelationKey

**Files:**
- Create: `src/providers/mod.rs`
- Create: `src/providers/types.rs`
- Modify: `src/main.rs` (add `mod providers;`)

**Step 1: Create the providers module directory**

Run: `mkdir -p src/providers`

**Step 2: Create `src/providers/types.rs` with shared data types**

```rust
use std::collections::HashMap;
use std::path::PathBuf;

// -- Correlation --

/// Typed keys emitted by providers for cross-provider linking.
/// Items sharing a CorrelationKey value are grouped into the same WorkItem.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CorrelationKey {
    /// Branch name — cross-cutting, no source qualifier needed.
    Branch(String),
    /// Repository path — cross-cutting.
    RepoPath(PathBuf),
    /// Issue reference: (provider_name, issue_id).
    IssueRef(String, String),
    /// Change request reference: (provider_name, CR id).
    ChangeRequestRef(String, String),
    /// Session reference: (provider_name, session_id).
    SessionRef(String, String),
}

// -- Source Control types --

#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub is_trunk: bool,
}

#[derive(Debug, Clone)]
pub struct Checkout {
    pub branch: String,
    pub path: PathBuf,
    pub is_trunk: bool,
    pub correlation_keys: Vec<CorrelationKey>,
}

#[derive(Debug, Clone)]
pub struct AheadBehind {
    pub ahead: i64,
    pub behind: i64,
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub short_sha: String,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct WorkingTreeStatus {
    pub staged: usize,
    pub modified: usize,
    pub untracked: usize,
}

// -- Remote Platform types --

#[derive(Debug, Clone)]
pub struct ChangeRequest {
    pub id: String,
    pub title: String,
    pub branch: String,
    pub status: ChangeRequestStatus,
    pub body: Option<String>,
    pub correlation_keys: Vec<CorrelationKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeRequestStatus {
    Open,
    Draft,
    Merged,
    Closed,
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub id: String,
    pub title: String,
    pub labels: Vec<String>,
    pub correlation_keys: Vec<CorrelationKey>,
}

// -- AI types --

#[derive(Debug, Clone)]
pub struct CloudAgentSession {
    pub id: String,
    pub title: String,
    pub status: SessionStatus,
    pub model: Option<String>,
    pub correlation_keys: Vec<CorrelationKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Idle,
    Archived,
}

// -- Workspace types --

#[derive(Debug, Clone)]
pub struct Workspace {
    /// Opaque handle passed back to select_workspace().
    pub ws_ref: String,
    pub name: String,
    pub directories: Vec<PathBuf>,
    pub correlation_keys: Vec<CorrelationKey>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceConfig {
    pub name: String,
    pub working_directory: PathBuf,
    pub template_vars: HashMap<String, String>,
    /// Opaque template data — each WorkspaceManager implementation interprets this.
    pub template_yaml: Option<String>,
}
```

**Step 3: Create `src/providers/mod.rs`**

```rust
pub mod types;
```

**Step 4: Add `mod providers` to `src/main.rs`**

Add after `mod config;` (line 7):

```rust
mod providers;
```

**Step 5: Verify it compiles**

Run: `cargo check`
Expected: compiles (types may warn as unused — that's fine for now)

**Step 6: Commit**

```bash
git add src/providers/
git add src/main.rs
git commit -m "feat: add shared provider types and CorrelationKey"
```

---

### Task 3: Create Vcs and CheckoutManager traits

**Files:**
- Create: `src/providers/vcs/mod.rs`
- Modify: `src/providers/mod.rs`

**Step 1: Create vcs directory**

Run: `mkdir -p src/providers/vcs`

**Step 2: Create `src/providers/vcs/mod.rs`**

```rust
use std::path::Path;

use async_trait::async_trait;

use crate::providers::types::{
    AheadBehind, BranchInfo, Checkout, CommitInfo, WorkingTreeStatus,
};

#[async_trait]
pub trait Vcs: Send + Sync {
    /// Human-readable name for UI display (e.g. "Git", "Jujutsu").
    fn display_name(&self) -> &str;

    async fn list_local_branches(&self, repo_root: &Path) -> Result<Vec<BranchInfo>, String>;

    async fn list_remote_branches(&self, repo_root: &Path) -> Result<Vec<String>, String>;

    async fn commit_log(
        &self,
        repo_root: &Path,
        branch: &str,
        limit: usize,
    ) -> Result<Vec<CommitInfo>, String>;

    /// Commits ahead/behind between `branch` and `reference` (e.g. "main", "origin/feat").
    async fn ahead_behind(
        &self,
        repo_root: &Path,
        branch: &str,
        reference: &str,
    ) -> Result<AheadBehind, String>;

    async fn working_tree_status(
        &self,
        repo_root: &Path,
        checkout_path: &Path,
    ) -> Result<WorkingTreeStatus, String>;
}

#[async_trait]
pub trait CheckoutManager: Send + Sync {
    fn display_name(&self) -> &str;

    async fn list_checkouts(&self, repo_root: &Path) -> Result<Vec<Checkout>, String>;

    async fn create_checkout(&self, repo_root: &Path, branch: &str) -> Result<Checkout, String>;

    async fn remove_checkout(&self, repo_root: &Path, branch: &str) -> Result<(), String>;
}

/// A Vcs paired with its checkout manager, produced by a VcsFactory.
pub struct VcsBundle {
    pub vcs: Box<dyn Vcs>,
    pub checkout_manager: Box<dyn CheckoutManager>,
}
```

**Step 3: Add `pub mod vcs;` to `src/providers/mod.rs`**

```rust
pub mod types;
pub mod vcs;
```

**Step 4: Verify it compiles**

Run: `cargo check`
Expected: compiles

**Step 5: Commit**

```bash
git add src/providers/
git commit -m "feat: add Vcs and CheckoutManager traits"
```

---

### Task 4: Create CodeReview trait

**Files:**
- Create: `src/providers/code_review/mod.rs`
- Modify: `src/providers/mod.rs`

**Step 1: Create directory and trait file**

Run: `mkdir -p src/providers/code_review`

**Step 2: Create `src/providers/code_review/mod.rs`**

```rust
use std::path::Path;

use async_trait::async_trait;

use crate::providers::types::ChangeRequest;

#[async_trait]
pub trait CodeReview: Send + Sync {
    fn display_name(&self) -> &str;

    async fn list_change_requests(
        &self,
        repo_root: &Path,
        limit: usize,
    ) -> Result<Vec<ChangeRequest>, String>;

    async fn get_change_request(
        &self,
        repo_root: &Path,
        id: &str,
    ) -> Result<ChangeRequest, String>;

    async fn open_in_browser(&self, repo_root: &Path, id: &str) -> Result<(), String>;
}
```

**Step 3: Add to `src/providers/mod.rs`**

```rust
pub mod types;
pub mod vcs;
pub mod code_review;
```

**Step 4: Verify and commit**

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: add CodeReview trait"
```

---

### Task 5: Create IssueTracker trait

**Files:**
- Create: `src/providers/issue_tracker/mod.rs`
- Modify: `src/providers/mod.rs`

**Step 1: Create directory and trait file**

Run: `mkdir -p src/providers/issue_tracker`

**Step 2: Create `src/providers/issue_tracker/mod.rs`**

```rust
use std::path::Path;

use async_trait::async_trait;

use crate::providers::types::Issue;

#[async_trait]
pub trait IssueTracker: Send + Sync {
    fn display_name(&self) -> &str;

    async fn list_issues(
        &self,
        repo_root: &Path,
        limit: usize,
    ) -> Result<Vec<Issue>, String>;

    async fn open_in_browser(&self, repo_root: &Path, id: &str) -> Result<(), String>;
}
```

**Step 3: Add to `src/providers/mod.rs` and verify**

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: add IssueTracker trait"
```

---

### Task 6: Create CodingAgent trait

**Files:**
- Create: `src/providers/coding_agent/mod.rs`
- Modify: `src/providers/mod.rs`

**Step 1: Create directory and trait file**

Run: `mkdir -p src/providers/coding_agent`

**Step 2: Create `src/providers/coding_agent/mod.rs`**

```rust
use async_trait::async_trait;

use crate::providers::types::CloudAgentSession;

#[async_trait]
pub trait CodingAgent: Send + Sync {
    fn display_name(&self) -> &str;

    async fn list_sessions(&self) -> Result<Vec<CloudAgentSession>, String>;

    async fn archive_session(&self, session_id: &str) -> Result<(), String>;

    /// Returns the CLI command to attach/teleport into a session.
    /// The app passes this to the workspace manager to run in a pane.
    async fn attach_command(&self, session_id: &str) -> Result<String, String>;
}
```

**Step 3: Add to `src/providers/mod.rs` and verify**

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: add CodingAgent trait"
```

---

### Task 7: Create AiUtility trait

**Files:**
- Create: `src/providers/ai_utility/mod.rs`
- Modify: `src/providers/mod.rs`

**Step 1: Create directory and trait file**

Run: `mkdir -p src/providers/ai_utility`

**Step 2: Create `src/providers/ai_utility/mod.rs`**

```rust
use async_trait::async_trait;

#[async_trait]
pub trait AiUtility: Send + Sync {
    fn display_name(&self) -> &str;

    /// Generate a branch name from issue context (titles, numbers, etc).
    async fn generate_branch_name(&self, context: &str) -> Result<String, String>;
}
```

**Step 3: Add to `src/providers/mod.rs` and verify**

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: add AiUtility trait"
```

---

### Task 8: Create WorkspaceManager trait

**Files:**
- Create: `src/providers/workspace/mod.rs`
- Modify: `src/providers/mod.rs`

**Step 1: Create directory and trait file**

Run: `mkdir -p src/providers/workspace`

**Step 2: Create `src/providers/workspace/mod.rs`**

```rust
use async_trait::async_trait;

use crate::providers::types::{Workspace, WorkspaceConfig};

#[async_trait]
pub trait WorkspaceManager: Send + Sync {
    fn display_name(&self) -> &str;

    async fn list_workspaces(&self) -> Result<Vec<Workspace>, String>;

    /// Create a workspace from config. The implementation interprets
    /// template_yaml and template_vars as appropriate.
    async fn create_workspace(&self, config: &WorkspaceConfig) -> Result<Workspace, String>;

    async fn select_workspace(&self, ws_ref: &str) -> Result<(), String>;
}
```

**Step 3: Add to `src/providers/mod.rs` and verify**

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: add WorkspaceManager trait"
```

---

### Task 9: Create ProviderRegistry

**Files:**
- Create: `src/providers/registry.rs`
- Modify: `src/providers/mod.rs`

**Step 1: Create `src/providers/registry.rs`**

```rust
use indexmap::IndexMap;

use crate::providers::ai_utility::AiUtility;
use crate::providers::code_review::CodeReview;
use crate::providers::coding_agent::CodingAgent;
use crate::providers::issue_tracker::IssueTracker;
use crate::providers::vcs::{CheckoutManager, Vcs};
use crate::providers::workspace::WorkspaceManager;

/// Named, ordered collection of all active providers.
///
/// Keys are provider names (e.g. "git", "github", "linear").
/// These names serve as config keys, correlation source identifiers,
/// and UI provenance labels.
pub struct ProviderRegistry {
    pub vcs: IndexMap<String, Box<dyn Vcs>>,
    pub checkout_managers: IndexMap<String, Box<dyn CheckoutManager>>,
    pub code_review: IndexMap<String, Box<dyn CodeReview>>,
    pub issue_trackers: IndexMap<String, Box<dyn IssueTracker>>,
    pub coding_agents: IndexMap<String, Box<dyn CodingAgent>>,
    pub ai_utilities: IndexMap<String, Box<dyn AiUtility>>,
    pub workspace_manager: Option<(String, Box<dyn WorkspaceManager>)>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            vcs: IndexMap::new(),
            checkout_managers: IndexMap::new(),
            code_review: IndexMap::new(),
            issue_trackers: IndexMap::new(),
            coding_agents: IndexMap::new(),
            ai_utilities: IndexMap::new(),
            workspace_manager: None,
        }
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 2: Add to `src/providers/mod.rs` and verify**

```rust
pub mod types;
pub mod vcs;
pub mod code_review;
pub mod issue_tracker;
pub mod coding_agent;
pub mod ai_utility;
pub mod workspace;
pub mod registry;
```

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: add ProviderRegistry"
```

---

### Task 10: Create correlation engine with tests

This is the most algorithmically interesting part. The correlation engine groups
items from different providers into unified work items using a union-find data
structure over CorrelationKey values.

**Files:**
- Create: `src/providers/correlation.rs`
- Modify: `src/providers/mod.rs`

**Step 1: Write the failing test**

Create `src/providers/correlation.rs` with types and tests first:

```rust
use std::collections::HashMap;

use crate::providers::types::CorrelationKey;

/// An item from any provider, carrying correlation keys.
#[derive(Debug, Clone)]
pub struct CorrelatedItem {
    /// Which provider produced this item (e.g. "github", "linear").
    pub provider_name: String,
    /// What kind of item this is.
    pub kind: ItemKind,
    /// Display text.
    pub title: String,
    /// Keys for cross-provider matching.
    pub correlation_keys: Vec<CorrelationKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemKind {
    Checkout,
    ChangeRequest,
    Issue,
    CloudSession,
    Workspace,
    RemoteBranch,
}

/// A group of correlated items from across providers.
#[derive(Debug, Clone)]
pub struct CorrelatedGroup {
    pub items: Vec<CorrelatedItem>,
}

impl CorrelatedGroup {
    /// The branch name shared by this group, if any.
    pub fn branch(&self) -> Option<&str> {
        self.items.iter().find_map(|item| {
            item.correlation_keys.iter().find_map(|k| match k {
                CorrelationKey::Branch(b) => Some(b.as_str()),
                _ => None,
            })
        })
    }

    /// Whether this group contains an item of the given kind.
    pub fn has(&self, kind: &ItemKind) -> bool {
        self.items.iter().any(|i| &i.kind == kind)
    }
}

/// Union-Find data structure for grouping items by shared keys.
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }

    fn union(&mut self, x: usize, y: usize) {
        let rx = self.find(x);
        let ry = self.find(y);
        if rx == ry {
            return;
        }
        if self.rank[rx] < self.rank[ry] {
            self.parent[rx] = ry;
        } else if self.rank[rx] > self.rank[ry] {
            self.parent[ry] = rx;
        } else {
            self.parent[ry] = rx;
            self.rank[rx] += 1;
        }
    }
}

/// Group items that share any CorrelationKey value.
///
/// Correlation is transitive: if A shares a key with B, and B shares a
/// different key with C, then A, B, and C end up in the same group.
pub fn correlate(items: Vec<CorrelatedItem>) -> Vec<CorrelatedGroup> {
    if items.is_empty() {
        return Vec::new();
    }

    let mut uf = UnionFind::new(items.len());

    // Map each key to the first item index that has it.
    // When a second item has the same key, union them.
    let mut key_to_item: HashMap<CorrelationKey, usize> = HashMap::new();

    for (i, item) in items.iter().enumerate() {
        for key in &item.correlation_keys {
            if let Some(&first) = key_to_item.get(key) {
                uf.union(first, i);
            } else {
                key_to_item.insert(key.clone(), i);
            }
        }
    }

    // Collect groups by root.
    let mut groups: HashMap<usize, Vec<CorrelatedItem>> = HashMap::new();
    for (i, item) in items.into_iter().enumerate() {
        let root = uf.find(i);
        groups.entry(root).or_default().push(item);
    }

    groups
        .into_values()
        .map(|items| CorrelatedGroup { items })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::types::CorrelationKey;

    fn item(provider: &str, kind: ItemKind, title: &str, keys: Vec<CorrelationKey>) -> CorrelatedItem {
        CorrelatedItem {
            provider_name: provider.to_string(),
            kind,
            title: title.to_string(),
            correlation_keys: keys,
        }
    }

    #[test]
    fn empty_input() {
        let groups = correlate(vec![]);
        assert!(groups.is_empty());
    }

    #[test]
    fn single_item_forms_own_group() {
        let items = vec![item(
            "git",
            ItemKind::Checkout,
            "main",
            vec![CorrelationKey::Branch("main".into())],
        )];
        let groups = correlate(items);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 1);
    }

    #[test]
    fn items_sharing_branch_are_grouped() {
        let items = vec![
            item("git", ItemKind::Checkout, "feat-x", vec![
                CorrelationKey::Branch("feat-x".into()),
            ]),
            item("github", ItemKind::ChangeRequest, "PR #5", vec![
                CorrelationKey::Branch("feat-x".into()),
                CorrelationKey::ChangeRequestRef("github".into(), "5".into()),
            ]),
        ];
        let groups = correlate(items);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(groups[0].branch(), Some("feat-x"));
    }

    #[test]
    fn transitive_correlation() {
        // Checkout links to PR via branch, PR links to issue via IssueRef.
        // Issue should end up in the same group as checkout even though
        // they share no direct key.
        let items = vec![
            item("git", ItemKind::Checkout, "feat-x", vec![
                CorrelationKey::Branch("feat-x".into()),
            ]),
            item("github", ItemKind::ChangeRequest, "PR #5", vec![
                CorrelationKey::Branch("feat-x".into()),
                CorrelationKey::IssueRef("linear".into(), "PROJ-43".into()),
            ]),
            item("linear", ItemKind::Issue, "PROJ-43", vec![
                CorrelationKey::IssueRef("linear".into(), "PROJ-43".into()),
            ]),
        ];
        let groups = correlate(items);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 3);
    }

    #[test]
    fn unrelated_items_stay_separate() {
        let items = vec![
            item("git", ItemKind::Checkout, "feat-a", vec![
                CorrelationKey::Branch("feat-a".into()),
            ]),
            item("git", ItemKind::Checkout, "feat-b", vec![
                CorrelationKey::Branch("feat-b".into()),
            ]),
            item("linear", ItemKind::Issue, "PROJ-99", vec![
                CorrelationKey::IssueRef("linear".into(), "PROJ-99".into()),
            ]),
        ];
        let groups = correlate(items);
        assert_eq!(groups.len(), 3);
    }

    #[test]
    fn no_correlation_keys_each_item_separate() {
        let items = vec![
            item("github", ItemKind::Issue, "bug 1", vec![]),
            item("github", ItemKind::Issue, "bug 2", vec![]),
        ];
        let groups = correlate(items);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn workspace_correlates_via_repo_path() {
        let items = vec![
            item("git", ItemKind::Checkout, "feat-x", vec![
                CorrelationKey::Branch("feat-x".into()),
                CorrelationKey::RepoPath("/code/proj/.worktrees/feat-x".into()),
            ]),
            item("cmux", ItemKind::Workspace, "ws-1", vec![
                CorrelationKey::RepoPath("/code/proj/.worktrees/feat-x".into()),
            ]),
        ];
        let groups = correlate(items);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 2);
    }
}
```

**Step 2: Run tests to verify they pass**

Run: `cargo test --lib providers::correlation`
Expected: all 6 tests pass

**Step 3: Add to `src/providers/mod.rs`**

```rust
pub mod types;
pub mod vcs;
pub mod code_review;
pub mod issue_tracker;
pub mod coding_agent;
pub mod ai_utility;
pub mod workspace;
pub mod registry;
pub mod correlation;
```

**Step 4: Verify full project compiles**

Run: `cargo check`

**Step 5: Commit**

```bash
git add src/providers/
git commit -m "feat: add correlation engine with union-find grouping"
```

---

### Task 11: Create discovery module (stub)

The discovery module will be fully implemented in Plan 2 (Migration). For now,
create the module with the detection pipeline signature and a placeholder that
returns an empty registry.

**Files:**
- Create: `src/providers/discovery.rs`
- Modify: `src/providers/mod.rs`

**Step 1: Create `src/providers/discovery.rs`**

```rust
use std::path::Path;

use crate::providers::registry::ProviderRegistry;

/// Detect available providers for a given repository.
///
/// Detection pipeline:
/// 1. VCS: check for .git/ or .jj/ directories
/// 2. Remote host: parse git remote URL → GitHub/GitLab
/// 3. Checkout manager: check for `wt`, fall back to git worktree
/// 4. Coding agent: check for `claude` CLI
/// 5. AI utility: check for `claude` CLI
/// 6. Workspace manager: check env vars ($CMUX_SESSION, $ZELLIJ, $TMUX)
///
/// Config overrides are applied on top of detected providers.
pub async fn detect_providers(_repo_root: &Path) -> ProviderRegistry {
    // TODO: implement detection pipeline in Plan 2
    ProviderRegistry::new()
}
```

**Step 2: Add to `src/providers/mod.rs`**

```rust
pub mod types;
pub mod vcs;
pub mod code_review;
pub mod issue_tracker;
pub mod coding_agent;
pub mod ai_utility;
pub mod workspace;
pub mod registry;
pub mod correlation;
pub mod discovery;
```

**Step 3: Verify and commit**

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: add discovery module stub"
```

---

### Task 12: Final verification

**Step 1: Run all tests**

Run: `cargo test`
Expected: all correlation tests pass, existing code unmodified

**Step 2: Check for warnings**

Run: `cargo check 2>&1`
Expected: may have unused warnings on new types — acceptable at this stage.
Suppress with `#[allow(dead_code)]` on `providers/mod.rs` if noisy:

```rust
#[allow(dead_code)]
pub mod types;
// etc.
```

**Step 3: Verify no existing code was modified**

Run: `git diff HEAD -- src/app.rs src/data.rs src/actions.rs src/ui.rs src/event.rs src/template.rs src/config.rs`
Expected: no diff — these files are untouched.
