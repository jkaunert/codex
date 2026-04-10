use crate::model::SkillMetadata;
use codex_protocol::protocol::SKILLS_INSTRUCTIONS_CLOSE_TAG;
use codex_protocol::protocol::SKILLS_INSTRUCTIONS_OPEN_TAG;

pub fn render_skills_section(
    implicit_skills: &[SkillMetadata],
    explicit_only_skills: &[SkillMetadata],
) -> Option<String> {
    if implicit_skills.is_empty() && explicit_only_skills.is_empty() {
        return None;
    }

    let mut lines: Vec<String> = Vec::new();
    lines.push("## Skills".to_string());
    lines.push("A skill is a set of local instructions to follow that is stored in a `SKILL.md` file. Below is the list of skills that can be used. Each entry includes a name, description, and file path so you can open the source for full instructions when using a specific skill.".to_string());
    if !implicit_skills.is_empty() {
        lines.push("### Available skills".to_string());
        render_skill_entries(&mut lines, implicit_skills);
    }

    if !explicit_only_skills.is_empty() {
        lines.push("### Explicit-only skills".to_string());
        lines.push("These skills are available in this session but are not eligible for automatic task matching. Use them only when the user names them explicitly, selects them directly, or another active skill/instruction explicitly routes through them.".to_string());
        render_skill_entries(&mut lines, explicit_only_skills);
    }

    lines.push("### How to use skills".to_string());
    lines.push(
        r###"- Discovery: The lists above are the skills available in this session (name + description + file path). Skill bodies live on disk at the listed paths.
- Trigger rules for available skills: If the user names a skill (with `$SkillName` or plain text) OR the task clearly matches a skill's description shown above, you must use that skill for that turn. Multiple mentions mean use them all. Do not carry skills across turns unless re-mentioned.
- Trigger rules for explicit-only skills: Use them only when the user names them directly, selects them through a structured skill mention, or another active skill/instruction explicitly tells you to route through them.
- Missing/blocked: If a named skill isn't in the list or the path can't be read, say so briefly and continue with the best fallback.
- How to use a skill (progressive disclosure):
  1) After deciding to use a skill, open its `SKILL.md`. Read only enough to follow the workflow.
  2) When `SKILL.md` references relative paths (e.g., `scripts/foo.py`), resolve them relative to the skill directory listed above first, and only consider other paths if needed.
  3) If `SKILL.md` points to extra folders such as `references/`, load only the specific files needed for the request; don't bulk-load everything.
  4) If `scripts/` exist, prefer running or patching them instead of retyping large code blocks.
  5) If `assets/` or templates exist, reuse them instead of recreating from scratch.
- Coordination and sequencing:
  - If multiple skills apply, choose the minimal set that covers the request and state the order you'll use them.
  - Announce which skill(s) you're using and why (one short line). If you skip an obvious skill, say why.
- Context hygiene:
  - Keep context small: summarize long sections instead of pasting them; only load extra files when needed.
  - Avoid deep reference-chasing: prefer opening only files directly linked from `SKILL.md` unless you're blocked.
  - When variants exist (frameworks, providers, domains), pick only the relevant reference file(s) and note that choice.
- Safety and fallback: If a skill can't be applied cleanly (missing files, unclear instructions), state the issue, pick the next-best approach, and continue."###
            .to_string(),
    );

    let body = lines.join("\n");
    Some(format!(
        "{SKILLS_INSTRUCTIONS_OPEN_TAG}\n{body}\n{SKILLS_INSTRUCTIONS_CLOSE_TAG}"
    ))
}

fn render_skill_entries(lines: &mut Vec<String>, skills: &[SkillMetadata]) {
    for skill in skills {
        let path_str = skill.path_to_skills_md.to_string_lossy().replace('\\', "/");
        let name = skill.name.as_str();
        let description = skill.description.as_str();
        lines.push(format!("- {name}: {description} (file: {path_str})"));
    }
}

#[cfg(test)]
mod tests {
    use super::render_skills_section;
    use crate::SkillMetadata;
    use crate::SkillPolicy;
    use codex_protocol::protocol::SkillScope;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    fn skill(
        name: &str,
        description: &str,
        path: &str,
        allow_implicit: Option<bool>,
    ) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            description: description.to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: allow_implicit.map(|allow_implicit_invocation| SkillPolicy {
                allow_implicit_invocation: Some(allow_implicit_invocation),
                products: Vec::new(),
            }),
            path_to_skills_md: PathBuf::from(path),
            scope: SkillScope::User,
        }
    }

    #[test]
    fn render_skills_section_returns_none_when_no_skills_are_available() {
        assert_eq!(render_skills_section(&[], &[]), None);
    }

    #[test]
    fn render_skills_section_renders_implicit_and_explicit_only_sections() {
        let available = skill("alpha", "alpha skill", "/tmp/alpha/SKILL.md", None);
        let explicit_only = skill("beta", "beta skill", "/tmp/beta/SKILL.md", Some(false));

        let rendered = render_skills_section(&[available], &[explicit_only])
            .expect("skills section should render");

        let expected = "<skills_instructions>\n## Skills\nA skill is a set of local instructions to follow that is stored in a `SKILL.md` file. Below is the list of skills that can be used. Each entry includes a name, description, and file path so you can open the source for full instructions when using a specific skill.\n### Available skills\n- alpha: alpha skill (file: /tmp/alpha/SKILL.md)\n### Explicit-only skills\nThese skills are available in this session but are not eligible for automatic task matching. Use them only when the user names them explicitly, selects them directly, or another active skill/instruction explicitly routes through them.\n- beta: beta skill (file: /tmp/beta/SKILL.md)\n### How to use skills\n- Discovery: The lists above are the skills available in this session (name + description + file path). Skill bodies live on disk at the listed paths.\n- Trigger rules for available skills: If the user names a skill (with `$SkillName` or plain text) OR the task clearly matches a skill's description shown above, you must use that skill for that turn. Multiple mentions mean use them all. Do not carry skills across turns unless re-mentioned.\n- Trigger rules for explicit-only skills: Use them only when the user names them directly, selects them through a structured skill mention, or another active skill/instruction explicitly tells you to route through them.\n- Missing/blocked: If a named skill isn't in the list or the path can't be read, say so briefly and continue with the best fallback.\n- How to use a skill (progressive disclosure):\n  1) After deciding to use a skill, open its `SKILL.md`. Read only enough to follow the workflow.\n  2) When `SKILL.md` references relative paths (e.g., `scripts/foo.py`), resolve them relative to the skill directory listed above first, and only consider other paths if needed.\n  3) If `SKILL.md` points to extra folders such as `references/`, load only the specific files needed for the request; don't bulk-load everything.\n  4) If `scripts/` exist, prefer running or patching them instead of retyping large code blocks.\n  5) If `assets/` or templates exist, reuse them instead of recreating from scratch.\n- Coordination and sequencing:\n  - If multiple skills apply, choose the minimal set that covers the request and state the order you'll use them.\n  - Announce which skill(s) you're using and why (one short line). If you skip an obvious skill, say why.\n- Context hygiene:\n  - Keep context small: summarize long sections instead of pasting them; only load extra files when needed.\n  - Avoid deep reference-chasing: prefer opening only files directly linked from `SKILL.md` unless you're blocked.\n  - When variants exist (frameworks, providers, domains), pick only the relevant reference file(s) and note that choice.\n- Safety and fallback: If a skill can't be applied cleanly (missing files, unclear instructions), state the issue, pick the next-best approach, and continue.\n</skills_instructions>";

        assert_eq!(rendered, expected);
    }
}
