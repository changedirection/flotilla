# Provider Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Move existing subprocess integration code behind the provider traits, implement the discovery pipeline, and rewire the app to use the ProviderRegistry.

**Architecture:** Each `fetch_*` function in `data.rs` and each action in `actions.rs` becomes a method on a concrete trait implementation (e.g. `GitHubCodeReview`, `WtCheckoutManager`). The discovery pipeline auto-detects providers. `DataStore::refresh()` calls through the registry, and `correlate()` is replaced by the correlation engine from Plan 1.

**Tech Stack:** Rust 2021, async-trait, indexmap, tokio, serde

**Prerequisite:** Provider Foundation plan (Plan 1) must be complete.

---

### Task 1: Implement GitVcs

Extract git-related queries from `src/data.rs` into a Vcs implementation.

**Files:**
- Create: `src/providers/vcs/git.rs`
- Modify: `src/providers/vcs/mod.rs`

**Step 1: Create `src/providers/vcs/git.rs`**

Move the following logic from `data.rs`:
- `fetch_remote_branches()` (lines 503-520) → `list_remote_branches()`
- `run_command()` helper (lines 473-485) → private `run_cmd()` on the struct

New methods (not currently extracted but needed):
- `list_local_branches()` — call `git branch --list --format='%(refname:short)'`
- `commit_log()` — call `git log <branch> --oneline -<limit>`
- `ahead_behind()` — call `git rev-list --count --left-right <branch>...<reference>`
- `working_tree_status()` — call `git status --porcelain` in checkout_path, count lines

```rust
use std::path::Path;

use async_trait::async_trait;

use crate::providers::types::{AheadBehind, BranchInfo, CommitInfo, WorkingTreeStatus};
use crate::providers::vcs::Vcs;

pub struct GitVcs;

impl GitVcs {
    pub fn new() -> Self {
        Self
    }

    async fn run_cmd(cmd: &str, args: &[&str], cwd: &Path) -> Result<String, String> {
        let output = tokio::process::Command::new(cmd)
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }
}

#[async_trait]
impl Vcs for GitVcs {
    fn display_name(&self) -> &str {
        "Git"
    }

    async fn list_local_branches(&self, repo_root: &Path) -> Result<Vec<BranchInfo>, String> {
        let output = Self::run_cmd(
            "git",
            &["branch", "--list", "--format=%(refname:short)"],
            repo_root,
        )
        .await?;
        // Detect trunk branch
        let head = Self::run_cmd("git", &["symbolic-ref", "--short", "HEAD"], repo_root)
            .await
            .ok();
        let trunk_candidates = ["main", "master", "trunk"];
        Ok(output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|name| {
                let is_trunk = trunk_candidates.contains(&name)
                    || head.as_deref().map(|h| h.trim()) == Some(name);
                BranchInfo {
                    name: name.to_string(),
                    is_trunk,
                }
            })
            .collect())
    }

    async fn list_remote_branches(&self, repo_root: &Path) -> Result<Vec<String>, String> {
        // Extracted from data.rs:503-520
        let output = Self::run_cmd(
            "git",
            &["ls-remote", "--heads", "origin"],
            repo_root,
        )
        .await?;
        Ok(output
            .lines()
            .filter_map(|line| {
                line.split('\t')
                    .nth(1)
                    .and_then(|r| r.strip_prefix("refs/heads/"))
                    .map(|s| s.to_string())
            })
            .collect())
    }

    async fn commit_log(
        &self,
        repo_root: &Path,
        branch: &str,
        limit: usize,
    ) -> Result<Vec<CommitInfo>, String> {
        let output = Self::run_cmd(
            "git",
            &["log", branch, "--oneline", &format!("-{limit}")],
            repo_root,
        )
        .await?;
        Ok(output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|line| {
                let (sha, msg) = line.split_once(' ').unwrap_or((line, ""));
                CommitInfo {
                    short_sha: sha.to_string(),
                    message: msg.to_string(),
                }
            })
            .collect())
    }

    async fn ahead_behind(
        &self,
        repo_root: &Path,
        branch: &str,
        reference: &str,
    ) -> Result<AheadBehind, String> {
        let output = Self::run_cmd(
            "git",
            &[
                "rev-list",
                "--count",
                "--left-right",
                &format!("{branch}...{reference}"),
            ],
            repo_root,
        )
        .await?;
        let parts: Vec<&str> = output.trim().split('\t').collect();
        if parts.len() == 2 {
            Ok(AheadBehind {
                ahead: parts[0].parse().unwrap_or(0),
                behind: parts[1].parse().unwrap_or(0),
            })
        } else {
            Ok(AheadBehind { ahead: 0, behind: 0 })
        }
    }

    async fn working_tree_status(
        &self,
        _repo_root: &Path,
        checkout_path: &Path,
    ) -> Result<WorkingTreeStatus, String> {
        let output = Self::run_cmd("git", &["status", "--porcelain"], checkout_path).await?;
        let mut status = WorkingTreeStatus::default();
        for line in output.lines() {
            if line.len() < 2 {
                continue;
            }
            let index = line.as_bytes()[0];
            let worktree = line.as_bytes()[1];
            if index == b'?' {
                status.untracked += 1;
            } else if index != b' ' {
                status.staged += 1;
            }
            if worktree != b' ' && worktree != b'?' {
                status.modified += 1;
            }
        }
        Ok(status)
    }
}
```

**Step 2: Add `pub mod git;` to `src/providers/vcs/mod.rs`**

**Step 3: Verify**

Run: `cargo check`

**Step 4: Commit**

```bash
git add src/providers/
git commit -m "feat: implement GitVcs provider"
```

---

### Task 2: Implement WtCheckoutManager

Extract worktree operations from `data.rs` and `actions.rs`.

**Files:**
- Create: `src/providers/vcs/wt.rs`
- Modify: `src/providers/vcs/mod.rs`

**Step 1: Create `src/providers/vcs/wt.rs`**

Move logic from:
- `data.rs` `fetch_worktrees()` (lines 487-492) → `list_checkouts()`
- `actions.rs` `create_worktree()` (lines 212-246) → `create_checkout()`
- `actions.rs` `remove_worktree()` (lines 248-261) → `remove_checkout()`

The current `Worktree` struct from `data.rs` is richer than the generic `Checkout`
type (it has `main`, `remote`, `working_tree`, `commit` fields from `wt list`).
Keep those as internal parsing types and map to the generic `Checkout` with
appropriate correlation keys.

```rust
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;

use crate::providers::types::{Checkout, CorrelationKey};
use crate::providers::vcs::CheckoutManager;

pub struct WtCheckoutManager;

/// Internal type matching `wt list --format=json` output.
#[derive(Deserialize)]
struct WtWorktree {
    branch: String,
    path: PathBuf,
    #[serde(default)]
    is_main: bool,
}

impl WtCheckoutManager {
    pub fn new() -> Self {
        Self
    }

    async fn run_cmd(args: &[&str], cwd: &Path) -> Result<String, String> {
        let output = tokio::process::Command::new("wt")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }
}

#[async_trait]
impl CheckoutManager for WtCheckoutManager {
    fn display_name(&self) -> &str {
        "wt (git worktrees)"
    }

    async fn list_checkouts(&self, repo_root: &Path) -> Result<Vec<Checkout>, String> {
        let output = Self::run_cmd(&["list", "--format=json"], repo_root).await?;
        // wt may append ANSI escape codes after JSON
        let json_end = output.rfind(']').map(|i| i + 1).unwrap_or(output.len());
        let worktrees: Vec<WtWorktree> =
            serde_json::from_str(&output[..json_end]).map_err(|e| e.to_string())?;
        Ok(worktrees
            .into_iter()
            .map(|wt| Checkout {
                correlation_keys: vec![
                    CorrelationKey::Branch(wt.branch.clone()),
                    CorrelationKey::RepoPath(wt.path.clone()),
                ],
                branch: wt.branch,
                path: wt.path,
                is_trunk: wt.is_main,
            })
            .collect())
    }

    async fn create_checkout(
        &self,
        repo_root: &Path,
        branch: &str,
    ) -> Result<Checkout, String> {
        Self::run_cmd(&["switch", "--create", branch, "--no-cd"], repo_root).await?;

        // Look up the created worktree path
        let list_output = Self::run_cmd(&["list", "--format=json"], repo_root).await?;
        let worktrees: Vec<WtWorktree> =
            serde_json::from_str(&list_output).map_err(|e| e.to_string())?;
        let wt = worktrees
            .iter()
            .find(|w| w.branch.ends_with(branch) || w.branch == branch)
            .ok_or("Could not find worktree path after creation")?;

        Ok(Checkout {
            correlation_keys: vec![
                CorrelationKey::Branch(wt.branch.clone()),
                CorrelationKey::RepoPath(wt.path.clone()),
            ],
            branch: wt.branch.clone(),
            path: wt.path.clone(),
            is_trunk: false,
        })
    }

    async fn remove_checkout(&self, repo_root: &Path, branch: &str) -> Result<(), String> {
        Self::run_cmd(&["remove", branch], repo_root).await?;
        Ok(())
    }
}
```

**Step 2: Add `pub mod wt;` to `src/providers/vcs/mod.rs`**

**Step 3: Verify and commit**

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: implement WtCheckoutManager provider"
```

---

### Task 3: Implement GitHubCodeReview

**Files:**
- Create: `src/providers/code_review/github.rs`
- Modify: `src/providers/code_review/mod.rs`

**Step 1: Create `src/providers/code_review/github.rs`**

Move logic from:
- `data.rs` `fetch_prs()` (lines 494-501) → `list_change_requests()`
- `data.rs` lines 317-346 ("Fixes #N" parsing) → correlation key emission
- `actions.rs` `open_pr_in_browser()` (lines 263-276) → `open_in_browser()`
- `data.rs` `fetch_merged_pr_branches()` can stay as a helper method

```rust
use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;

use crate::providers::code_review::CodeReview;
use crate::providers::types::{ChangeRequest, ChangeRequestStatus, CorrelationKey};

pub struct GitHubCodeReview {
    provider_name: String,
}

#[derive(Deserialize)]
struct GhPr {
    number: i64,
    title: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    state: String,
    #[serde(default)]
    body: Option<String>,
}

impl GitHubCodeReview {
    pub fn new(provider_name: &str) -> Self {
        Self {
            provider_name: provider_name.to_string(),
        }
    }

    async fn run_gh(args: &[&str], cwd: &Path) -> Result<String, String> {
        let output = tokio::process::Command::new("gh")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }

    /// Parse "Fixes #N", "Closes #N", "Resolves #N" from text.
    fn parse_issue_refs(text: &str, provider_name: &str) -> Vec<CorrelationKey> {
        let mut keys = Vec::new();
        let lower = text.to_lowercase();
        for keyword in ["fixes", "closes", "resolves"] {
            let mut search_from = 0;
            while let Some(pos) = lower[search_from..].find(keyword) {
                let after = search_from + pos + keyword.len();
                let rest = text[after..].trim_start();
                if let Some(rest) = rest.strip_prefix('#') {
                    let num_str: String =
                        rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if !num_str.is_empty() {
                        keys.push(CorrelationKey::IssueRef(
                            provider_name.to_string(),
                            num_str,
                        ));
                    }
                }
                search_from = after;
            }
        }
        keys
    }
}

#[async_trait]
impl CodeReview for GitHubCodeReview {
    fn display_name(&self) -> &str {
        "GitHub"
    }

    async fn list_change_requests(
        &self,
        repo_root: &Path,
        limit: usize,
    ) -> Result<Vec<ChangeRequest>, String> {
        let output = Self::run_gh(
            &[
                "pr",
                "list",
                "--json",
                "number,title,headRefName,state,body",
                "--limit",
                &limit.to_string(),
            ],
            repo_root,
        )
        .await?;
        let prs: Vec<GhPr> = serde_json::from_str(&output).map_err(|e| e.to_string())?;

        Ok(prs
            .into_iter()
            .map(|pr| {
                let mut keys = vec![
                    CorrelationKey::Branch(pr.head_ref_name.clone()),
                    CorrelationKey::ChangeRequestRef(
                        self.provider_name.clone(),
                        pr.number.to_string(),
                    ),
                ];
                // Parse issue refs from title and body
                keys.extend(Self::parse_issue_refs(&pr.title, &self.provider_name));
                if let Some(body) = &pr.body {
                    keys.extend(Self::parse_issue_refs(body, &self.provider_name));
                }

                let status = match pr.state.as_str() {
                    "MERGED" => ChangeRequestStatus::Merged,
                    "CLOSED" => ChangeRequestStatus::Closed,
                    _ if pr.title.starts_with("Draft:") || pr.title.starts_with("[Draft]") => {
                        ChangeRequestStatus::Draft
                    }
                    _ => ChangeRequestStatus::Open,
                };

                ChangeRequest {
                    id: pr.number.to_string(),
                    title: pr.title,
                    branch: pr.head_ref_name,
                    status,
                    body: pr.body,
                    correlation_keys: keys,
                }
            })
            .collect())
    }

    async fn get_change_request(
        &self,
        repo_root: &Path,
        id: &str,
    ) -> Result<ChangeRequest, String> {
        let output = Self::run_gh(
            &["pr", "view", id, "--json", "number,title,headRefName,state,body"],
            repo_root,
        )
        .await?;
        let pr: GhPr = serde_json::from_str(&output).map_err(|e| e.to_string())?;
        Ok(ChangeRequest {
            id: pr.number.to_string(),
            title: pr.title,
            branch: pr.head_ref_name,
            status: match pr.state.as_str() {
                "MERGED" => ChangeRequestStatus::Merged,
                "CLOSED" => ChangeRequestStatus::Closed,
                _ => ChangeRequestStatus::Open,
            },
            body: pr.body,
            correlation_keys: vec![],
        })
    }

    async fn open_in_browser(&self, repo_root: &Path, id: &str) -> Result<(), String> {
        Self::run_gh(&["pr", "view", id, "--web"], repo_root).await?;
        Ok(())
    }
}
```

**Step 2: Add `pub mod github;` and verify**

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: implement GitHubCodeReview provider"
```

---

### Task 4: Implement GitHubIssueTracker

**Files:**
- Create: `src/providers/issue_tracker/github.rs`
- Modify: `src/providers/issue_tracker/mod.rs`

**Step 1: Create `src/providers/issue_tracker/github.rs`**

Move logic from `data.rs` `fetch_issues()` (lines 535-542) and
`actions.rs` `open_issue_in_browser()` (lines 320-333).

```rust
use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;

use crate::providers::issue_tracker::IssueTracker;
use crate::providers::types::{CorrelationKey, Issue};

pub struct GitHubIssueTracker {
    provider_name: String,
}

#[derive(Deserialize)]
struct GhIssue {
    number: i64,
    title: String,
    labels: Vec<GhLabel>,
}

#[derive(Deserialize)]
struct GhLabel {
    name: String,
}

impl GitHubIssueTracker {
    pub fn new(provider_name: &str) -> Self {
        Self {
            provider_name: provider_name.to_string(),
        }
    }

    async fn run_gh(args: &[&str], cwd: &Path) -> Result<String, String> {
        let output = tokio::process::Command::new("gh")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }
}

#[async_trait]
impl IssueTracker for GitHubIssueTracker {
    fn display_name(&self) -> &str {
        "GitHub Issues"
    }

    async fn list_issues(
        &self,
        repo_root: &Path,
        limit: usize,
    ) -> Result<Vec<Issue>, String> {
        let output = Self::run_gh(
            &[
                "issue",
                "list",
                "--json",
                "number,title,labels",
                "--limit",
                &limit.to_string(),
                "--state",
                "open",
            ],
            repo_root,
        )
        .await?;
        let issues: Vec<GhIssue> = serde_json::from_str(&output).map_err(|e| e.to_string())?;
        Ok(issues
            .into_iter()
            .map(|i| Issue {
                correlation_keys: vec![CorrelationKey::IssueRef(
                    self.provider_name.clone(),
                    i.number.to_string(),
                )],
                id: i.number.to_string(),
                title: i.title,
                labels: i.labels.into_iter().map(|l| l.name).collect(),
            })
            .collect())
    }

    async fn open_in_browser(&self, repo_root: &Path, id: &str) -> Result<(), String> {
        Self::run_gh(&["issue", "view", id, "--web"], repo_root).await?;
        Ok(())
    }
}
```

**Step 2: Add `pub mod github;` and verify**

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: implement GitHubIssueTracker provider"
```

---

### Task 5: Implement ClaudeCodingAgent

**Files:**
- Create: `src/providers/coding_agent/claude.rs`
- Modify: `src/providers/coding_agent/mod.rs`

**Step 1: Create `src/providers/coding_agent/claude.rs`**

Move logic from `data.rs`:
- OAuth token management (lines 544-612)
- `fetch_sessions()` (lines 650-681) → `list_sessions()`
- `archive_session()` (lines 683-720) → `archive_session()`
- Session data types remain internal

The `attach_command()` method simply returns the CLI string.

```rust
use std::process::Stdio;
use std::sync::{LazyLock, Mutex};

use async_trait::async_trait;
use serde::Deserialize;

use crate::providers::coding_agent::CodingAgent;
use crate::providers::types::{CloudAgentSession, CorrelationKey, SessionStatus};

pub struct ClaudeCodingAgent {
    provider_name: String,
}

// --- Internal types for OAuth + API ---

#[derive(Deserialize)]
struct OAuthCredentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: OAuthToken,
}

#[derive(Deserialize, Clone)]
struct OAuthToken {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: i64,
}

struct AuthCache {
    token: Option<OAuthToken>,
    org_uuid: Option<String>,
}

static AUTH_CACHE: LazyLock<Mutex<AuthCache>> = LazyLock::new(|| {
    Mutex::new(AuthCache {
        token: None,
        org_uuid: None,
    })
});

#[derive(Deserialize)]
struct SessionsResponse {
    data: Vec<ApiSession>,
}

#[derive(Deserialize)]
struct ApiSession {
    id: String,
    title: String,
    session_status: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    session_context: ApiSessionContext,
}

#[derive(Default, Deserialize)]
struct ApiSessionContext {
    #[serde(default)]
    model: String,
    #[serde(default)]
    outcomes: Vec<ApiOutcome>,
}

#[derive(Deserialize)]
struct ApiOutcome {
    #[serde(default)]
    git_info: Option<ApiGitInfo>,
}

#[derive(Deserialize)]
struct ApiGitInfo {
    #[serde(default)]
    branches: Vec<String>,
}

impl ApiSession {
    fn branch(&self) -> Option<&str> {
        self.session_context
            .outcomes
            .first()
            .and_then(|o| o.git_info.as_ref())
            .and_then(|gi| gi.branches.first())
            .map(|s| {
                s.strip_prefix("refs/heads/")
                    .or_else(|| s.strip_prefix("refs/remotes/origin/"))
                    .unwrap_or(s)
            })
    }
}

impl ClaudeCodingAgent {
    pub fn new(provider_name: &str) -> Self {
        Self {
            provider_name: provider_name.to_string(),
        }
    }

    // OAuth methods — same as current data.rs implementation
    async fn read_oauth_token_from_keychain() -> Result<OAuthToken, String> {
        let output = tokio::process::Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err("No Claude Code credentials in keychain".to_string());
        }
        let json = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let creds: OAuthCredentials = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        Ok(creds.claude_ai_oauth)
    }

    async fn get_oauth_token() -> Result<OAuthToken, String> {
        {
            let cache = AUTH_CACHE.lock().unwrap();
            if let Some(ref token) = cache.token {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                if token.expires_at > now + 60 {
                    return Ok(token.clone());
                }
            }
        }
        let token = Self::read_oauth_token_from_keychain().await?;
        let mut cache = AUTH_CACHE.lock().unwrap();
        cache.token = Some(token.clone());
        cache.org_uuid = None;
        Ok(token)
    }

    async fn get_org_uuid(token: &str) -> Result<String, String> {
        {
            let cache = AUTH_CACHE.lock().unwrap();
            if let Some(ref uuid) = cache.org_uuid {
                return Ok(uuid.clone());
            }
        }
        let output = tokio::process::Command::new("curl")
            .args([
                "-s",
                "-H",
                &format!("Authorization: Bearer {token}"),
                "-H",
                "anthropic-version: 2023-06-01",
                "https://api.anthropic.com/api/oauth/profile",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let body = String::from_utf8_lossy(&output.stdout).to_string();
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let uuid = v
            .get("organization")
            .and_then(|o| o.get("uuid"))
            .and_then(|u| u.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "No organization.uuid in profile".to_string())?;
        let mut cache = AUTH_CACHE.lock().unwrap();
        cache.org_uuid = Some(uuid.clone());
        Ok(uuid)
    }
}

#[async_trait]
impl CodingAgent for ClaudeCodingAgent {
    fn display_name(&self) -> &str {
        "Claude Code"
    }

    async fn list_sessions(&self) -> Result<Vec<CloudAgentSession>, String> {
        let token = Self::get_oauth_token().await?;
        let org_uuid = Self::get_org_uuid(&token.access_token).await?;

        let output = tokio::process::Command::new("curl")
            .args([
                "-s",
                "-H",
                &format!("Authorization: Bearer {}", token.access_token),
                "-H",
                "anthropic-beta: ccr-byoc-2025-07-29",
                "-H",
                "anthropic-version: 2023-06-01",
                "-H",
                &format!("x-organization-uuid: {org_uuid}"),
                "https://api.anthropic.com/v1/sessions",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|e| e.to_string())?;

        let body = String::from_utf8_lossy(&output.stdout).to_string();
        let resp: SessionsResponse =
            serde_json::from_str(&body).map_err(|e| e.to_string())?;

        let mut sessions: Vec<_> = resp
            .data
            .into_iter()
            .filter(|s| s.session_status != "archived")
            .collect();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        Ok(sessions
            .into_iter()
            .map(|s| {
                let mut keys = vec![CorrelationKey::SessionRef(
                    self.provider_name.clone(),
                    s.id.clone(),
                )];
                if let Some(branch) = s.branch() {
                    keys.push(CorrelationKey::Branch(branch.to_string()));
                }
                CloudAgentSession {
                    id: s.id,
                    title: s.title,
                    status: match s.session_status.as_str() {
                        "running" => SessionStatus::Running,
                        "archived" => SessionStatus::Archived,
                        _ => SessionStatus::Idle,
                    },
                    model: if s.session_context.model.is_empty() {
                        None
                    } else {
                        Some(s.session_context.model)
                    },
                    correlation_keys: keys,
                }
            })
            .collect())
    }

    async fn archive_session(&self, session_id: &str) -> Result<(), String> {
        let token = Self::get_oauth_token().await?;
        let org_uuid = Self::get_org_uuid(&token.access_token).await?;
        let url = format!("https://api.anthropic.com/v1/sessions/{session_id}");
        let output = tokio::process::Command::new("curl")
            .args([
                "-s",
                "-w",
                "\n%{http_code}",
                "-X",
                "PATCH",
                "-H",
                &format!("Authorization: Bearer {}", token.access_token),
                "-H",
                "anthropic-beta: ccr-byoc-2025-07-29",
                "-H",
                "anthropic-version: 2023-06-01",
                "-H",
                &format!("x-organization-uuid: {org_uuid}"),
                "-H",
                "content-type: application/json",
                "-d",
                r#"{"session_status":"archived"}"#,
                &url,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|e| e.to_string())?;

        let body = String::from_utf8_lossy(&output.stdout).to_string();
        let status_code: u16 = body
            .lines()
            .last()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        if (200..300).contains(&status_code) {
            Ok(())
        } else {
            Err(format!(
                "archive failed (HTTP {}): {}",
                status_code,
                body.lines().next().unwrap_or("")
            ))
        }
    }

    async fn attach_command(&self, session_id: &str) -> Result<String, String> {
        Ok(format!("claude --teleport {session_id}"))
    }
}
```

**Step 2: Add `pub mod claude;` and verify**

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: implement ClaudeCodingAgent provider"
```

---

### Task 6: Implement ClaudeAiUtility

**Files:**
- Create: `src/providers/ai_utility/claude.rs`
- Modify: `src/providers/ai_utility/mod.rs`

**Step 1: Create `src/providers/ai_utility/claude.rs`**

Move logic from `actions.rs` `generate_branch_name()` (lines 278-318).

```rust
use async_trait::async_trait;

use crate::providers::ai_utility::AiUtility;

pub struct ClaudeAiUtility;

impl ClaudeAiUtility {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AiUtility for ClaudeAiUtility {
    fn display_name(&self) -> &str {
        "Claude CLI"
    }

    async fn generate_branch_name(&self, context: &str) -> Result<String, String> {
        let prompt = format!(
            "Suggest a short git branch name for these GitHub issues. \
             Output ONLY the branch name, nothing else. Use kebab-case: {context}"
        );
        let output = tokio::process::Command::new("claude")
            .args(["-p", &prompt])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if branch.is_empty() {
                Err("claude returned empty output".to_string())
            } else {
                Ok(branch)
            }
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }
}
```

**Step 2: Add `pub mod claude;` and verify**

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: implement ClaudeAiUtility provider"
```

---

### Task 7: Implement CmuxWorkspaceManager

**Files:**
- Create: `src/providers/workspace/cmux.rs`
- Modify: `src/providers/workspace/mod.rs`

**Step 1: Create `src/providers/workspace/cmux.rs`**

Move logic from:
- `data.rs` `fetch_cmux_workspaces()` (lines 798-818) → `list_workspaces()`
- `actions.rs` `create_cmux_workspace()` (lines 37-205) → `create_workspace()`
- `actions.rs` `select_cmux_workspace()` (lines 207-210) → `select_workspace()`
- `actions.rs` `cmux_cmd()` helper (lines 8-24) → private method

This is the largest single migration. The template rendering from `template.rs`
is consumed here. `WorkspaceConfig::template_yaml` carries the YAML string
which the cmux implementation parses with the existing `WorkspaceTemplate` type.

```rust
use std::path::PathBuf;

use async_trait::async_trait;

use crate::providers::types::{CorrelationKey, Workspace, WorkspaceConfig};
use crate::providers::workspace::WorkspaceManager;
use crate::template::WorkspaceTemplate;

const CMUX_BIN: &str = "/Applications/cmux.app/Contents/Resources/bin/cmux";

pub struct CmuxWorkspaceManager;

impl CmuxWorkspaceManager {
    pub fn new() -> Self {
        Self
    }

    async fn cmux_cmd(args: &[&str]) -> Result<String, String> {
        let output = tokio::process::Command::new(CMUX_BIN)
            .args(args)
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return Err(format!(
                "cmux {} failed: {}",
                args.first().unwrap_or(&""),
                if stderr.is_empty() { &stdout } else { &stderr }
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn parse_ok_ref(output: &str) -> String {
        output
            .strip_prefix("OK ")
            .unwrap_or(output)
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string()
    }
}

#[async_trait]
impl WorkspaceManager for CmuxWorkspaceManager {
    fn display_name(&self) -> &str {
        "cmux"
    }

    async fn list_workspaces(&self) -> Result<Vec<Workspace>, String> {
        let output = Self::cmux_cmd(&["--json", "list-workspaces"]).await?;
        let parsed: serde_json::Value =
            serde_json::from_str(&output).map_err(|e| e.to_string())?;
        let workspaces = parsed["workspaces"]
            .as_array()
            .ok_or("no workspaces array")?;
        Ok(workspaces
            .iter()
            .filter_map(|ws| {
                let ws_ref = ws["ref"].as_str()?.to_string();
                let name = ws["title"].as_str().unwrap_or("").to_string();
                let directories: Vec<PathBuf> = ws["directories"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(PathBuf::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let keys: Vec<CorrelationKey> = directories
                    .iter()
                    .map(|d| CorrelationKey::RepoPath(d.clone()))
                    .collect();
                Some(Workspace {
                    ws_ref,
                    name,
                    directories,
                    correlation_keys: keys,
                })
            })
            .collect())
    }

    async fn create_workspace(&self, config: &WorkspaceConfig) -> Result<Workspace, String> {
        // Parse template from config, or use default
        let template = if let Some(yaml) = &config.template_yaml {
            serde_yaml::from_str::<WorkspaceTemplate>(yaml)
                .unwrap_or_else(|_| WorkspaceTemplate::load_default())
        } else {
            WorkspaceTemplate::load_default()
        };
        let rendered = template.render(&config.template_vars);

        // The full workspace creation sequence from actions.rs:37-205
        // (create workspace, split panes, create surfaces, send commands, etc.)
        // is moved here verbatim. See actions.rs for the original implementation.
        //
        // For brevity in this plan, the method body is the same as
        // actions::create_cmux_workspace() but using Self::cmux_cmd()
        // and Self::parse_ok_ref() instead of the module-level functions.

        let ws_output = Self::cmux_cmd(&["new-workspace", "--name", &config.name]).await?;
        let ws_ref = Self::parse_ok_ref(&ws_output);
        if ws_ref.is_empty() {
            return Err("cmux new-workspace returned no workspace ref".to_string());
        }

        // ... (rest of create_cmux_workspace logic from actions.rs:54-204)
        // Copy the full pane/surface creation loop from actions.rs.
        // The only change: use config.working_directory instead of worktree_path param.

        // After creation, return the workspace handle
        Ok(Workspace {
            ws_ref,
            name: config.name.clone(),
            directories: vec![config.working_directory.clone()],
            correlation_keys: vec![CorrelationKey::RepoPath(
                config.working_directory.clone(),
            )],
        })
    }

    async fn select_workspace(&self, ws_ref: &str) -> Result<(), String> {
        Self::cmux_cmd(&["select-workspace", "--workspace", ws_ref]).await?;
        Ok(())
    }
}
```

Note: The `create_workspace` body should be copied in full from `actions.rs`
lines 47-204. The plan abbreviates for readability, but the implementer
must copy the complete pane orchestration logic.

**Step 2: Update `template.rs` to expose `load_default()`**

Add to `src/template.rs`:

```rust
pub fn load_default() -> Self {
    Self::default_template()
}
```

**Step 3: Add `pub mod cmux;` and verify**

Run: `cargo check`

```bash
git add src/providers/ src/template.rs
git commit -m "feat: implement CmuxWorkspaceManager provider"
```

---

### Task 8: Implement discovery pipeline

**Files:**
- Modify: `src/providers/discovery.rs`

**Step 1: Implement `detect_providers()`**

```rust
use std::path::Path;

use crate::providers::ai_utility::claude::ClaudeAiUtility;
use crate::providers::code_review::github::GitHubCodeReview;
use crate::providers::coding_agent::claude::ClaudeCodingAgent;
use crate::providers::issue_tracker::github::GitHubIssueTracker;
use crate::providers::registry::ProviderRegistry;
use crate::providers::vcs::git::GitVcs;
use crate::providers::vcs::wt::WtCheckoutManager;
use crate::providers::workspace::cmux::CmuxWorkspaceManager;

/// Check if a command exists and runs successfully.
async fn command_exists(cmd: &str, args: &[&str]) -> bool {
    tokio::process::Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Parse the remote URL to detect the hosting platform.
async fn detect_remote_host(repo_root: &Path) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_root)
        .output()
        .await
        .ok()?;
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.contains("github.com") {
        Some("github".to_string())
    } else if url.contains("gitlab.com") || url.contains("gitlab") {
        Some("gitlab".to_string())
    } else {
        None
    }
}

pub async fn detect_providers(repo_root: &Path) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();

    // 1. VCS detection
    if repo_root.join(".git").exists() || repo_root.join(".git").is_file() {
        registry
            .vcs
            .insert("git".to_string(), Box::new(GitVcs::new()));

        // Checkout manager: prefer wt if available
        if command_exists("wt", &["--version"]).await {
            registry
                .checkout_managers
                .insert("git".to_string(), Box::new(WtCheckoutManager::new()));
        }
        // TODO: fallback to GitWorktreeCheckoutManager when implemented
    }
    // TODO: jj detection (.jj/ directory)

    // 2. Remote host detection
    if let Some(host) = detect_remote_host(repo_root).await {
        match host.as_str() {
            "github" => {
                if command_exists("gh", &["--version"]).await {
                    registry
                        .code_review
                        .insert("github".to_string(), Box::new(GitHubCodeReview::new("github")));
                    registry.issue_trackers.insert(
                        "github".to_string(),
                        Box::new(GitHubIssueTracker::new("github")),
                    );
                }
            }
            // TODO: "gitlab" => { ... }
            _ => {}
        }
    }

    // 3. Coding agent detection
    if command_exists("claude", &["--version"]).await {
        registry.coding_agents.insert(
            "claude".to_string(),
            Box::new(ClaudeCodingAgent::new("claude")),
        );
    }

    // 4. AI utility detection
    if command_exists("claude", &["--version"]).await {
        registry
            .ai_utilities
            .insert("claude".to_string(), Box::new(ClaudeAiUtility::new()));
    }

    // 5. Workspace manager detection (env vars)
    if std::env::var("CMUX_SESSION").is_ok()
        || command_exists(
            "/Applications/cmux.app/Contents/Resources/bin/cmux",
            &["--version"],
        )
        .await
    {
        registry.workspace_manager = Some((
            "cmux".to_string(),
            Box::new(CmuxWorkspaceManager::new()),
        ));
    }
    // TODO: $ZELLIJ, $TMUX detection

    registry
}
```

**Step 2: Verify and commit**

Run: `cargo check`

```bash
git add src/providers/
git commit -m "feat: implement provider discovery pipeline"
```

---

### Task 9: Rewire DataStore to use ProviderRegistry

This is the integration task. `DataStore::refresh()` switches from calling
`fetch_*()` functions directly to calling through the registry.

**Files:**
- Modify: `src/data.rs`
- Modify: `src/app.rs`
- Modify: `src/main.rs`

This task is complex and should be done incrementally:

**Step 1: Add ProviderRegistry to App**

In `src/app.rs`, add a registry field to `App`:

```rust
use crate::providers::registry::ProviderRegistry;

pub struct App {
    // ... existing fields ...
    pub registry: ProviderRegistry,
}
```

Initialize in `App::new()` with an empty registry (discovery happens in main).

**Step 2: Run discovery at startup in `src/main.rs`**

After creating the app, run discovery for each repo and populate the registry.
For now, use a single shared registry (all repos use the same providers).

```rust
use providers::discovery::detect_providers;

// After creating app:
let registry = detect_providers(&repo_roots[0]).await;
app.registry = registry;
```

**Step 3: Create a new `refresh` path using the registry**

Add a `refresh_via_registry()` method to `DataStore` that calls through the
registry's providers instead of the direct `fetch_*()` functions. Keep the old
`refresh()` method temporarily for comparison.

**Step 4: Switch `refresh_all()` to use the registry path**

Update `main.rs` `refresh_all()` to call the new method.

**Step 5: Build correlation using the engine**

Replace `DataStore::correlate()` with calls to `providers::correlation::correlate()`,
mapping `CorrelatedGroup`s back into `TableEntry`/`WorkItem` for the existing UI.

**Step 6: Verify the app still works**

Run: `cargo run`
Test: all sections (worktrees, PRs, issues, sessions, remote branches) appear

**Step 7: Remove old `fetch_*()` functions and `correlate()`**

Once the registry path works, delete the old direct subprocess calls.

**Step 8: Commit**

```bash
git add src/
git commit -m "refactor: rewire DataStore to use ProviderRegistry"
```

---

### Task 10: Rewire actions to use registry

**Files:**
- Modify: `src/main.rs` (pending action dispatch)
- Modify: `src/actions.rs` (remove migrated functions)

**Step 1: Update pending action handlers in `main.rs`**

Replace direct calls to `actions::create_worktree()`, `actions::remove_worktree()`,
etc. with calls through the registry:

```rust
// Before:
actions::create_worktree(&branch, &repo)

// After:
if let Some(cm) = app.registry.checkout_managers.values().next() {
    cm.create_checkout(repo.as_path(), &branch).await
}
```

Similarly for:
- `actions::open_pr_in_browser()` → `registry.code_review["github"].open_in_browser()`
- `actions::open_issue_in_browser()` → `registry.issue_trackers["github"].open_in_browser()`
- `data::archive_session()` → `registry.coding_agents["claude"].archive_session()`
- `actions::generate_branch_name()` → `registry.ai_utilities["claude"].generate_branch_name()`
- `actions::create_cmux_workspace()` → `registry.workspace_manager.create_workspace()`
- `actions::select_cmux_workspace()` → `registry.workspace_manager.select_workspace()`

**Step 2: Remove migrated functions from `actions.rs`**

Delete: `create_worktree`, `remove_worktree`, `open_pr_in_browser`,
`open_issue_in_browser`, `generate_branch_name`.

Keep: `create_cmux_workspace` and `select_cmux_workspace` can be removed once
the workspace manager provider is wired in.

**Step 3: Remove migrated functions from `data.rs`**

Delete: `fetch_worktrees`, `fetch_prs`, `fetch_issues`, `fetch_sessions`,
`fetch_remote_branches`, `fetch_merged_pr_branches`, `fetch_cmux_workspaces`,
`archive_session`, OAuth helpers, `run_command`.

**Step 4: Clean up unused types in `data.rs`**

Types like `GithubPr`, `GithubIssue`, `WebSession`, `CmuxWorkspace`, `OAuthToken`
etc. are now internal to their respective provider implementations. Remove them
from `data.rs`.

Keep: `WorkItem`, `WorkItemKind`, `TableEntry`, `SectionHeader`, `DataStore`,
`DeleteConfirmInfo` — these are used by the UI layer.

**Step 5: Verify and commit**

Run: `cargo check && cargo test && cargo run`

```bash
git add src/
git commit -m "refactor: rewire actions to use ProviderRegistry, remove old code"
```

---

### Task 11: Final cleanup and verification

**Step 1: Run clippy**

Run: `cargo clippy -- -W warnings`
Fix any warnings.

**Step 2: Verify all features work**

Manual test checklist:
- [ ] App launches, shows worktrees
- [ ] PRs appear and correlate with worktrees by branch
- [ ] Issues appear, linked issues show under their PR's worktree
- [ ] Sessions appear with branch correlation
- [ ] Remote branches appear (excluding known/merged)
- [ ] Create worktree (n key) works
- [ ] Delete worktree (d key) works
- [ ] Open PR in browser (p key) works
- [ ] Open issue in browser works
- [ ] Archive session works
- [ ] Generate branch name works
- [ ] Create/switch workspace works
- [ ] Multi-repo tabs work
- [ ] Refresh (r key) works

**Step 3: Commit**

```bash
git add src/
git commit -m "chore: cleanup after provider migration"
```
