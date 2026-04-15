#![cfg(not(target_os = "windows"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use anyhow::Result;
use codex_login::CodexAuth;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::user_input::UserInput;
use core_test_support::apps_test_server::AppsTestServer;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::stdio_server_bin;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use tempfile::TempDir;
use wiremock::MockServer;

const SAMPLE_PLUGIN_CONFIG_NAME: &str = "sample@test";
const SAMPLE_PLUGIN_DISPLAY_NAME: &str = "sample";
const SAMPLE_PLUGIN_DESCRIPTION: &str = "inspect sample data";
const APPLE_PLUGIN_CONFIG_NAME: &str = "apple-workflow@test";
const APPLE_PLUGIN_DISPLAY_NAME: &str = "apple-appdev-workflow";

fn write_skill(home: &std::path::Path, name: &str, description: &str, body: &str) {
    let skill_dir = home.join("skills").join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    let contents = format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n");
    std::fs::write(skill_dir.join("SKILL.md"), contents).unwrap();
}

fn write_skill_metadata(home: &std::path::Path, name: &str, contents: &str) {
    let metadata_dir = home.join("skills").join(name).join("agents");
    std::fs::create_dir_all(&metadata_dir).unwrap();
    std::fs::write(metadata_dir.join("openai.yaml"), contents).unwrap();
}

fn sample_plugin_root(home: &TempDir) -> std::path::PathBuf {
    home.path().join("plugins/cache/test/sample/local")
}

fn apple_plugin_root(home: &TempDir) -> std::path::PathBuf {
    home.path().join("plugins/cache/test/apple-workflow/local")
}

fn write_sample_plugin_manifest_and_config(home: &TempDir) -> std::path::PathBuf {
    let plugin_root = sample_plugin_root(home);
    std::fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("create plugin manifest dir");
    std::fs::write(
        plugin_root.join(".codex-plugin/plugin.json"),
        format!(
            r#"{{"name":"{SAMPLE_PLUGIN_DISPLAY_NAME}","description":"{SAMPLE_PLUGIN_DESCRIPTION}"}}"#
        ),
    )
    .expect("write plugin manifest");
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            "[features]\nplugins = true\n\n[plugins.\"{SAMPLE_PLUGIN_CONFIG_NAME}\"]\nenabled = true\n"
        ),
    )
    .expect("write config");
    plugin_root
}

fn write_apple_orchestrator_plugin(home: &TempDir) {
    let plugin_root = apple_plugin_root(home);
    std::fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("create plugin manifest dir");
    std::fs::write(
        plugin_root.join(".codex-plugin/plugin.json"),
        format!(
            r#"{{"name":"{APPLE_PLUGIN_DISPLAY_NAME}","description":"broad Apple workflow routing"}}"#
        ),
    )
    .expect("write plugin manifest");
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            "[features]\nplugins = true\n\n[plugins.\"{APPLE_PLUGIN_CONFIG_NAME}\"]\nenabled = true\n"
        ),
    )
    .expect("write config");

    let skill_dir = plugin_root.join("skills/apple-app-orchestrator");
    std::fs::create_dir_all(skill_dir.as_path()).expect("create plugin skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: broad Apple workflow routing\n---\n\n# apple app orchestrator body\n",
    )
    .expect("write plugin skill");
}

fn write_plugin_skill_plugin(home: &TempDir) {
    let plugin_root = write_sample_plugin_manifest_and_config(home);
    let skill_dir = plugin_root.join("skills/sample-search");
    std::fs::create_dir_all(skill_dir.as_path()).expect("create plugin skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: inspect sample data\n---\n\n# body\n",
    )
    .expect("write plugin skill");
}

fn write_plugin_mcp_plugin(home: &TempDir, command: &str) {
    let plugin_root = write_sample_plugin_manifest_and_config(home);
    std::fs::write(
        plugin_root.join(".mcp.json"),
        format!(
            r#"{{
  "mcpServers": {{
    "sample": {{
      "command": "{command}"
    }}
  }}
}}"#
        ),
    )
    .expect("write plugin mcp config");
}

fn write_plugin_app_plugin(home: &TempDir) {
    let plugin_root = write_sample_plugin_manifest_and_config(home);
    std::fs::write(
        plugin_root.join(".app.json"),
        r#"{
  "apps": {
    "calendar": {
      "id": "calendar"
    }
  }
}"#,
    )
    .expect("write plugin app config");
}

async fn build_apps_enabled_plugin_test_codex(
    server: &MockServer,
    codex_home: Arc<TempDir>,
    chatgpt_base_url: String,
) -> Result<Arc<codex_core::CodexThread>> {
    let mut builder = test_codex()
        .with_home(codex_home)
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config(move |config| {
            config
                .features
                .enable(Feature::Apps)
                .expect("test config should allow feature update");
            config.chatgpt_base_url = chatgpt_base_url;
        });
    Ok(builder
        .build(server)
        .await
        .expect("create new conversation")
        .codex)
}

use codex_features::Feature;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_turn_includes_skill_instructions_for_plain_text_name() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let skill_body = "skill body";
    let mut builder = test_codex().with_pre_build_hook(|home| {
        write_skill(home, "demo", "demo skill", skill_body);
    });
    let test = builder.build(&server).await?;

    let mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "done"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let session_model = test.session_configured.model.clone();
    test.codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "please use demo for this task".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.cwd_path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            approvals_reviewer: None,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            permission_profile: None,
            model: session_model,
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
            environments: None,
        })
        .await?;

    wait_for_event(test.codex.as_ref(), |event| {
        matches!(event, codex_protocol::protocol::EventMsg::TurnComplete(_))
    })
    .await;

    let request = mock.single_request();
    let user_texts = request.message_input_texts("user");
    assert!(
        user_texts
            .iter()
            .any(|text| text.contains("<skill>\n<name>demo</name>") && text.contains(skill_body)),
        "expected skill instructions in user input, got {user_texts:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initial_context_lists_explicit_only_skills_separately() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex().with_pre_build_hook(|home| {
        write_skill(home, "demo", "demo skill", "skill body");
        write_skill(home, "hidden", "hidden skill", "hidden body");
        write_skill_metadata(
            home,
            "hidden",
            "policy:\n  allow_implicit_invocation: false\n",
        );
    });
    let test = builder.build(&server).await?;

    let mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "done"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let session_model = test.session_configured.model.clone();
    test.codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "help me with this task".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.cwd_path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            approvals_reviewer: None,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            permission_profile: None,
            model: session_model,
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
            environments: None,
        })
        .await?;

    wait_for_event(test.codex.as_ref(), |event| {
        matches!(event, codex_protocol::protocol::EventMsg::TurnComplete(_))
    })
    .await;

    let request = mock.single_request();
    let developer_messages = request.message_input_texts("developer");
    assert!(
        developer_messages
            .iter()
            .any(|text| text.contains("### Explicit-only skills")
                && text.contains("- hidden: hidden skill")),
        "expected explicit-only skills section in developer prompt, got {developer_messages:?}"
    );
    assert!(
        developer_messages.iter().any(|text| {
            text.contains("Trigger rules for explicit-only skills")
                && text.contains(
                    "another active skill/instruction explicitly tells you to route through them",
                )
        }),
        "expected explicit-only trigger guidance in developer prompt, got {developer_messages:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_text_plugin_mentions_inject_plugin_guidance() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let apps_server = AppsTestServer::mount_with_connector_name(&server, "Google Calendar").await?;
    let mock = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let codex_home = Arc::new(TempDir::new()?);
    let rmcp_test_server_bin = match stdio_server_bin() {
        Ok(bin) => bin,
        Err(err) => {
            eprintln!("test_stdio_server binary not available, skipping test: {err}");
            return Ok(());
        }
    };
    write_plugin_skill_plugin(codex_home.as_ref());
    write_plugin_mcp_plugin(codex_home.as_ref(), &rmcp_test_server_bin);
    write_plugin_app_plugin(codex_home.as_ref());

    let codex =
        build_apps_enabled_plugin_test_codex(&server, codex_home, apps_server.chatgpt_base_url)
            .await?;

    codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "use sample for this task".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;
    wait_for_event(&codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let request = mock.single_request();
    let developer_messages = request.message_input_texts("developer");
    assert!(
        developer_messages
            .iter()
            .any(|text| text.contains("Skills from this plugin are prefixed with `sample:`")),
        "expected plugin skills guidance: {developer_messages:?}"
    );
    assert!(
        developer_messages
            .iter()
            .any(|text| text.contains("MCP servers from this plugin")),
        "expected visible plugin MCP guidance: {developer_messages:?}"
    );
    assert!(
        developer_messages
            .iter()
            .any(|text| text.contains("Apps from this plugin")),
        "expected visible plugin app guidance: {developer_messages:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_top_level_skill_injection_injects_apple_plugin_orchestrator() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "done"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let codex_home = Arc::new(TempDir::new()?);
    write_apple_orchestrator_plugin(codex_home.as_ref());

    let mut builder = test_codex()
        .with_home(codex_home)
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config(|config| {
            config
                .features
                .enable(Feature::DesktopDeterministicTopLevelSkillInjection)
                .expect("test config should allow feature update");
        });
    let test = builder.build(&server).await?;

    test.codex
        .set_app_server_client_info(Some("desktop-client".to_string()), Some("test".to_string()))
        .await?;

    test.codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "Create a new iOS SwiftUI app named SampleApp.".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;

    wait_for_event(test.codex.as_ref(), |event| {
        matches!(event, codex_protocol::protocol::EventMsg::TurnComplete(_))
    })
    .await;

    let request = mock.single_request();
    let user_texts = request.message_input_texts("user");
    assert!(
        user_texts.iter().any(|text| {
            text.contains("<skill>\n<name>apple-appdev-workflow:apple-app-orchestrator</name>")
        }),
        "expected deterministic structured injection for the Apple orchestrator, got {user_texts:?}"
    );

    Ok(())
}
