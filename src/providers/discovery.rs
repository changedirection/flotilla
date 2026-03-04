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
    // TODO: implement detection pipeline in Plan 2 (Migration)
    ProviderRegistry::new()
}
