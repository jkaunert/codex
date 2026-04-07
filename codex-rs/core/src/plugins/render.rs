#[cfg(test)]
use crate::context::AvailablePluginsInstructions;
#[cfg(test)]
use crate::context::ContextualUserFragment;
use crate::plugins::PluginCapabilitySummary;

#[cfg(test)]
pub(crate) fn render_plugins_section(plugins: &[PluginCapabilitySummary]) -> Option<String> {
    AvailablePluginsInstructions::from_plugins(plugins).map(|instructions| instructions.render())
}

pub(crate) fn render_explicit_plugin_instructions(
    plugin: &PluginCapabilitySummary,
    available_mcp_servers: &[String],
    available_apps: &[String],
) -> Option<String> {
    let mut lines = vec![format!(
        "Capabilities from the `{}` plugin:",
        plugin.display_name
    )];

    if plugin.has_skills {
        if let Some(namespace) = plugin_skill_namespace(plugin) {
            lines.push(format!(
                "- Skills from this plugin are prefixed with `{namespace}:` in the Skills list."
            ));
        } else {
            lines.push("- Skills from this plugin are available in the Skills list.".to_string());
        }
    }

    if !available_mcp_servers.is_empty() {
        lines.push(format!(
            "- MCP servers from this plugin available in this session: {}.",
            available_mcp_servers
                .iter()
                .map(|server| format!("`{server}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if !available_apps.is_empty() {
        lines.push(format!(
            "- Apps from this plugin available in this session: {}.",
            available_apps
                .iter()
                .map(|app| format!("`{app}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if lines.len() == 1 {
        return None;
    }

    lines.push("Use these plugin-associated capabilities to help solve the task.".to_string());

    Some(lines.join("\n"))
}

fn plugin_skill_namespace(plugin: &PluginCapabilitySummary) -> Option<&str> {
    plugin.config_name.split_once('@').map_or_else(
        || (!plugin.config_name.is_empty()).then_some(plugin.config_name.as_str()),
        |(plugin_name, _)| (!plugin_name.is_empty()).then_some(plugin_name),
    )
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
