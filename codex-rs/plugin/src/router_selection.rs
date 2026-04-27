#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginRouterSelection {
    pub host_scopes: Vec<String>,
    pub domains: Vec<PluginRouterSelectionDomain>,
    pub suppression: PluginRouterSelectionSuppression,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRouterSelectionDomain {
    pub prompt_signals: Vec<String>,
    pub workspace_files: Vec<String>,
    pub workspace_extensions: Vec<String>,
    pub select: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRouterSelectionSuppression {
    pub when_explicit_skill_selected: bool,
}

impl Default for PluginRouterSelectionSuppression {
    fn default() -> Self {
        Self {
            when_explicit_skill_selected: true,
        }
    }
}
