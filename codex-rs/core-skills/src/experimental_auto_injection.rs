use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::SkillMetadata;
use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::AbsolutePathBuf;

const DESKTOP_CLIENT_SIGNAL: &str = "desktop";
const PRIMARY_TARGET_SKILL: &str = "apple-appdev-workflow:apple-app-orchestrator";
const FALLBACK_TARGET_SKILL: &str = "apple-app-orchestrator";
const REVIEW_PRIMARY_TARGET_SKILL: &str = "apple-appdev-workflow:apple-review-orchestrator";
const REVIEW_FALLBACK_TARGET_SKILL: &str = "apple-review-orchestrator";
const DEBUG_PRIMARY_TARGET_SKILL: &str = "apple-appdev-workflow:apple-debug-orchestrator";
const DEBUG_FALLBACK_TARGET_SKILL: &str = "apple-debug-orchestrator";
const RELEASE_PRIMARY_TARGET_SKILL: &str = "apple-appdev-workflow:apple-release-orchestrator";
const RELEASE_FALLBACK_TARGET_SKILL: &str = "apple-release-orchestrator";
const ARCHITECTURE_PRIMARY_TARGET_SKILL: &str =
    "apple-appdev-workflow:apple-architecture-orchestrator";
const ARCHITECTURE_FALLBACK_TARGET_SKILL: &str = "apple-architecture-orchestrator";
const PERSISTENCE_PRIMARY_TARGET_SKILL: &str =
    "apple-appdev-workflow:apple-persistence-orchestrator";
const PERSISTENCE_FALLBACK_TARGET_SKILL: &str = "apple-persistence-orchestrator";
const PRODUCT_SURFACE_PRIMARY_TARGET_SKILL: &str =
    "apple-appdev-workflow:apple-product-surface-orchestrator";
const PRODUCT_SURFACE_FALLBACK_TARGET_SKILL: &str = "apple-product-surface-orchestrator";
const APPLE_PLUGIN_NAMESPACE_SIGNAL: &str = "apple-appdev-workflow:";
const APPLE_BARE_SKILL_SIGNALS: &[&str] = &[
    "apple-architecture-orchestrator",
    "apple-product-surface-orchestrator",
    "apple-review-orchestrator",
    "apple-debug-orchestrator",
    "apple-release-orchestrator",
    "apple-persistence-orchestrator",
    "apple-bootstrap-orchestrator",
    "apple-accessibility-orchestrator",
];
const APPLE_PROMPT_SIGNALS: &[&str] = &[
    "ios",
    "macos",
    "swiftui",
    "swiftdata",
    "uikit",
    "appkit",
    "xcode",
    "testflight",
    "bundle id",
    "app store",
    "core data",
    "apple app",
];
const REVIEW_PROMPT_SIGNALS: &[&str] = &[
    "review the current branch diff",
    "review this branch diff",
    "precommit review",
    "premerge review",
    "before commit",
    "precommit",
    "premerge",
    "missing tests",
    "regressions",
];
const STRONG_REVIEW_PROMPT_SIGNALS: &[&str] = &[
    "review the current branch diff",
    "review this branch diff",
    "precommit review",
    "premerge review",
];
const DEBUG_PROMPT_SIGNALS: &[&str] = &[
    "diagnose",
    "debug this",
    "runtime issue",
    "likely cause",
    "reproduce",
    "crash",
    "hang",
    "broken flow",
];
const RELEASE_PROMPT_SIGNALS: &[&str] = &[
    "release readiness",
    "ready to ship",
    "ready only for narrower distribution",
    "no-go",
    "go/no-go",
    "final validation",
    "ship readiness",
    "release doctor",
    "ship-candidate",
];
const ARCHITECTURE_PROMPT_SIGNALS: &[&str] = &[
    "program architecture",
    "app architecture",
    "app structure",
    "shared packages",
    "cross-app boundaries",
    "root anchoring",
    "ownership seams",
    "dependency direction",
];
const PERSISTENCE_PROMPT_SIGNALS: &[&str] = &[
    "swiftdata model",
    "migration approach",
    "data-loss risk",
    "rollout risk",
    "legacy core data import path",
    "coexistence",
    "schema migration",
    "versionedschema",
    "schemamigrationplan",
];
const PRODUCT_SURFACE_PROMPT_SIGNALS: &[&str] = &[
    "main product surface",
    "empty states",
    "on-screen copy",
    "onboarding-to-home story",
    "surface coherence",
    "liquid glass",
];
const APPLE_WORKSPACE_MARKERS: &[&str] = &["Package.swift"];
const APPLE_WORKSPACE_EXTENSIONS: &[&str] = &["xcodeproj", "xcworkspace"];

/// Experimental desktop-only deterministic top-level skill injection.
///
/// This helper is intentionally conservative:
/// - off unless explicitly enabled by config
/// - scoped to desktop-like app-server clients
/// - only activates when no explicit skill already resolved upstream
/// - injects at most one top-level Apple orchestrator plus one matched domain
///   orchestrator
pub fn maybe_collect_desktop_top_level_skill_injection(
    inputs: &[UserInput],
    skills: &[SkillMetadata],
    disabled_paths: &HashSet<AbsolutePathBuf>,
    client_name: Option<&str>,
    cwd: &Path,
    enabled: bool,
) -> Vec<SkillMetadata> {
    if !enabled || !is_desktop_client(client_name) {
        return Vec::new();
    }

    if !has_apple_prompt_signal(inputs) && !has_apple_workspace_signal(cwd) {
        return Vec::new();
    }

    let Some(app_skill) = find_enabled_skill(
        skills,
        disabled_paths,
        PRIMARY_TARGET_SKILL,
        FALLBACK_TARGET_SKILL,
    ) else {
        return Vec::new();
    };

    let mut injected = vec![app_skill];
    if let Some((primary, fallback)) = matched_domain_target(inputs)
        && let Some(domain_skill) = find_enabled_skill(skills, disabled_paths, primary, fallback)
        && !injected
            .iter()
            .any(|existing| existing.path_to_skills_md == domain_skill.path_to_skills_md)
    {
        injected.push(domain_skill);
    }

    injected
}

/// Experimental desktop-only parent-orchestrator augmentation for explicit
/// domain-orchestrator mentions.
///
/// This preserves the same top-level ownership shape as broad deterministic
/// injection without dragging leaf specialists into the payload. If a desktop
/// turn explicitly resolves to a known top-level Apple domain orchestrator and
/// does not already include the Apple app orchestrator, inject the missing
/// parent.
pub fn maybe_collect_desktop_explicit_parent_orchestrator_augmentation(
    explicit_skills: &[SkillMetadata],
    skills: &[SkillMetadata],
    disabled_paths: &HashSet<AbsolutePathBuf>,
    client_name: Option<&str>,
    enabled: bool,
) -> Vec<SkillMetadata> {
    if !enabled || !is_desktop_client(client_name) || explicit_skills.is_empty() {
        return Vec::new();
    }

    if explicit_skills.iter().any(|skill| {
        matches_named_skill(
            skill.name.as_str(),
            PRIMARY_TARGET_SKILL,
            FALLBACK_TARGET_SKILL,
        )
    }) {
        return Vec::new();
    }

    if !explicit_skills
        .iter()
        .any(|skill| matches_supported_domain_orchestrator(skill.name.as_str()))
    {
        return Vec::new();
    }

    find_enabled_skill(
        skills,
        disabled_paths,
        PRIMARY_TARGET_SKILL,
        FALLBACK_TARGET_SKILL,
    )
    .into_iter()
    .collect()
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
            lowered.contains(APPLE_PLUGIN_NAMESPACE_SIGNAL)
                || APPLE_BARE_SKILL_SIGNALS
                    .iter()
                    .any(|signal| lowered.contains(signal))
                || APPLE_PROMPT_SIGNALS
                    .iter()
                    .any(|signal| lowered.contains(signal))
        }
        UserInput::Image { .. }
        | UserInput::LocalImage { .. }
        | UserInput::Skill { .. }
        | UserInput::Mention { .. }
        | _ => false,
    })
}

fn matched_domain_target(inputs: &[UserInput]) -> Option<(&'static str, &'static str)> {
    let release_named = text_inputs_contain_any(
        inputs,
        &[RELEASE_PRIMARY_TARGET_SKILL, RELEASE_FALLBACK_TARGET_SKILL],
    );
    let architecture_named = text_inputs_contain_any(
        inputs,
        &[
            ARCHITECTURE_PRIMARY_TARGET_SKILL,
            ARCHITECTURE_FALLBACK_TARGET_SKILL,
        ],
    );
    let persistence_named = text_inputs_contain_any(
        inputs,
        &[
            PERSISTENCE_PRIMARY_TARGET_SKILL,
            PERSISTENCE_FALLBACK_TARGET_SKILL,
        ],
    );
    let product_surface_named = text_inputs_contain_any(
        inputs,
        &[
            PRODUCT_SURFACE_PRIMARY_TARGET_SKILL,
            PRODUCT_SURFACE_FALLBACK_TARGET_SKILL,
        ],
    );
    let debug_named = text_inputs_contain_any(
        inputs,
        &[DEBUG_PRIMARY_TARGET_SKILL, DEBUG_FALLBACK_TARGET_SKILL],
    );
    let review_named = text_inputs_contain_any(
        inputs,
        &[REVIEW_PRIMARY_TARGET_SKILL, REVIEW_FALLBACK_TARGET_SKILL],
    );
    let release_prompt = text_inputs_contain_any(inputs, RELEASE_PROMPT_SIGNALS);
    let architecture_prompt = text_inputs_contain_any(inputs, ARCHITECTURE_PROMPT_SIGNALS);
    let persistence_prompt = text_inputs_contain_any(inputs, PERSISTENCE_PROMPT_SIGNALS);
    let product_surface_prompt = text_inputs_contain_any(inputs, PRODUCT_SURFACE_PROMPT_SIGNALS);
    let debug_prompt = text_inputs_contain_any(inputs, DEBUG_PROMPT_SIGNALS);
    let review_prompt = text_inputs_contain_any(inputs, REVIEW_PROMPT_SIGNALS);
    let strong_review_prompt = text_inputs_contain_any(inputs, STRONG_REVIEW_PROMPT_SIGNALS);

    if release_named || release_prompt {
        return Some((RELEASE_PRIMARY_TARGET_SKILL, RELEASE_FALLBACK_TARGET_SKILL));
    }

    if architecture_named {
        return Some((
            ARCHITECTURE_PRIMARY_TARGET_SKILL,
            ARCHITECTURE_FALLBACK_TARGET_SKILL,
        ));
    }

    if persistence_named {
        return Some((
            PERSISTENCE_PRIMARY_TARGET_SKILL,
            PERSISTENCE_FALLBACK_TARGET_SKILL,
        ));
    }

    if product_surface_named {
        return Some((
            PRODUCT_SURFACE_PRIMARY_TARGET_SKILL,
            PRODUCT_SURFACE_FALLBACK_TARGET_SKILL,
        ));
    }

    if debug_named {
        return Some((DEBUG_PRIMARY_TARGET_SKILL, DEBUG_FALLBACK_TARGET_SKILL));
    }

    if review_named || strong_review_prompt {
        return Some((REVIEW_PRIMARY_TARGET_SKILL, REVIEW_FALLBACK_TARGET_SKILL));
    }

    if architecture_prompt {
        return Some((
            ARCHITECTURE_PRIMARY_TARGET_SKILL,
            ARCHITECTURE_FALLBACK_TARGET_SKILL,
        ));
    }

    if persistence_prompt {
        return Some((
            PERSISTENCE_PRIMARY_TARGET_SKILL,
            PERSISTENCE_FALLBACK_TARGET_SKILL,
        ));
    }

    if product_surface_prompt {
        return Some((
            PRODUCT_SURFACE_PRIMARY_TARGET_SKILL,
            PRODUCT_SURFACE_FALLBACK_TARGET_SKILL,
        ));
    }

    if debug_prompt {
        return Some((DEBUG_PRIMARY_TARGET_SKILL, DEBUG_FALLBACK_TARGET_SKILL));
    }

    if review_prompt {
        return Some((REVIEW_PRIMARY_TARGET_SKILL, REVIEW_FALLBACK_TARGET_SKILL));
    }

    None
}

fn text_inputs_contain_any(inputs: &[UserInput], signals: &[&str]) -> bool {
    inputs.iter().any(|input| match input {
        UserInput::Text { text, .. } => {
            let lowered = text.to_ascii_lowercase();
            signals.iter().any(|signal| lowered.contains(signal))
        }
        UserInput::Image { .. }
        | UserInput::LocalImage { .. }
        | UserInput::Skill { .. }
        | UserInput::Mention { .. }
        | _ => false,
    })
}

fn has_apple_workspace_signal(cwd: &Path) -> bool {
    path_has_apple_workspace_signal(cwd, 1)
}

fn path_has_apple_workspace_signal(path: &Path, remaining_child_depth: usize) -> bool {
    let Ok(entries) = fs::read_dir(path) else {
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
        if path
            .extension()
            .and_then(|value| value.to_str())
            .map(|ext| APPLE_WORKSPACE_EXTENSIONS.contains(&ext))
            .unwrap_or(false)
        {
            return true;
        }
        remaining_child_depth > 0
            && path.is_dir()
            && !name.starts_with('.')
            && path_has_apple_workspace_signal(&path, remaining_child_depth - 1)
    })
}

fn matches_named_skill(name: &str, primary: &str, fallback: &str) -> bool {
    name == primary || name == fallback || name.ends_with(&format!(":{fallback}"))
}

fn matches_supported_domain_orchestrator(name: &str) -> bool {
    matches_named_skill(
        name,
        REVIEW_PRIMARY_TARGET_SKILL,
        REVIEW_FALLBACK_TARGET_SKILL,
    ) || matches_named_skill(
        name,
        DEBUG_PRIMARY_TARGET_SKILL,
        DEBUG_FALLBACK_TARGET_SKILL,
    ) || matches_named_skill(
        name,
        RELEASE_PRIMARY_TARGET_SKILL,
        RELEASE_FALLBACK_TARGET_SKILL,
    ) || matches_named_skill(
        name,
        ARCHITECTURE_PRIMARY_TARGET_SKILL,
        ARCHITECTURE_FALLBACK_TARGET_SKILL,
    ) || matches_named_skill(
        name,
        PERSISTENCE_PRIMARY_TARGET_SKILL,
        PERSISTENCE_FALLBACK_TARGET_SKILL,
    ) || matches_named_skill(
        name,
        PRODUCT_SURFACE_PRIMARY_TARGET_SKILL,
        PRODUCT_SURFACE_FALLBACK_TARGET_SKILL,
    )
}

fn find_enabled_skill(
    skills: &[SkillMetadata],
    disabled_paths: &HashSet<AbsolutePathBuf>,
    primary: &str,
    fallback: &str,
) -> Option<SkillMetadata> {
    skills
        .iter()
        .find(|skill| {
            !disabled_paths.contains(&skill.path_to_skills_md)
                && matches_named_skill(skill.name.as_str(), primary, fallback)
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::protocol::SkillScope;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn make_skill(name: &str, path: &Path) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            description: format!("{name} skill"),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            path_to_skills_md: AbsolutePathBuf::try_from(path.to_path_buf())
                .expect("absolute skill path"),
            scope: SkillScope::User,
        }
    }

    #[test]
    fn desktop_client_with_apple_prompt_injects_top_level_orchestrator() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path = tempdir.path().join("SKILL.md");
        let skills = vec![make_skill(PRIMARY_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Create a new iOS SwiftUI app.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
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
        let skill_path = tempdir.path().join("SKILL.md");
        let skills = vec![make_skill(PRIMARY_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Create a new iOS SwiftUI app.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("codex-tui"),
            tempdir.path(),
            true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn disabled_experiment_does_not_inject() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path = tempdir.path().join("SKILL.md");
        let skills = vec![make_skill(PRIMARY_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Audit this UIKit screen.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            false,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn apple_workspace_signal_injects_without_prompt_keywords() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path = tempdir.path().join("SKILL.md");
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
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert_eq!(1, injected.len());
    }

    #[test]
    fn namespaced_plugin_prompt_injects_without_other_apple_keywords() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let review_skill_path = tempdir.path().join("review.md");
        let skills = vec![
            make_skill(PRIMARY_TARGET_SKILL, &app_skill_path),
            make_skill(REVIEW_PRIMARY_TARGET_SKILL, &review_skill_path),
        ];
        let inputs = vec![UserInput::Text {
            text: "Use apple-appdev-workflow:apple-review-orchestrator to review the current branch diff."
                .to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert_eq!(
            vec![
                PRIMARY_TARGET_SKILL.to_string(),
                REVIEW_PRIMARY_TARGET_SKILL.to_string(),
            ],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn child_workspace_signal_injects_from_parent_directory() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path = tempdir.path().join("SKILL.md");
        let child = tempdir.path().join("Wayfinder");
        fs::create_dir_all(&child).expect("child workspace dir");
        fs::create_dir(child.join("Wayfinder.xcodeproj")).expect("child xcodeproj marker");
        let skills = vec![make_skill(PRIMARY_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Review the current branch diff for bugs.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert_eq!(1, injected.len());
    }

    #[test]
    fn disabled_skill_path_is_not_injected() {
        let tempdir = TempDir::new().expect("tempdir");
        let skill_path = tempdir.path().join("SKILL.md");
        let skills = vec![make_skill(PRIMARY_TARGET_SKILL, &skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Create a new macOS SwiftUI app.".to_string(),
            text_elements: Vec::new(),
        }];
        let disabled_paths =
            HashSet::from([AbsolutePathBuf::try_from(skill_path.clone())
                .expect("absolute disabled skill path")]);

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &disabled_paths,
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn review_prompt_injects_app_and_review_orchestrators() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let review_skill_path = tempdir.path().join("review.md");
        fs::write(tempdir.path().join("Package.swift"), "// marker").expect("write package marker");
        let skills = vec![
            make_skill(PRIMARY_TARGET_SKILL, &app_skill_path),
            make_skill(REVIEW_PRIMARY_TARGET_SKILL, &review_skill_path),
        ];
        let inputs = vec![UserInput::Text {
            text: "Review the current branch diff for bugs, risks, regressions, and missing tests before commit."
                .to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert_eq!(
            vec![
                PRIMARY_TARGET_SKILL.to_string(),
                REVIEW_PRIMARY_TARGET_SKILL.to_string(),
            ],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn architecture_prompt_injects_app_and_architecture_orchestrators() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let architecture_skill_path = tempdir.path().join("architecture.md");
        fs::write(tempdir.path().join("Package.swift"), "// marker").expect("write package marker");
        let skills = vec![
            make_skill(PRIMARY_TARGET_SKILL, &app_skill_path),
            make_skill(ARCHITECTURE_PRIMARY_TARGET_SKILL, &architecture_skill_path),
        ];
        let inputs = vec![UserInput::Text {
            text: "Assess the Wayfinder and WayfinderDesk program architecture and tell me what should change first so shared packages, root anchoring, and cross-app boundaries stay coherent."
                .to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert_eq!(
            vec![
                PRIMARY_TARGET_SKILL.to_string(),
                ARCHITECTURE_PRIMARY_TARGET_SKILL.to_string(),
            ],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn persistence_prompt_injects_app_and_persistence_orchestrators() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let persistence_skill_path = tempdir.path().join("persistence.md");
        fs::write(tempdir.path().join("Package.swift"), "// marker").expect("write package marker");
        let skills = vec![
            make_skill(PRIMARY_TARGET_SKILL, &app_skill_path),
            make_skill(PERSISTENCE_PRIMARY_TARGET_SKILL, &persistence_skill_path),
        ];
        let inputs = vec![UserInput::Text {
            text: "Review the SwiftData model and migration approach for queued sync records and the legacy Core Data import path, and recommend what should change first to reduce rollout and data-loss risk."
                .to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert_eq!(
            vec![
                PRIMARY_TARGET_SKILL.to_string(),
                PERSISTENCE_PRIMARY_TARGET_SKILL.to_string(),
            ],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn product_surface_prompt_injects_app_and_product_surface_orchestrators() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let product_surface_skill_path = tempdir.path().join("product-surface.md");
        fs::write(tempdir.path().join("Package.swift"), "// marker").expect("write package marker");
        let skills = vec![
            make_skill(PRIMARY_TARGET_SKILL, &app_skill_path),
            make_skill(
                PRODUCT_SURFACE_PRIMARY_TARGET_SKILL,
                &product_surface_skill_path,
            ),
        ];
        let inputs = vec![UserInput::Text {
            text: "Review this SwiftUI app's main product surface and recommend what should change first so the navigation, empty states, and on-screen copy feel more coherent and production-ready. Also tell me where Liquid Glass would help and where it would be a mistake."
                .to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert_eq!(
            vec![
                PRIMARY_TARGET_SKILL.to_string(),
                PRODUCT_SURFACE_PRIMARY_TARGET_SKILL.to_string(),
            ],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn debug_prompt_injects_app_and_debug_orchestrators() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let debug_skill_path = tempdir.path().join("debug.md");
        let skills = vec![
            make_skill(PRIMARY_TARGET_SKILL, &app_skill_path),
            make_skill(DEBUG_PRIMARY_TARGET_SKILL, &debug_skill_path),
        ];
        let inputs = vec![UserInput::Text {
            text: "Diagnose this iOS runtime issue and tell me the likely cause.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert_eq!(
            vec![
                PRIMARY_TARGET_SKILL.to_string(),
                DEBUG_PRIMARY_TARGET_SKILL.to_string(),
            ],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn release_prompt_injects_app_and_release_orchestrators() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let release_skill_path = tempdir.path().join("release.md");
        fs::write(tempdir.path().join("Package.swift"), "// marker").expect("write package marker");
        let skills = vec![
            make_skill(PRIMARY_TARGET_SKILL, &app_skill_path),
            make_skill(RELEASE_PRIMARY_TARGET_SKILL, &release_skill_path),
        ];
        let inputs = vec![UserInput::Text {
            text: "Run a release-readiness review for this branch and tell me if we are actually ready to ship."
                .to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert_eq!(
            vec![
                PRIMARY_TARGET_SKILL.to_string(),
                RELEASE_PRIMARY_TARGET_SKILL.to_string(),
            ],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn release_signals_take_priority_over_review_signals() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let review_skill_path = tempdir.path().join("review.md");
        let release_skill_path = tempdir.path().join("release.md");
        fs::write(tempdir.path().join("Package.swift"), "// marker").expect("write package marker");
        let skills = vec![
            make_skill(PRIMARY_TARGET_SKILL, &app_skill_path),
            make_skill(REVIEW_PRIMARY_TARGET_SKILL, &review_skill_path),
            make_skill(RELEASE_PRIMARY_TARGET_SKILL, &release_skill_path),
        ];
        let inputs = vec![UserInput::Text {
            text: "Run a release-readiness review for this branch and tell me if we are ready to ship."
                .to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert_eq!(
            vec![
                PRIMARY_TARGET_SKILL.to_string(),
                RELEASE_PRIMARY_TARGET_SKILL.to_string(),
            ],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn strong_review_signals_take_priority_over_generic_debug_signals() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let review_skill_path = tempdir.path().join("review.md");
        let debug_skill_path = tempdir.path().join("debug.md");
        fs::write(tempdir.path().join("Package.swift"), "// marker").expect("write package marker");
        let skills = vec![
            make_skill(PRIMARY_TARGET_SKILL, &app_skill_path),
            make_skill(REVIEW_PRIMARY_TARGET_SKILL, &review_skill_path),
            make_skill(DEBUG_PRIMARY_TARGET_SKILL, &debug_skill_path),
        ];
        let inputs = vec![UserInput::Text {
            text: "Review the current branch diff for bugs, regressions, and missing tests before commit, then identify the likely cause of anything suspicious."
                .to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert_eq!(
            vec![
                PRIMARY_TARGET_SKILL.to_string(),
                REVIEW_PRIMARY_TARGET_SKILL.to_string(),
            ],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn explicit_debug_skill_signal_still_beats_review_prompt_shape() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let review_skill_path = tempdir.path().join("review.md");
        let debug_skill_path = tempdir.path().join("debug.md");
        fs::write(tempdir.path().join("Package.swift"), "// marker").expect("write package marker");
        let skills = vec![
            make_skill(PRIMARY_TARGET_SKILL, &app_skill_path),
            make_skill(REVIEW_PRIMARY_TARGET_SKILL, &review_skill_path),
            make_skill(DEBUG_PRIMARY_TARGET_SKILL, &debug_skill_path),
        ];
        let inputs = vec![UserInput::Text {
            text: "Use apple-appdev-workflow:apple-debug-orchestrator to diagnose this crash before commit."
                .to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
        );

        assert_eq!(
            vec![
                PRIMARY_TARGET_SKILL.to_string(),
                DEBUG_PRIMARY_TARGET_SKILL.to_string(),
            ],
            injected
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn missing_domain_orchestrator_keeps_app_only_injection() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        fs::write(tempdir.path().join("Package.swift"), "// marker").expect("write package marker");
        let skills = vec![make_skill(PRIMARY_TARGET_SKILL, &app_skill_path)];
        let inputs = vec![UserInput::Text {
            text: "Review the current branch diff for bugs before commit.".to_string(),
            text_elements: Vec::new(),
        }];

        let injected = maybe_collect_desktop_top_level_skill_injection(
            &inputs,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            tempdir.path(),
            true,
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
    fn explicit_release_orchestrator_augments_missing_app_parent() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let release_skill_path = tempdir.path().join("release.md");
        let explicit_skills = vec![make_skill(
            RELEASE_PRIMARY_TARGET_SKILL,
            &release_skill_path,
        )];
        let skills = vec![
            make_skill(PRIMARY_TARGET_SKILL, &app_skill_path),
            make_skill(RELEASE_PRIMARY_TARGET_SKILL, &release_skill_path),
        ];

        let injected = maybe_collect_desktop_explicit_parent_orchestrator_augmentation(
            &explicit_skills,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            true,
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
    fn explicit_specialist_does_not_augment_app_parent() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let specialist_skill_path = tempdir.path().join("release-ops.md");
        let explicit_skills = vec![make_skill(
            "apple-appdev-workflow:apple-build-release-ops",
            &specialist_skill_path,
        )];
        let skills = vec![make_skill(PRIMARY_TARGET_SKILL, &app_skill_path)];

        let injected = maybe_collect_desktop_explicit_parent_orchestrator_augmentation(
            &explicit_skills,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            true,
        );

        assert!(injected.is_empty());
    }

    #[test]
    fn explicit_domain_with_existing_app_does_not_duplicate_parent() {
        let tempdir = TempDir::new().expect("tempdir");
        let app_skill_path = tempdir.path().join("app.md");
        let review_skill_path = tempdir.path().join("review.md");
        let explicit_skills = vec![
            make_skill(PRIMARY_TARGET_SKILL, &app_skill_path),
            make_skill(REVIEW_PRIMARY_TARGET_SKILL, &review_skill_path),
        ];
        let skills = explicit_skills.clone();

        let injected = maybe_collect_desktop_explicit_parent_orchestrator_augmentation(
            &explicit_skills,
            &skills,
            &HashSet::new(),
            Some("desktop-client"),
            true,
        );

        assert!(injected.is_empty());
    }
}
