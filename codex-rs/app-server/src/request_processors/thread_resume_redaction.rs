use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::McpToolCallResult;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::Turn;
use codex_app_server_protocol::UserInput;
use serde_json::Value as JsonValue;

// Temporary bandaid for remote clients: thread/resume can include large MCP and
// image-generation payloads. Keep this response-only so persisted rollout
// history, model resume history, and other APIs stay unchanged.
const REDACTED_PAYLOAD: &str = "[redacted]";
const REDACTED_IMAGE: &str = "[redacted image]";
const MAX_REMOTE_RESUME_TURNS: usize = 200;
const MAX_REMOTE_TEXT_CHARS: usize = 20_000;
const CHATGPT_REMOTE_CLIENT_NAMES: &[&str] =
    &["codex_chatgpt_android_remote", "codex_chatgpt_ios_remote"];

pub(super) fn should_redact_thread_resume_payloads(client_name: Option<&str>) -> bool {
    client_name.is_some_and(|client_name| CHATGPT_REMOTE_CLIENT_NAMES.contains(&client_name))
}

pub(super) fn redact_thread_resume_payloads(turns: &mut Vec<Turn>) {
    cap_remote_resume_turns(turns);

    for turn in turns {
        turn.items.retain_mut(|item| match item {
            ThreadItem::UserMessage { content, .. } => {
                redact_user_inputs(content);
                true
            }
            ThreadItem::HookPrompt { fragments, .. } => {
                for fragment in fragments {
                    truncate_for_remote(&mut fragment.text);
                }
                true
            }
            ThreadItem::AgentMessage { text, .. } | ThreadItem::Plan { text, .. } => {
                truncate_for_remote(text);
                true
            }
            ThreadItem::Reasoning {
                summary, content, ..
            } => {
                truncate_string_vec(summary);
                truncate_string_vec(content);
                true
            }
            ThreadItem::CommandExecution {
                command,
                aggregated_output,
                ..
            } => {
                truncate_for_remote(command);
                if let Some(output) = aggregated_output {
                    truncate_for_remote(output);
                }
                true
            }
            ThreadItem::FileChange { .. } => false,
            ThreadItem::McpToolCall {
                arguments,
                result,
                error,
                ..
            } => {
                *arguments = JsonValue::String(REDACTED_PAYLOAD.to_string());
                if result.is_some() {
                    *result = Some(Box::new(redacted_mcp_tool_call_result()));
                }
                if let Some(error) = error {
                    error.message = REDACTED_PAYLOAD.to_string();
                }
                true
            }
            ThreadItem::DynamicToolCall {
                arguments,
                content_items,
                ..
            } => {
                *arguments = JsonValue::String(REDACTED_PAYLOAD.to_string());
                if let Some(content_items) = content_items {
                    redact_dynamic_tool_call_content(content_items);
                }
                true
            }
            ThreadItem::CollabAgentToolCall { prompt, .. } => {
                if let Some(prompt) = prompt {
                    truncate_for_remote(prompt);
                }
                true
            }
            ThreadItem::EnteredReviewMode { review, .. }
            | ThreadItem::ExitedReviewMode { review, .. } => {
                truncate_for_remote(review);
                true
            }
            ThreadItem::ImageView { .. } => false,
            ThreadItem::ImageGeneration { .. } => false,
            _ => true,
        });
    }

    turns.retain(|turn| !turn.items.is_empty());
}

fn redacted_mcp_tool_call_result() -> McpToolCallResult {
    McpToolCallResult {
        content: vec![serde_json::json!({
            "type": "text",
            "text": REDACTED_PAYLOAD,
        })],
        structured_content: None,
        meta: None,
    }
}

fn cap_remote_resume_turns(turns: &mut Vec<Turn>) {
    let excess_turns = turns.len().saturating_sub(MAX_REMOTE_RESUME_TURNS);
    if excess_turns > 0 {
        turns.drain(0..excess_turns);
    }
}

fn redact_user_inputs(content: &mut [UserInput]) {
    for input in content {
        match input {
            UserInput::Text {
                text,
                text_elements,
            } => {
                if truncate_for_remote(text) {
                    text_elements.clear();
                }
            }
            UserInput::Image { .. } | UserInput::LocalImage { .. } => {
                *input = UserInput::Text {
                    text: REDACTED_IMAGE.to_string(),
                    text_elements: Vec::new(),
                };
            }
            UserInput::Skill { .. } | UserInput::Mention { .. } => {}
        }
    }
}

fn redact_dynamic_tool_call_content(content_items: &mut [DynamicToolCallOutputContentItem]) {
    for content_item in content_items {
        match content_item {
            DynamicToolCallOutputContentItem::InputText { text } => {
                truncate_for_remote(text);
            }
            DynamicToolCallOutputContentItem::InputImage { .. } => {
                *content_item = DynamicToolCallOutputContentItem::InputText {
                    text: REDACTED_IMAGE.to_string(),
                };
            }
        }
    }
}

fn truncate_string_vec(values: &mut [String]) {
    for value in values {
        truncate_for_remote(value);
    }
}

fn truncate_for_remote(value: &mut String) -> bool {
    if value.chars().count() <= MAX_REMOTE_TEXT_CHARS {
        return false;
    }

    let cutoff = value
        .char_indices()
        .nth(MAX_REMOTE_TEXT_CHARS)
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    value.truncate(cutoff);
    value.push_str("\n[truncated for mobile remote resume]");
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::CommandExecutionSource;
    use codex_app_server_protocol::CommandExecutionStatus;
    use codex_app_server_protocol::DynamicToolCallOutputContentItem;
    use codex_app_server_protocol::DynamicToolCallStatus;
    use codex_app_server_protocol::FileUpdateChange;
    use codex_app_server_protocol::McpToolCallError;
    use codex_app_server_protocol::McpToolCallStatus;
    use codex_app_server_protocol::PatchApplyStatus;
    use codex_app_server_protocol::PatchChangeKind;
    use codex_app_server_protocol::SessionSource;
    use codex_app_server_protocol::Thread;
    use codex_app_server_protocol::ThreadStatus;
    use codex_app_server_protocol::TurnItemsView;
    use codex_app_server_protocol::TurnStatus;
    use codex_utils_absolute_path::test_support::PathBufExt;
    use codex_utils_absolute_path::test_support::test_path_buf;
    use pretty_assertions::assert_eq;

    #[test]
    fn redacts_mcp_success_result_and_removes_image_generation() {
        let mut thread = test_thread(vec![
            ThreadItem::AgentMessage {
                id: "agent-1".to_string(),
                text: "kept".to_string(),
                phase: None,
                memory_citation: None,
            },
            ThreadItem::McpToolCall {
                id: "mcp-1".to_string(),
                server: "docs".to_string(),
                tool: "lookup".to_string(),
                status: McpToolCallStatus::Completed,
                arguments: serde_json::json!({"secret":"argument"}),
                mcp_app_resource_uri: Some("ui://widget/lookup.html".to_string()),
                plugin_id: Some("sample@test".to_string()),
                result: Some(Box::new(McpToolCallResult {
                    content: vec![serde_json::json!({
                        "type": "text",
                        "text": "secret result"
                    })],
                    structured_content: Some(serde_json::json!({"secret":"structured"})),
                    meta: Some(serde_json::json!({"secret":"meta"})),
                })),
                error: None,
                duration_ms: Some(8),
            },
            ThreadItem::ImageGeneration {
                id: "ig-1".to_string(),
                status: "completed".to_string(),
                revised_prompt: Some("revised".to_string()),
                result: "base64-result".to_string(),
                saved_path: Some(test_path_buf("/tmp/ig-1.png").abs()),
            },
        ]);

        redact_thread_resume_payloads(&mut thread.turns);

        assert_eq!(thread.turns[0].items.len(), 2);
        assert_eq!(
            thread.turns[0].items[0],
            ThreadItem::AgentMessage {
                id: "agent-1".to_string(),
                text: "kept".to_string(),
                phase: None,
                memory_citation: None,
            }
        );
        assert_eq!(
            thread.turns[0].items[1],
            ThreadItem::McpToolCall {
                id: "mcp-1".to_string(),
                server: "docs".to_string(),
                tool: "lookup".to_string(),
                status: McpToolCallStatus::Completed,
                arguments: JsonValue::String(REDACTED_PAYLOAD.to_string()),
                mcp_app_resource_uri: Some("ui://widget/lookup.html".to_string()),
                plugin_id: Some("sample@test".to_string()),
                result: Some(Box::new(redacted_mcp_tool_call_result())),
                error: None,
                duration_ms: Some(8),
            }
        );
    }

    #[test]
    fn redacts_large_remote_resume_only_payloads() {
        let mut long_text = "a".repeat(MAX_REMOTE_TEXT_CHARS + 1);
        long_text.push('é');
        let mut thread = test_thread(vec![
            ThreadItem::UserMessage {
                id: "user-1".to_string(),
                content: vec![
                    UserInput::Text {
                        text: long_text.clone(),
                        text_elements: vec![codex_app_server_protocol::TextElement::new(
                            codex_app_server_protocol::ByteRange { start: 0, end: 1 },
                            Some("chip".to_string()),
                        )],
                    },
                    UserInput::Image {
                        detail: None,
                        url: "data:image/png;base64,secret".to_string(),
                    },
                    UserInput::LocalImage {
                        detail: None,
                        path: std::path::PathBuf::from("/tmp/secret.png"),
                    },
                ],
            },
            ThreadItem::CommandExecution {
                id: "cmd-1".to_string(),
                command: "echo kept".to_string(),
                cwd: test_path_buf("/tmp").abs(),
                process_id: None,
                source: CommandExecutionSource::Agent,
                status: CommandExecutionStatus::Completed,
                command_actions: Vec::new(),
                aggregated_output: Some(long_text.clone()),
                exit_code: Some(0),
                duration_ms: Some(1),
            },
            ThreadItem::FileChange {
                id: "patch-1".to_string(),
                changes: vec![FileUpdateChange {
                    path: "large.patch".to_string(),
                    kind: PatchChangeKind::Add,
                    diff: long_text.clone(),
                }],
                status: PatchApplyStatus::Completed,
            },
            ThreadItem::DynamicToolCall {
                id: "tool-1".to_string(),
                namespace: Some("test".to_string()),
                tool: "lookup".to_string(),
                arguments: serde_json::json!({"secret":"argument"}),
                status: DynamicToolCallStatus::Completed,
                content_items: Some(vec![
                    DynamicToolCallOutputContentItem::InputText {
                        text: long_text.clone(),
                    },
                    DynamicToolCallOutputContentItem::InputImage {
                        image_url: "data:image/png;base64,secret".to_string(),
                    },
                ]),
                success: Some(true),
                duration_ms: Some(1),
            },
            ThreadItem::ImageView {
                id: "image-view-1".to_string(),
                path: test_path_buf("/tmp/view.png").abs(),
            },
        ]);

        redact_thread_resume_payloads(&mut thread.turns);

        let turn = &thread.turns[0];
        assert!(
            !turn
                .items
                .iter()
                .any(|item| matches!(item, ThreadItem::FileChange { .. })),
            "file-change diffs should be omitted from mobile resume"
        );
        assert!(
            !turn
                .items
                .iter()
                .any(|item| matches!(item, ThreadItem::ImageView { .. })),
            "local image views should be omitted from mobile resume"
        );

        let user_item = turn
            .items
            .iter()
            .find(|item| matches!(item, ThreadItem::UserMessage { .. }))
            .expect("user message remains");
        let ThreadItem::UserMessage { content, .. } = user_item else {
            unreachable!("matched user message");
        };
        assert!(matches!(
            &content[0],
            UserInput::Text {
                text,
                text_elements
            } if text.ends_with("[truncated for mobile remote resume]")
                && text_elements.is_empty()
        ));
        assert_eq!(
            content[1],
            UserInput::Text {
                text: REDACTED_IMAGE.to_string(),
                text_elements: Vec::new(),
            }
        );
        assert_eq!(
            content[2],
            UserInput::Text {
                text: REDACTED_IMAGE.to_string(),
                text_elements: Vec::new(),
            }
        );

        let command_item = turn
            .items
            .iter()
            .find(|item| matches!(item, ThreadItem::CommandExecution { .. }))
            .expect("command item remains");
        let ThreadItem::CommandExecution {
            aggregated_output, ..
        } = command_item
        else {
            unreachable!("matched command item");
        };
        assert!(
            aggregated_output
                .as_ref()
                .expect("output")
                .ends_with("[truncated for mobile remote resume]")
        );

        let dynamic_item = turn
            .items
            .iter()
            .find(|item| matches!(item, ThreadItem::DynamicToolCall { .. }))
            .expect("dynamic tool item remains");
        let ThreadItem::DynamicToolCall {
            arguments,
            content_items,
            ..
        } = dynamic_item
        else {
            unreachable!("matched dynamic tool item");
        };
        assert_eq!(arguments, &JsonValue::String(REDACTED_PAYLOAD.to_string()));
        assert!(matches!(
            &content_items.as_ref().expect("content items")[0],
            DynamicToolCallOutputContentItem::InputText { text }
                if text.ends_with("[truncated for mobile remote resume]")
        ));
        assert_eq!(
            content_items.as_ref().expect("content items")[1],
            DynamicToolCallOutputContentItem::InputText {
                text: REDACTED_IMAGE.to_string(),
            }
        );
    }

    #[test]
    fn caps_remote_resume_turn_count() {
        let mut thread = test_thread(Vec::new());
        thread.turns = (0..(MAX_REMOTE_RESUME_TURNS + 3))
            .map(|index| Turn {
                id: format!("turn-{index}"),
                items: vec![ThreadItem::AgentMessage {
                    id: format!("agent-{index}"),
                    text: "kept".to_string(),
                    phase: None,
                    memory_citation: None,
                }],
                items_view: TurnItemsView::Full,
                status: TurnStatus::Completed,
                error: None,
                started_at: None,
                completed_at: None,
                duration_ms: None,
            })
            .collect();

        redact_thread_resume_payloads(&mut thread.turns);

        assert_eq!(thread.turns.len(), MAX_REMOTE_RESUME_TURNS);
        assert_eq!(thread.turns[0].id, "turn-3");
    }

    #[test]
    fn redacts_mcp_error_message() {
        let mut thread = test_thread(vec![ThreadItem::McpToolCall {
            id: "mcp-1".to_string(),
            server: "docs".to_string(),
            tool: "lookup".to_string(),
            status: McpToolCallStatus::Failed,
            arguments: serde_json::json!({"secret":"argument"}),
            mcp_app_resource_uri: None,
            plugin_id: None,
            result: None,
            error: Some(McpToolCallError {
                message: "secret error".to_string(),
            }),
            duration_ms: Some(8),
        }]);

        redact_thread_resume_payloads(&mut thread.turns);

        assert_eq!(
            thread.turns[0].items[0],
            ThreadItem::McpToolCall {
                id: "mcp-1".to_string(),
                server: "docs".to_string(),
                tool: "lookup".to_string(),
                status: McpToolCallStatus::Failed,
                arguments: JsonValue::String(REDACTED_PAYLOAD.to_string()),
                mcp_app_resource_uri: None,
                plugin_id: None,
                result: None,
                error: Some(McpToolCallError {
                    message: REDACTED_PAYLOAD.to_string(),
                }),
                duration_ms: Some(8),
            }
        );
    }

    fn test_thread(items: Vec<ThreadItem>) -> Thread {
        Thread {
            id: "thread-1".to_string(),
            session_id: "session-1".to_string(),
            forked_from_id: None,
            parent_thread_id: None,
            preview: "preview".to_string(),
            ephemeral: false,
            model_provider: "mock_provider".to_string(),
            created_at: 0,
            updated_at: 0,
            status: ThreadStatus::Idle,
            path: None,
            cwd: test_path_buf("/tmp").abs(),
            cli_version: "0.0.0".to_string(),
            source: SessionSource::Cli,
            thread_source: None,
            agent_nickname: None,
            agent_role: None,
            git_info: None,
            name: None,
            turns: vec![Turn {
                id: "turn-1".to_string(),
                items,
                items_view: TurnItemsView::Full,
                status: TurnStatus::Completed,
                error: None,
                started_at: None,
                completed_at: None,
                duration_ms: None,
            }],
        }
    }
}
