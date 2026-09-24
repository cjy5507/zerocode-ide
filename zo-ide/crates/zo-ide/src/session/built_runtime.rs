use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};

use plugins::PluginRegistry;
use runtime::ConversationRuntime;
use tools::GlobalToolRegistry;

use super::{RuntimeLspState, RuntimeMcpState};

pub(crate) struct RuntimePluginState {
    pub(crate) feature_config: runtime::RuntimeFeatureConfig,
    pub(crate) tool_registry: GlobalToolRegistry,
    pub(crate) plugin_registry: PluginRegistry,
    pub(crate) memory_retriever: Option<Arc<dyn runtime::MemoryRetriever + Send + Sync>>,
    /// Seated beside the retriever and shown every recall after it settles;
    /// it answers with the order a turn reads. Built here, where the project's
    /// working directory is known, because that is where its setting and its
    /// ledger live.
    pub(crate) recall_seat: Option<Arc<dyn runtime::RecallSeat>>,
    /// Seated beside full compaction and asked which tool results the
    /// summary still needs. Built here for the reason the recall seat is.
    pub(crate) compaction_seat: Option<Arc<dyn runtime::CompactionSeat>>,
    /// Seated beside the edit tools and asked about every patch they write
    /// before the model reads the result. Built here for the reason the
    /// recall seat is.
    pub(crate) patch_review_seat: Option<Arc<dyn runtime::PatchReviewSeat>>,
    /// Seated beside the tools: handed every shell command before it runs and
    /// every text a tool hands back before the model reads it (t-6348).
    pub(crate) tool_guard_seat: Option<Arc<dyn runtime::ToolGuardSeat>>,
    /// Seated at the public turn boundary to rank files for code requests.
    pub(crate) file_pick_seat: Option<Arc<dyn runtime::FilePickSeat>>,
    pub(crate) skill_suggestion_seat: Option<Arc<dyn runtime::skill_rank::SkillSuggestionSeat>>,
    pub(crate) mcp_state: Option<Arc<Mutex<RuntimeMcpState>>>,
    pub(crate) lsp_state: Option<Arc<Mutex<RuntimeLspState>>>,
}

pub(crate) struct BuiltRuntime {
    pub(crate) runtime:
        Option<ConversationRuntime<crate::AnthropicRuntimeClient, crate::CliToolExecutor>>,
    pub(crate) plugin_registry: PluginRegistry,
    pub(crate) plugins_active: bool,
    pub(crate) mcp_state: Option<Arc<Mutex<RuntimeMcpState>>>,
    pub(crate) mcp_active: bool,
    pub(crate) lsp_state: Option<Arc<Mutex<RuntimeLspState>>>,
    pub(crate) lsp_active: bool,
}

impl BuiltRuntime {
    pub(crate) fn new(
        runtime: ConversationRuntime<crate::AnthropicRuntimeClient, crate::CliToolExecutor>,
        plugin_registry: PluginRegistry,
        mcp_state: Option<Arc<Mutex<RuntimeMcpState>>>,
        lsp_state: Option<Arc<Mutex<RuntimeLspState>>>,
    ) -> Self {
        Self {
            runtime: Some(runtime),
            plugin_registry,
            plugins_active: true,
            mcp_state,
            mcp_active: true,
            lsp_state,
            lsp_active: true,
        }
    }

    pub(crate) fn set_hook_abort_signal(&mut self, hook_abort_signal: runtime::HookAbortSignal) {
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.set_hook_abort_signal(hook_abort_signal);
        }
    }

    /// Read-only twin of [`Self::try_runtime_mut`], for a caller that only
    /// asks the runtime a question (the attempt a turn will carry) and must
    /// not take a mutable borrow to do it.
    pub(crate) fn try_runtime(
        &self,
    ) -> Option<&ConversationRuntime<crate::AnthropicRuntimeClient, crate::CliToolExecutor>>
    {
        self.runtime.as_ref()
    }

    /// Safe mutable accessor — returns `None` instead of panicking when the
    /// inner runtime has been taken (e.g. during `/resume` shutdown).
    pub(crate) fn try_runtime_mut(
        &mut self,
    ) -> Option<&mut ConversationRuntime<crate::AnthropicRuntimeClient, crate::CliToolExecutor>>
    {
        self.runtime.as_mut()
    }

    pub(crate) fn shutdown_plugins(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.plugins_active {
            self.plugin_registry.shutdown()?;
            self.plugins_active = false;
        }
        Ok(())
    }

    pub(crate) fn shutdown_mcp(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.mcp_active {
            if let Some(mcp_state) = &self.mcp_state {
                mcp_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .shutdown()?;
            }
            self.mcp_active = false;
        }
        Ok(())
    }

    pub(crate) fn shutdown_lsp(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.lsp_active {
            if let Some(lsp_state) = &self.lsp_state {
                lsp_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .shutdown()?;
            }
            self.lsp_active = false;
        }
        Ok(())
    }
}

impl Deref for BuiltRuntime {
    type Target = ConversationRuntime<crate::AnthropicRuntimeClient, crate::CliToolExecutor>;

    fn deref(&self) -> &Self::Target {
        self.runtime
            .as_ref()
            .expect("runtime should exist while built runtime is alive")
    }
}

impl DerefMut for BuiltRuntime {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.runtime
            .as_mut()
            .expect("runtime should exist while built runtime is alive")
    }
}

impl Drop for BuiltRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown_lsp();
        let _ = self.shutdown_mcp();
        let _ = self.shutdown_plugins();
    }
}
