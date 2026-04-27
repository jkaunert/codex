use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::SkillMetadata;
use codex_plugin::PluginRouterSelection;
use codex_plugin::PluginRouterSelectionDomain;
use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::AbsolutePathBuf;

const DESKTOP_CLIENT_SIGNAL: &str = "desktop";
const PRIMARY_TARGET_SKILL: &str = "apple-appdev-workflow:apple-app-orchestrator";
const FALLBACK_TARGET_SKILL: &str = "apple-app-orchestrator";
const APPLE_PROMPT_SIGNALS: &[&str] = &[
    "ios",
    "macos",
    "swiftui",
    "swiftdata",
    "uikit",
    "appkit",
    "xcode",
    "xcodebuild",
    "testflight",
    "bundle id",
    "app store",
    "core data",
    "apple app",
];
const APPLE_WORKSPACE_MARKERS: &[&str] = &["Package.swift"];
const APPLE_WORKSPACE_EXTENSIONS: &[&str] = &["xcodeproj", "xcworkspace"];

/// Experimental desktop-only deterministic top-level skill injection.
///
/// This helper is intentionally conservative:
/// - off unless explicitly enabled by config
/// - scoped to desktop-like app-server clients
/// - only activates when no explicit skill already resolved upstream
/// - injects at most one top-level Apple orchestrator skill
pub fn maybe_collect_desktop_top_level_skill_injection(
    inputs: &[UserInput],
    skills: &[SkillMetadata],
    disabled_paths: &HashSet<AbsolutePathBuf>,
    router_selections: &[PluginRouterSelection],
    client_name: Option<&str>,
    cwd: &Path,
    enabled: bool,
) -> Vec<SkillMetadata> {
    if !enabled || !is_desktop_client(client_name) {
        return Vec::new();
    }

    let configured_injections = collect_manifest_router_selection_injections(
        inputs,
        skills,
        disabled_paths,
        router_selections,
        client_name,
        cwd,
    );
    if !configured_injections.is_empty() {
        return configured_injections;
    }

    if !has_apple_prompt_signal(inputs) && !has_apple_workspace_signal(cwd) {
        return Vec::new();
    }

    skills
        .iter()
        .find(|skill| {
            !disabled_paths.contains(&skill.path_to_skills_md)
                && matches_target_skill_name(skill.name.as_str())
        })
        .cloned()
        .into_iter()
        .collect()
}

fn collect_manifest_router_selection_injections(
    inputs: &[UserInput],
    skills: &[SkillMetadata],
    disabled_paths: &HashSet<AbsolutePathBuf>,
    router_selections: &[PluginRouterSelection],
    client_name: Option<&str>,
    cwd: &Path,
) -> Vec<SkillMetadata> {
    for config in router_selections {
        if config.suppression.when_explicit_skill_selected && has_structured_skill_input(inputs) {
            continue;
        }
        if !client_matches_any_host_scope(client_name, &config.host_scopes) {
            continue;
        }

        for domain in &config.domains {
            if !has_configured_prompt_signal(inputs, &domain.prompt_signals)
                && !has_configured_workspace_signal(cwd, domain)
            {
                continue;
            }

            if let Some(skill) = skills.iter().find(|skill| {
                !disabled_paths.contains(&skill.path_to_skills_md) && skill.name == domain.select
            }) {
                return vec![skill.clone()];
            }
        }
    }

    Vec::new()
}

fn has_structured_skill_input(inputs: &[UserInput]) -> bool {
    inputs
        .iter()
        .any(|input| matches!(input, UserInput::Skill { .. }))
}

fn client_matches_any_host_scope(client_name: Option<&str>, host_scopes: &[String]) -> bool {
    let Some(client_name) = client_name.map(|name| name.to_ascii_lowercase()) else {
        return false;
    };
    host_scopes
        .iter()
        .any(|scope| client_name.contains(&scope.to_ascii_lowercase()))
}

fn has_configured_prompt_signal(inputs: &[UserInput], prompt_signals: &[String]) -> bool {
    inputs.iter().any(|input| match input {
        UserInput::Text { text, .. } => {
            let lowered = text.to_ascii_lowercase();
            prompt_signals
                .iter()
                .any(|signal| text_contains_prompt_signal(&lowered, &signal.to_ascii_lowercase()))
        }
        UserInput::Image { .. }
        | UserInput::LocalImage { .. }
        | UserInput::Skill { .. }
        | UserInput::Mention { .. }
        | _ => false,
    })
}

fn has_configured_workspace_signal(cwd: &Path, domain: &PluginRouterSelectionDomain) -> bool {
    let Ok(entries) = fs::read_dir(cwd) else {
        return false;
    };

    entries.flatten().any(|entry| {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            return false;
        };
        if domain.workspace_files.iter().any(|marker| marker == name) {
            return true;
        }
        path.extension()
            .and_then(|value| value.to_str())
            .map(|ext| {
                domain
                    .workspace_extensions
                    .iter()
                    .any(|configured| configured == ext)
            })
            .unwrap_or(false)
    })
}

fn is_desktop_client(client_name: Option<&str>) -> bool {
    client_name
        .map(|name| name.to_ascii_lowercase().contains(DESKTOP_CLIENT_SIGNAL))
        .unwrap_or(false)
}

fn has_apple_prompt_signal(inputs: &[UserInput]) -> bool {
    inputs.iter().any(|input| match input {
        UserInput::Text { text, .. } => {
            let lowered = text.to_ascii_lowercase();
            APPLE_PROMPT_SIGNALS
                .iter()
                .any(|signal| text_contains_prompt_signal(&lowered, signal))
        }
        UserInput::Image { .. }
        | UserInput::LocalImage { .. }
        | UserInput::Skill { .. }
        | UserInput::Mention { .. }
        | _ => false,
    })
}

fn text_contains_prompt_signal(text: &str, signal: &str) -> bool {
    text.match_indices(signal).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + signal.len()..].chars().next();

        before.is_none_or(is_prompt_signal_boundary)
            && (after.is_none_or(is_prompt_signal_boundary)
                || signal_allows_version_suffix(signal, after))
    })
}

fn signal_allows_version_suffix(signal: &str, after: Option<char>) -> bool {
    matches!(signal, "ios" | "macos") && after.is_some_and(|ch| ch.is_ascii_digit())
}

fn is_prompt_signal_boundary(ch: char) -> bool {
    !ch.is_ascii_alphanumeric()
}

fn has_apple_workspace_signal(cwd: &Path) -> bool {
    let Ok(entries) = fs::read_dir(cwd) else {
        return false;
    };

    entries.flatten().any(|entry| {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            return false;
        };
        if APPLE_WORKSPACE_MARKERS.contains(&name) {
            return true;
        }
        path.extension()
            .and_then(|value| value.to_str())
            .map(|ext| APPLE_WORKSPACE_EXTENSIONS.contains(&ext))
            .unwrap_or(false)
    })
}

fn matches_target_skill_name(name: &str) -> bool {
    name == PRIMARY_TARGET_SKILL
        || name == FALLBACK_TARGET_SKILL
        || name.ends_with(":apple-app-orchestrator")
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::protocol::SkillScope;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn make_skill(name: &str, path: &AbsolutePathBuf) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            description: format!("{name} skill"),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            path_to_skills_md: path.clone(),
            scope: SkillScope::User,
        }
    }

    fn router_selection(select: &str) -> PluginRouterSelection {
        PluginRouterSelection {
            host_scopes: vec!["desktop".to_string()],
            domains: vec![PluginRouterSelectionDomain {
                prompt_signals: vec!["kubernetes".to_string()],
                workspace_files: vec!["kustomization.yaml".to_string()],
                workspace_extensions: vec!["tf".to_string()],
                select: select.to_string(),
            }],
            ..PluginRouterSelection::default()
        }
    }

    #[test]
    fn desktop_client_with_apple_prompt_injects_top_level_orchestrator() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let skills = vec![make_skill(PRIMARY_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Create a new iOS SwiftUI app.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            &[],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert_eq!(
            vec![PRIMARY_TARGET_SKILL.to_string()],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn non_desktop_client_does_not_inject() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let skills = vec![make_skill(PRIMARY_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Create a new iOS SwiftUI app.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            &[],
            Some("codex-tui"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn disabled_experiment_does_not_inject() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let skills = vec![make_skill(PRIMARY_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Audit this UIKit screen.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            &[],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ false,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn apple_workspace_signal_injects_without_prompt_keywords() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let package_swift = tempdir.path().join("Package.swift");
        fs::write(&package_swift, "// marker").expect("write package marker");
        let skills = vec![make_skill(PRIMARY_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Review the current branch diff for bugs.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            &[],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert_eq!(1, injected.len());
    }

    #[test]
    fn apple_prompt_signal_requires_token_boundary() {
        assert!(text_contains_prompt_signal(
            "create an ios17 swiftui app",
            "ios"
        ));
        assert!(text_contains_prompt_signal(
            "fix xcodebuild logs",
            "xcodebuild"
        ));
        assert!(!text_contains_prompt_signal(
            "review axios client retries",
            "ios"
        ));
        assert!(!text_contains_prompt_signal(
            "inspect bios firmware notes",
            "ios"
        ));
    }

    #[test]
    fn disabled_skill_path_is_not_injected() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let skills = vec![make_skill(PRIMARY_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Create a new macOS SwiftUI app.".to_string(),
            text_elements: Vec::new(),
        }];
        let disabled_paths = HashSet::from([skill_path.clone()]);

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &disabled_paths,
            &[],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn router_selection_metadata_injects_configured_non_apple_domain() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let skill_name = "infra-workflow:infra-orchestrator";
        let skills = vec![make_skill(skill_name, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Review this Kubernetes rollout plan.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            &[router_selection(skill_name)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert_eq!(
            vec![skill_name.to_string()],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn router_selection_metadata_uses_workspace_signal() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let skill_name = "infra-workflow:infra-orchestrator";
        let skills = vec![make_skill(skill_name, &skill_path)];
        fs::write(tempdir.path().join("kustomization.yaml"), "resources: []")
            .expect("write marker");
        let inputs = vec![UserInput::Text {
            text: "Review the current branch diff for bugs.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            &[router_selection(skill_name)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert_eq!(1, injected.len());
    }

    #[test]
    fn router_selection_metadata_suppresses_when_structured_skill_selected() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let skill_name = "infra-workflow:infra-orchestrator";
        let skills = vec![make_skill(skill_name, &skill_path)];
        let inputs = vec![UserInput::Skill {
            name: "other-workflow:other-orchestrator".to_string(),
            path: tempdir.path().join("other/SKILL.md"),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            &[router_selection(skill_name)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert!(injected.is_empty());
    }
}
