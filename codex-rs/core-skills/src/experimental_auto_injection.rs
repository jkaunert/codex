use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::SkillMetadata;
use codex_plugin::PluginRouterSelection;
use codex_plugin::PluginRouterSelectionDomain;
use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;

const DESKTOP_CLIENT_SIGNAL: &str = "desktop";
const ROUTE_ROLE_TOP_LEVEL_ORCHESTRATOR: &str = "top-level-orchestrator";
const ROUTE_ROLE_BRIGADE_ORCHESTRATOR: &str = "brigade-orchestrator";
const ROUTING_SCOPE_BROAD: &str = "broad";
const ROUTING_SCOPE_DOMAIN: &str = "domain";

/// Experimental desktop-only deterministic top-level skill injection.
///
/// This helper is intentionally conservative:
/// - off unless explicitly enabled by config
/// - scoped to desktop-like app-server clients
/// - does not duplicate an explicit top-level owner selection
/// - preserves narrow explicit specialist selections
/// - injects at most one manifest-configured top-level router skill
pub fn maybe_collect_desktop_top_level_skill_injection(
    inputs: &[UserInput],
    explicit_skills: &[SkillMetadata],
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
        explicit_skills,
        skills,
        disabled_paths,
        router_selections,
        client_name,
        cwd,
    );
    if !configured_injections.is_empty() {
        return configured_injections;
    }

    Vec::new()
}

fn collect_manifest_router_selection_injections(
    inputs: &[UserInput],
    explicit_skills: &[SkillMetadata],
    skills: &[SkillMetadata],
    disabled_paths: &HashSet<AbsolutePathBuf>,
    router_selections: &[PluginRouterSelection],
    client_name: Option<&str>,
    cwd: &Path,
) -> Vec<SkillMetadata> {
    for config in router_selections {
        if !client_matches_any_host_scope(client_name, &config.host_scopes) {
            continue;
        }

        for domain in &config.domains {
            if !has_configured_prompt_signal(inputs, &domain.prompt_signals)
                && !has_configured_workspace_signal(cwd, domain)
            {
                continue;
            }

            if config.suppression.when_explicit_skill_selected {
                match explicit_skill_router_decision(inputs, explicit_skills, &domain.select) {
                    ExplicitSkillRouterDecision::NoExplicitSkill
                    | ExplicitSkillRouterDecision::AllowParentInjection => {}
                    ExplicitSkillRouterDecision::OwnerAlreadySelected
                    | ExplicitSkillRouterDecision::SuppressParentInjection => continue,
                }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExplicitSkillRouterDecision {
    NoExplicitSkill,
    OwnerAlreadySelected,
    AllowParentInjection,
    SuppressParentInjection,
}

fn explicit_skill_router_decision(
    inputs: &[UserInput],
    explicit_skills: &[SkillMetadata],
    selected_owner: &str,
) -> ExplicitSkillRouterDecision {
    let mut saw_compatible_route_skill = false;
    let mut saw_explicit_skill = false;

    for input in inputs {
        let UserInput::Skill { name, path } = input else {
            continue;
        };
        saw_explicit_skill = true;

        if same_skill_identity(name, selected_owner) {
            return ExplicitSkillRouterDecision::OwnerAlreadySelected;
        }

        if explicit_skill_allows_parent_injection(name, path, selected_owner) {
            saw_compatible_route_skill = true;
        } else {
            return ExplicitSkillRouterDecision::SuppressParentInjection;
        }
    }

    for skill in explicit_skills {
        saw_explicit_skill = true;

        if same_skill_identity(&skill.name, selected_owner) {
            return ExplicitSkillRouterDecision::OwnerAlreadySelected;
        }

        if explicit_skill_allows_parent_injection(
            &skill.name,
            skill.path_to_skills_md.as_path(),
            selected_owner,
        ) {
            saw_compatible_route_skill = true;
        } else {
            return ExplicitSkillRouterDecision::SuppressParentInjection;
        }
    }

    if saw_compatible_route_skill {
        ExplicitSkillRouterDecision::AllowParentInjection
    } else if saw_explicit_skill {
        ExplicitSkillRouterDecision::SuppressParentInjection
    } else {
        ExplicitSkillRouterDecision::NoExplicitSkill
    }
}

fn explicit_skill_allows_parent_injection(
    explicit_skill_name: &str,
    explicit_skill_path: &Path,
    selected_owner: &str,
) -> bool {
    if !same_plugin_namespace(explicit_skill_name, selected_owner) {
        return false;
    }

    let Some(metadata) = read_skill_route_metadata(explicit_skill_path) else {
        return false;
    };

    metadata.is_broad_or_domain_route()
}

fn same_plugin_namespace(explicit_skill_name: &str, selected_owner: &str) -> bool {
    let Some((explicit_plugin, _)) = skill_name_without_control_prefix(explicit_skill_name)
        .trim()
        .split_once(':')
    else {
        return false;
    };
    let Some((owner_plugin, _)) = skill_name_without_control_prefix(selected_owner)
        .trim()
        .split_once(':')
    else {
        return false;
    };

    explicit_plugin == owner_plugin
}

fn same_skill_identity(candidate: &str, selected_owner: &str) -> bool {
    skill_name_without_control_prefix(candidate).trim()
        == skill_name_without_control_prefix(selected_owner).trim()
}

fn skill_name_without_control_prefix(name: &str) -> &str {
    let name = name.trim();
    name.strip_prefix('$').unwrap_or(name)
}

#[derive(Debug, Default, Deserialize)]
struct SkillFrontmatter {
    #[serde(default)]
    metadata: SkillRouteMetadata,
}

#[derive(Debug, Default, Deserialize)]
struct SkillRouteMetadata {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    routing_scope: Option<String>,
}

impl SkillRouteMetadata {
    fn is_broad_or_domain_route(&self) -> bool {
        field_matches_any(
            self.role.as_deref(),
            &[
                ROUTE_ROLE_TOP_LEVEL_ORCHESTRATOR,
                ROUTE_ROLE_BRIGADE_ORCHESTRATOR,
            ],
        ) || field_matches_any(
            self.routing_scope.as_deref(),
            &[ROUTING_SCOPE_BROAD, ROUTING_SCOPE_DOMAIN],
        )
    }
}

fn field_matches_any(value: Option<&str>, expected_values: &[&str]) -> bool {
    value.map(str::trim).is_some_and(|value| {
        expected_values
            .iter()
            .any(|expected| value.eq_ignore_ascii_case(expected))
    })
}

fn read_skill_route_metadata(path: &Path) -> Option<SkillRouteMetadata> {
    let contents = fs::read_to_string(path).ok()?;
    let frontmatter = extract_skill_frontmatter(&contents)?;
    serde_yaml::from_str::<SkillFrontmatter>(&frontmatter)
        .ok()
        .map(|frontmatter| frontmatter.metadata)
}

fn extract_skill_frontmatter(contents: &str) -> Option<String> {
    let mut lines = contents.lines();
    if !matches!(lines.next(), Some(line) if line.trim() == "---") {
        return None;
    }

    let mut frontmatter_lines: Vec<&str> = Vec::new();
    let mut found_closing = false;
    for line in lines.by_ref() {
        if line.trim() == "---" {
            found_closing = true;
            break;
        }
        frontmatter_lines.push(line);
    }

    if frontmatter_lines.is_empty() || !found_closing {
        return None;
    }

    Some(frontmatter_lines.join("\n"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::protocol::SkillScope;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    const APPLE_TARGET_SKILL: &str = "apple-appdev-workflow:apple-app-orchestrator";
    const APPLE_REVIEW_SKILL: &str = "apple-appdev-workflow:apple-review-orchestrator";
    const APPLE_DECISION_STRESS_SKILL: &str = "apple-appdev-workflow:apple-decision-stress-test";

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

    fn apple_router_selection(select: &str) -> PluginRouterSelection {
        PluginRouterSelection {
            host_scopes: vec!["desktop".to_string()],
            domains: vec![PluginRouterSelectionDomain {
                prompt_signals: vec!["ios".to_string(), "swiftui".to_string()],
                workspace_files: vec!["Package.swift".to_string()],
                workspace_extensions: vec!["xcodeproj".to_string(), "xcworkspace".to_string()],
                select: select.to_string(),
            }],
            ..PluginRouterSelection::default()
        }
    }

    fn write_skill_with_route_metadata(path: &Path, name: &str, role: &str, routing_scope: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create skill dir");
        }
        fs::write(
            path,
            format!(
                "---\nname: {name}\ndescription: test skill\nmetadata:\n  role: {role}\n  routing_scope: {routing_scope}\n---\n\n# Test Skill\n"
            ),
        )
        .expect("write skill");
    }

    fn write_raw_skill(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create skill dir");
        }
        fs::write(path, contents).expect("write skill");
    }

    fn injected_apple_skill_names(
        inputs: &[UserInput],
        skills: &[SkillMetadata],
        cwd: &Path,
    ) -> Vec<String> {
        injected_apple_skill_names_with_explicit(inputs, &[], skills, cwd)
    }

    fn injected_apple_skill_names_with_explicit(
        inputs: &[UserInput],
        explicit_skills: &[SkillMetadata],
        skills: &[SkillMetadata],
        cwd: &Path,
    ) -> Vec<String> {
        maybe_collect_desktop_top_level_skill_injection(
            inputs,
            explicit_skills,
            skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
            Some("desktop-client"),
            cwd,
            /*enabled*/ true,
        )
        .into_iter()
        .map(|skill| skill.name)
        .collect()
    }

    #[test]
    fn desktop_client_with_apple_prompt_requires_router_selection_metadata() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Create a new iOS SwiftUI app.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn non_desktop_client_does_not_inject() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Create a new iOS SwiftUI app.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
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
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Audit this UIKit screen.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
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
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Review the current branch diff for bugs.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert_eq!(1, injected.len());
    }

    #[test]
    fn router_selection_release_prompt_variant_matrix_matches_contract() {
        let tempdir = TempDir::new().expect("tempdir");
        let owner_path = AbsolutePathBuf::try_from(tempdir.path().join("owner/SKILL.md"))
            .expect("absolute path");
        let review_path = tempdir.path().join("review/SKILL.md");
        let decision_path = tempdir.path().join("decision/SKILL.md");
        write_skill_with_route_metadata(
            &review_path,
            APPLE_REVIEW_SKILL,
            "brigade-orchestrator",
            "domain",
        );
        write_skill_with_route_metadata(
            &decision_path,
            APPLE_DECISION_STRESS_SKILL,
            "specialist",
            "focused",
        );
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &owner_path)];

        let cases = [
            (
                "natural Apple prompt injects owner",
                vec![UserInput::Text {
                    text: "Review this iOS branch diff for bugs.".to_string(),
                    text_elements: Vec::new(),
                }],
                vec![APPLE_TARGET_SKILL],
            ),
            (
                "explicit top-level prompt does not duplicate owner",
                vec![
                    UserInput::Skill {
                        name: format!("${APPLE_TARGET_SKILL}"),
                        path: tempdir.path().join("owner/SKILL.md"),
                    },
                    UserInput::Text {
                        text: "Review this iOS branch diff for bugs.".to_string(),
                        text_elements: Vec::new(),
                    },
                ],
                vec![],
            ),
            (
                "explicit downstream domain prompt preserves owner",
                vec![
                    UserInput::Skill {
                        name: format!("${APPLE_REVIEW_SKILL}"),
                        path: review_path.clone(),
                    },
                    UserInput::Text {
                        text: "Review this iOS branch diff for bugs.".to_string(),
                        text_elements: Vec::new(),
                    },
                ],
                vec![APPLE_TARGET_SKILL],
            ),
            (
                "explicit narrow specialist prompt stays isolated",
                vec![
                    UserInput::Skill {
                        name: format!("${APPLE_DECISION_STRESS_SKILL}"),
                        path: decision_path,
                    },
                    UserInput::Text {
                        text: "Stress test this iOS architecture decision.".to_string(),
                        text_elements: Vec::new(),
                    },
                ],
                vec![],
            ),
        ];

        for (case, inputs, expected) in cases {
            assert_eq!(
                expected.into_iter().map(str::to_string).collect::<Vec<_>>(),
                injected_apple_skill_names(&inputs, &skills, tempdir.path()),
                "{case}"
            );
        }
    }

    #[test]
    fn explicit_top_level_skill_does_not_duplicate_owner_injection() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path = AbsolutePathBuf::try_from(tempdir.path().join("owner/SKILL.md"))
            .expect("absolute path");
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &skill_path)];
        let inputs = vec![
            UserInput::Skill {
                name: APPLE_TARGET_SKILL.to_string(),
                path: tempdir.path().join("owner/SKILL.md"),
            },
            UserInput::Text {
                text: "Review this iOS branch diff.".to_string(),
                text_elements: Vec::new(),
            },
        ];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn explicit_top_level_control_name_does_not_duplicate_owner_injection() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path = AbsolutePathBuf::try_from(tempdir.path().join("owner/SKILL.md"))
            .expect("absolute path");
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &skill_path)];
        let inputs = vec![
            UserInput::Skill {
                name: format!("${APPLE_TARGET_SKILL}"),
                path: tempdir.path().join("owner/SKILL.md"),
            },
            UserInput::Text {
                text: "Review this iOS branch diff.".to_string(),
                text_elements: Vec::new(),
            },
        ];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn explicit_domain_route_skill_preserves_parent_owner_injection() {
        let tempdir = TempDir::new().expect("tempdir");
        let owner_path = AbsolutePathBuf::try_from(tempdir.path().join("owner/SKILL.md"))
            .expect("absolute path");
        let review_path = tempdir.path().join("review/SKILL.md");
        write_skill_with_route_metadata(
            &review_path,
            APPLE_REVIEW_SKILL,
            "brigade-orchestrator",
            "domain",
        );
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &owner_path)];
        let inputs = vec![
            UserInput::Skill {
                name: APPLE_REVIEW_SKILL.to_string(),
                path: review_path,
            },
            UserInput::Text {
                text: "Review this iOS branch diff.".to_string(),
                text_elements: Vec::new(),
            },
        ];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert_eq!(
            vec![APPLE_TARGET_SKILL.to_string()],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn explicit_domain_route_skill_accepts_control_name_and_normalized_metadata() {
        let tempdir = TempDir::new().expect("tempdir");
        let owner_path = AbsolutePathBuf::try_from(tempdir.path().join("owner/SKILL.md"))
            .expect("absolute path");
        let review_path = tempdir.path().join("review/SKILL.md");
        write_raw_skill(
            &review_path,
            &format!(
                "---\r\nname: {APPLE_REVIEW_SKILL}\r\ndescription: test skill\r\nmetadata:\r\n  role: \" Brigade-Orchestrator \"\r\n  routing_scope: \" Domain \"\r\n---\r\n\r\n# Test Skill\r\n"
            ),
        );
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &owner_path)];
        let inputs = vec![
            UserInput::Skill {
                name: format!("${APPLE_REVIEW_SKILL}"),
                path: review_path,
            },
            UserInput::Text {
                text: "Review this iOS branch diff.".to_string(),
                text_elements: Vec::new(),
            },
        ];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert_eq!(
            vec![APPLE_TARGET_SKILL.to_string()],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn resolved_domain_route_skill_preserves_parent_owner_injection() {
        let tempdir = TempDir::new().expect("tempdir");
        let owner_path = AbsolutePathBuf::try_from(tempdir.path().join("owner/SKILL.md"))
            .expect("absolute path");
        let review_path = AbsolutePathBuf::try_from(tempdir.path().join("review/SKILL.md"))
            .expect("absolute path");
        write_skill_with_route_metadata(
            review_path.as_path(),
            APPLE_REVIEW_SKILL,
            "brigade-orchestrator",
            "domain",
        );
        let owner_skills = vec![make_skill(APPLE_TARGET_SKILL, &owner_path)];
        let explicit_skills = vec![make_skill(APPLE_REVIEW_SKILL, &review_path)];
        let inputs = vec![UserInput::Text {
            text: format!(
                "[${APPLE_REVIEW_SKILL}]({}) review this iOS branch diff.",
                review_path.display()
            ),
            text_elements: Vec::new(),
        }];

        assert_eq!(
            vec![APPLE_TARGET_SKILL.to_string()],
            injected_apple_skill_names_with_explicit(
                &inputs,
                &explicit_skills,
                &owner_skills,
                tempdir.path()
            )
        );
    }

    #[test]
    fn resolved_focused_specialist_suppresses_parent_owner_injection() {
        let tempdir = TempDir::new().expect("tempdir");
        let owner_path = AbsolutePathBuf::try_from(tempdir.path().join("owner/SKILL.md"))
            .expect("absolute path");
        let decision_path = AbsolutePathBuf::try_from(tempdir.path().join("decision/SKILL.md"))
            .expect("absolute path");
        write_skill_with_route_metadata(
            decision_path.as_path(),
            APPLE_DECISION_STRESS_SKILL,
            "specialist",
            "focused",
        );
        let owner_skills = vec![make_skill(APPLE_TARGET_SKILL, &owner_path)];
        let explicit_skills = vec![make_skill(APPLE_DECISION_STRESS_SKILL, &decision_path)];
        let inputs = vec![UserInput::Text {
            text: format!(
                "[${APPLE_DECISION_STRESS_SKILL}]({}) stress test this iOS review decision in isolation.",
                decision_path.display()
            ),
            text_elements: Vec::new(),
        }];

        assert_eq!(
            Vec::<String>::new(),
            injected_apple_skill_names_with_explicit(
                &inputs,
                &explicit_skills,
                &owner_skills,
                tempdir.path()
            )
        );
    }

    #[test]
    fn explicit_focused_specialist_suppresses_parent_owner_injection() {
        let tempdir = TempDir::new().expect("tempdir");
        let owner_path = AbsolutePathBuf::try_from(tempdir.path().join("owner/SKILL.md"))
            .expect("absolute path");
        let specialist_path = tempdir.path().join("decision/SKILL.md");
        write_skill_with_route_metadata(
            &specialist_path,
            APPLE_DECISION_STRESS_SKILL,
            "specialist",
            "focused",
        );
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &owner_path)];
        let inputs = vec![
            UserInput::Skill {
                name: APPLE_DECISION_STRESS_SKILL.to_string(),
                path: specialist_path,
            },
            UserInput::Text {
                text: "Stress test this iOS architecture decision.".to_string(),
                text_elements: Vec::new(),
            },
        ];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn explicit_focused_specialist_suppresses_even_with_compatible_domain_route() {
        let tempdir = TempDir::new().expect("tempdir");
        let owner_path = AbsolutePathBuf::try_from(tempdir.path().join("owner/SKILL.md"))
            .expect("absolute path");
        let review_path = tempdir.path().join("review/SKILL.md");
        let specialist_path = tempdir.path().join("decision/SKILL.md");
        write_skill_with_route_metadata(
            &review_path,
            APPLE_REVIEW_SKILL,
            "brigade-orchestrator",
            "domain",
        );
        write_skill_with_route_metadata(
            &specialist_path,
            APPLE_DECISION_STRESS_SKILL,
            "specialist",
            "focused",
        );
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &owner_path)];
        let inputs = vec![
            UserInput::Skill {
                name: APPLE_REVIEW_SKILL.to_string(),
                path: review_path,
            },
            UserInput::Skill {
                name: APPLE_DECISION_STRESS_SKILL.to_string(),
                path: specialist_path,
            },
            UserInput::Text {
                text: "Review and stress test this iOS decision.".to_string(),
                text_elements: Vec::new(),
            },
        ];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn explicit_same_plugin_skill_without_route_metadata_suppresses_parent_owner_injection() {
        let tempdir = TempDir::new().expect("tempdir");
        let owner_path = AbsolutePathBuf::try_from(tempdir.path().join("owner/SKILL.md"))
            .expect("absolute path");
        let skill_path = tempdir.path().join("unknown/SKILL.md");
        write_raw_skill(
            &skill_path,
            "---\nname: apple-appdev-workflow:unknown\ndescription: test skill\n---\n\n# Test Skill\n",
        );
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &owner_path)];
        let inputs = vec![
            UserInput::Skill {
                name: "apple-appdev-workflow:unknown".to_string(),
                path: skill_path,
            },
            UserInput::Text {
                text: "Review this iOS branch diff.".to_string(),
                text_elements: Vec::new(),
            },
        ];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn disabled_skill_path_is_not_injected() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Create a new macOS SwiftUI app.".to_string(),
            text_elements: Vec::new(),
        }];
        let disabled_paths = HashSet::from([skill_path.clone()]);

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &disabled_paths,
            &[apple_router_selection(APPLE_TARGET_SKILL)],
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
            &[],
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
            &[],
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
            &[],
            &skills,
            &HashSet::new(),
            &[router_selection(skill_name)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn apple_prompt_signal_requires_token_boundary() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Check whether axios bios and mission text should route.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn apple_prompt_signal_allows_platform_version_suffix() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path =
            AbsolutePathBuf::try_from(tempdir.path().join("SKILL.md")).expect("absolute path");
        let skills = vec![make_skill(APPLE_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Review iOS26 adoption risk.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &[],
            &skills,
            &HashSet::new(),
            &[apple_router_selection(APPLE_TARGET_SKILL)],
            Some("desktop-client"),
            tempdir.path(),
            /*enabled*/ true,
        );

        assert_eq!(1, injected.len());
    }
}
