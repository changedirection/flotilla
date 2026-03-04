use std::sync::Arc;
use indexmap::IndexMap;
use crate::providers::ai_utility::AiUtility;
use crate::providers::code_review::CodeReview;
use crate::providers::coding_agent::CodingAgent;
use crate::providers::issue_tracker::IssueTracker;
use crate::providers::vcs::{CheckoutManager, Vcs};
use crate::providers::workspace::WorkspaceManager;

pub struct ProviderRegistry {
    pub vcs: IndexMap<String, Box<dyn Vcs>>,
    pub checkout_managers: IndexMap<String, Box<dyn CheckoutManager>>,
    pub code_review: IndexMap<String, Box<dyn CodeReview>>,
    pub issue_trackers: IndexMap<String, Box<dyn IssueTracker>>,
    pub coding_agents: IndexMap<String, Arc<dyn CodingAgent>>,
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
    fn default() -> Self { Self::new() }
}
