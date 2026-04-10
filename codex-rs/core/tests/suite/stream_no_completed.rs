//! Verifies that the agent retries when the SSE stream terminates before
//! delivering a `response.completed` event.

use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use codex_utils_cargo_bin::find_resource;
use core_test_support::load_sse_fixture;
use core_test_support::responses;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_message_item_added;
use core_test_support::responses::ev_output_text_delta;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::start_websocket_server;
use core_test_support::skip_if_no_network;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::start_streaming_sse_server;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use serde_json::Value;
use serde_json::json;
use tokio::sync::oneshot;

fn sse_incomplete() -> String {
    let fixture = find_resource!("tests/fixtures/incomplete_sse.json")
        .unwrap_or_else(|err| panic!("failed to resolve incomplete_sse fixture: {err}"));
    load_sse_fixture(fixture)
}

fn ev_message_item_done(id: &str, text: &str) -> Value {
    json!({
        "type": "response.output_item.done",
        "item": {
            "type": "message",
            "role": "assistant",
            "id": id,
            "content": [{"type": "output_text", "text": text}]
        }
    })
}

fn ev_commentary_message_item_done(id: &str, text: &str) -> Value {
    json!({
        "type": "response.output_item.done",
        "item": {
            "type": "message",
            "role": "assistant",
            "id": id,
            "content": [{"type": "output_text", "text": text}],
            "phase": "commentary"
        }
    })
}

fn ev_reasoning_item_done(id: &str, text: &str) -> Value {
    json!({
        "type": "response.output_item.done",
        "item": {
            "type": "reasoning",
            "id": id,
            "summary": [{
                "type": "summary_text",
                "text": text
            }]
        }
    })
}

fn chunk(event: Value) -> StreamingSseChunk {
    StreamingSseChunk {
        gate: None,
        body: responses::sse(vec![event]),
    }
}

fn gated_chunk(gate: oneshot::Receiver<()>, events: Vec<Value>) -> StreamingSseChunk {
    StreamingSseChunk {
        gate: Some(gate),
        body: responses::sse(events),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retries_on_early_close() {
    skip_if_no_network!();

    let incomplete_sse = sse_incomplete();
    let completed_sse = responses::sse_completed("resp_ok");

    let (server, _) = start_streaming_sse_server(vec![
        vec![StreamingSseChunk {
            gate: None,
            body: incomplete_sse,
        }],
        vec![StreamingSseChunk {
            gate: None,
            body: completed_sse,
        }],
    ])
    .await;

    // Configure retry behavior explicitly to avoid mutating process-wide
    // environment variables.

    let model_provider = ModelProviderInfo {
        name: "openai".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        // Environment variable that should exist in the test environment.
        // ModelClient will return an error if the environment variable for the
        // provider is not set.
        env_key: Some("PATH".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        // exercise retry path: first attempt yields incomplete stream, so allow 1 retry
        request_max_retries: Some(0),
        stream_max_retries: Some(1),
        stream_idle_timeout_ms: Some(2000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let TestCodex { codex, .. } = test_codex()
        .with_config(move |config| {
            config.model_provider = model_provider;
        })
        .build_with_streaming_server(&server)
        .await
        .unwrap();

    codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await
        .unwrap();

    // Wait until TurnComplete (should succeed after retry).
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let requests = server.requests().await;
    assert_eq!(
        requests.len(),
        2,
        "expected retry after incomplete SSE stream"
    );

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retries_when_stream_idles_after_commentary_before_completed() {
    skip_if_no_network!();

    let (gate_completion_tx, gate_completion_rx) = oneshot::channel::<()>();

    let first_chunks = vec![
        chunk(ev_response_created("resp-idle")),
        chunk(ev_message_item_added("msg-1", "")),
        chunk(ev_output_text_delta("working")),
        chunk(ev_commentary_message_item_done("msg-1", "working")),
        gated_chunk(gate_completion_rx, vec![ev_completed("resp-idle")]),
    ];

    let second_chunks = vec![
        chunk(ev_response_created("resp-ok")),
        chunk(ev_message_item_added("msg-2", "")),
        chunk(ev_output_text_delta("final after retry")),
        chunk(ev_message_item_done("msg-2", "final after retry")),
        chunk(ev_completed("resp-ok")),
    ];

    let (server, _) = start_streaming_sse_server(vec![first_chunks, second_chunks]).await;

    let model_provider = ModelProviderInfo {
        name: "openai".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(1),
        stream_idle_timeout_ms: Some(50),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let TestCodex { codex, .. } = test_codex()
        .with_config(move |config| {
            config.model_provider = model_provider;
        })
        .build_with_streaming_server(&server)
        .await
        .unwrap();

    codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await
        .unwrap();

    tokio::time::timeout(tokio::time::Duration::from_secs(2), async {
        loop {
            if server.requests().await.len() == 2 {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected retry after commentary stream idled before response.completed");

    drop(gate_completion_tx);

    wait_for_event(
        &codex,
        |event| matches!(event, EventMsg::AgentMessage(message) if message.message == "final after retry"),
    )
    .await;
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let requests = server.requests().await;
    assert_eq!(
        requests.len(),
        2,
        "expected retry after commentary stream idled before response.completed"
    );

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retries_rebuild_prompt_from_history_after_commentary_only_interruption() {
    skip_if_no_network!();

    let first_attempt = vec![
        chunk(ev_response_created("resp-commentary")),
        chunk(ev_message_item_added("msg-1", "")),
        chunk(ev_output_text_delta("Routing: orchestrator-led")),
        chunk(ev_commentary_message_item_done(
            "msg-1",
            "Routing: orchestrator-led",
        )),
    ];

    let second_attempt = vec![
        chunk(ev_response_created("resp-final")),
        chunk(ev_message_item_added("msg-2", "")),
        chunk(ev_output_text_delta("final after retry")),
        chunk(ev_message_item_done("msg-2", "final after retry")),
        chunk(ev_completed("resp-final")),
    ];

    let (server, _) = start_streaming_sse_server(vec![first_attempt, second_attempt]).await;

    let model_provider = ModelProviderInfo {
        name: "openai".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(1),
        stream_idle_timeout_ms: Some(50),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let TestCodex { codex, .. } = test_codex()
        .with_config(move |config| {
            config.model_provider = model_provider;
        })
        .build_with_streaming_server(&server)
        .await
        .unwrap();

    codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await
        .unwrap();

    wait_for_event(
        &codex,
        |event| matches!(event, EventMsg::AgentMessage(message) if message.message == "final after retry"),
    )
    .await;
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let requests = server.requests().await;
    assert_eq!(
        requests.len(),
        2,
        "expected one retry after commentary interruption"
    );

    let retry_body: Value =
        serde_json::from_slice(&requests[1]).expect("retry request body should be valid JSON");
    let retry_input = retry_body["input"].as_array().expect("retry input array");
    let commentary_item = retry_input
        .iter()
        .find(|item| {
            item.get("role").and_then(|role| role.as_str()) == Some("assistant")
                && item.get("phase").and_then(|phase| phase.as_str()) == Some("commentary")
                && item
                    .get("content")
                    .and_then(|content| content.as_array())
                    .and_then(|content| content.first())
                    .and_then(|entry| entry.get("text"))
                    .and_then(|text| text.as_str())
                    == Some("Routing: orchestrator-led")
        })
        .expect(
            "retry request should include persisted commentary item from the interrupted attempt",
        );
    assert_eq!(
        commentary_item
            .get("phase")
            .and_then(|phase| phase.as_str()),
        Some("commentary")
    );

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retries_rebuild_prompt_from_history_after_commentary_delta_only_interruption() {
    skip_if_no_network!();

    let first_attempt = vec![
        chunk(ev_response_created("resp-partial")),
        chunk(ev_message_item_added("msg-1", "")),
        chunk(ev_output_text_delta("Routing: orchestrator-led")),
    ];

    let second_attempt = vec![
        chunk(ev_response_created("resp-final")),
        chunk(ev_message_item_added("msg-2", "")),
        chunk(ev_output_text_delta("final after retry")),
        chunk(ev_message_item_done("msg-2", "final after retry")),
        chunk(ev_completed("resp-final")),
    ];

    let (server, _) = start_streaming_sse_server(vec![first_attempt, second_attempt]).await;

    let model_provider = ModelProviderInfo {
        name: "openai".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(1),
        stream_idle_timeout_ms: Some(50),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let TestCodex { codex, .. } = test_codex()
        .with_config(move |config| {
            config.model_provider = model_provider;
        })
        .build_with_streaming_server(&server)
        .await
        .unwrap();

    codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await
        .unwrap();

    wait_for_event(
        &codex,
        |event| matches!(event, EventMsg::AgentMessage(message) if message.message == "final after retry"),
    )
    .await;
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let requests = server.requests().await;
    assert_eq!(
        requests.len(),
        2,
        "expected one retry after commentary delta interruption"
    );

    let retry_body: Value =
        serde_json::from_slice(&requests[1]).expect("retry request body should be valid JSON");
    let retry_input = retry_body["input"].as_array().expect("retry input array");
    retry_input
        .iter()
        .find(|item| {
            item.get("role").and_then(|role| role.as_str()) == Some("assistant")
                && item
                    .get("content")
                    .and_then(|content| content.as_array())
                    .and_then(|content| content.first())
                    .and_then(|entry| entry.get("text"))
                    .and_then(|text| text.as_str())
                    == Some("Routing: orchestrator-led")
        })
        .expect(
            "retry request should include partial streamed commentary from the interrupted attempt",
        );

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fails_fast_after_repeated_commentary_only_retries_without_progress() {
    skip_if_no_network!();

    let commentary_only_attempt = |response_id: &str, message_id: &str| {
        vec![
            chunk(ev_response_created(response_id)),
            chunk(ev_message_item_added(message_id, "")),
            chunk(ev_output_text_delta("Routing: orchestrator-led")),
            chunk(ev_commentary_message_item_done(
                message_id,
                "Routing: orchestrator-led",
            )),
        ]
    };

    let (server, _) = start_streaming_sse_server(vec![
        commentary_only_attempt("resp-1", "msg-1"),
        commentary_only_attempt("resp-2", "msg-2"),
        commentary_only_attempt("resp-3", "msg-3"),
        commentary_only_attempt("resp-4", "msg-4"),
    ])
    .await;

    let model_provider = ModelProviderInfo {
        name: "openai".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(5),
        stream_idle_timeout_ms: Some(50),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let TestCodex { codex, .. } = test_codex()
        .with_config(move |config| {
            config.model_provider = model_provider;
        })
        .build_with_streaming_server(&server)
        .await
        .unwrap();

    codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await
        .unwrap();

    let mut failure_message = None;
    wait_for_event(&codex, |event| match event {
        EventMsg::Error(error)
            if error
                .message
                .contains("without making substantive progress toward a completed answer") =>
        {
            failure_message = Some(error.message.clone());
            true
        }
        _ => false,
    })
    .await;

    let failure_message = failure_message.expect("expected commentary-only retry loop error");
    assert!(
        failure_message.contains("retried 3 times"),
        "expected retry count in error message: {failure_message}"
    );

    let requests = server.requests().await;
    assert_eq!(
        requests.len(),
        3,
        "expected commentary-only retry loop to stop before exhausting the full retry budget"
    );

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fails_fast_after_repeated_commentary_plus_reasoning_retries_without_progress() {
    skip_if_no_network!();

    let commentary_plus_reasoning_attempt =
        |response_id: &str, message_id: &str, reasoning_id: &str| {
            vec![
                chunk(ev_response_created(response_id)),
                chunk(ev_message_item_added(message_id, "")),
                chunk(ev_output_text_delta("Routing: orchestrator-led")),
                chunk(ev_commentary_message_item_done(
                    message_id,
                    "Routing: orchestrator-led",
                )),
                chunk(ev_reasoning_item_done(
                    reasoning_id,
                    "Still thinking through the supplied diff evidence.",
                )),
            ]
        };

    let (server, _) = start_streaming_sse_server(vec![
        commentary_plus_reasoning_attempt("resp-1", "msg-1", "rs-1"),
        commentary_plus_reasoning_attempt("resp-2", "msg-2", "rs-2"),
        commentary_plus_reasoning_attempt("resp-3", "msg-3", "rs-3"),
        commentary_plus_reasoning_attempt("resp-4", "msg-4", "rs-4"),
    ])
    .await;

    let model_provider = ModelProviderInfo {
        name: "openai".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(5),
        stream_idle_timeout_ms: Some(50),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let TestCodex { codex, .. } = test_codex()
        .with_config(move |config| {
            config.model_provider = model_provider;
        })
        .build_with_streaming_server(&server)
        .await
        .unwrap();

    codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await
        .unwrap();

    let mut failure_message = None;
    wait_for_event(&codex, |event| match event {
        EventMsg::Error(error)
            if error
                .message
                .contains("without making substantive progress toward a completed answer") =>
        {
            failure_message = Some(error.message.clone());
            true
        }
        _ => false,
    })
    .await;

    let failure_message =
        failure_message.expect("expected commentary-plus-reasoning retry loop error");
    assert!(
        failure_message.contains("retried 3 times"),
        "expected retry count in error message: {failure_message}"
    );

    let requests = server.requests().await;
    assert_eq!(
        requests.len(),
        3,
        "expected commentary-plus-reasoning retry loop to stop before exhausting the full retry budget"
    );

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resets_no_progress_loop_when_retry_branch_improves_to_visible_commentary() {
    skip_if_no_network!();

    let empty_stall_attempt = |response_id: &str, message_id: &str| {
        vec![
            chunk(ev_response_created(response_id)),
            chunk(ev_message_item_added(message_id, "")),
        ]
    };

    let visible_commentary_attempt = |response_id: &str, message_id: &str| {
        vec![
            chunk(ev_response_created(response_id)),
            chunk(ev_message_item_added(message_id, "")),
            chunk(ev_output_text_delta("Routing: orchestrator-led")),
            chunk(ev_commentary_message_item_done(
                message_id,
                "Routing: orchestrator-led",
            )),
        ]
    };

    let final_attempt = vec![
        chunk(ev_response_created("resp-final")),
        chunk(ev_message_item_added("msg-final", "")),
        chunk(ev_output_text_delta("final after improved retry")),
        chunk(ev_message_item_done("msg-final", "final after improved retry")),
        chunk(ev_completed("resp-final")),
    ];

    let (server, _) = start_streaming_sse_server(vec![
        empty_stall_attempt("resp-empty-1", "msg-empty-1"),
        empty_stall_attempt("resp-empty-2", "msg-empty-2"),
        visible_commentary_attempt("resp-commentary", "msg-commentary"),
        final_attempt,
    ])
    .await;

    let model_provider = ModelProviderInfo {
        name: "openai".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(5),
        stream_idle_timeout_ms: Some(50),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let TestCodex { codex, .. } = test_codex()
        .with_config(move |config| {
            config.model_provider = model_provider;
        })
        .build_with_streaming_server(&server)
        .await
        .unwrap();

    codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await
        .unwrap();

    wait_for_event(
        &codex,
        |event| matches!(event, EventMsg::AgentMessage(message) if message.message == "final after improved retry"),
    )
    .await;
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let requests = server.requests().await;
    assert_eq!(
        requests.len(),
        4,
        "expected the no-progress loop counter to reset once durable visible commentary arrived"
    );

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_first_reconnect_reuses_partial_response_id_after_partial_progress() {
    skip_if_no_network!();

    let first_connection = vec![
        vec![ev_response_created("resp-warm"), ev_completed("resp-warm")],
        vec![
            ev_response_created("resp-1"),
            ev_message_item_added("msg-1", ""),
            ev_output_text_delta("Routing: orchestrator-led"),
            ev_commentary_message_item_done("msg-1", "Routing: orchestrator-led"),
        ],
    ];
    let degraded_retry = |response_id: &str, message_id: &str| {
        vec![
            ev_response_created(response_id),
            ev_message_item_added(message_id, ""),
        ]
    };
    let server = start_websocket_server(vec![
        first_connection,
        vec![degraded_retry("resp-2", "msg-2")],
        vec![degraded_retry("resp-3", "msg-3")],
    ])
    .await;

    let mut builder = test_codex().with_config(|config| {
        config.model_provider.request_max_retries = Some(0);
        config.model_provider.stream_max_retries = Some(5);
        config.model_provider.stream_idle_timeout_ms = Some(50);
    });
    let TestCodex { codex, .. } = builder.build_with_websocket_server(&server).await.unwrap();

    codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await
        .unwrap();

    let mut failure_message = None;
    wait_for_event(&codex, |event| match event {
        EventMsg::Error(error)
            if error
                .message
                .contains("without making substantive progress toward a completed answer") =>
        {
            failure_message = Some(error.message.clone());
            true
        }
        _ => false,
    })
    .await;

    let failure_message = failure_message.expect("expected retry loop error");
    assert!(
        failure_message.contains("Routing: orchestrator-led"),
        "expected last visible assistant message in error: {failure_message}"
    );

    let connections = server.connections();
    assert_eq!(connections.len(), 3, "expected initial socket plus two reconnects");
    assert_eq!(
        connections[0].len(),
        2,
        "expected warmup and first real request on the initial websocket"
    );
    assert_eq!(
        connections[1].len(),
        1,
        "expected one degraded retry request on the second websocket"
    );
    assert_eq!(
        connections[2].len(),
        1,
        "expected one degraded retry request on the third websocket"
    );

    let initial_main_request = connections[0][1].body_json();
    assert!(
        initial_main_request["previous_response_id"].as_str().is_some(),
        "expected first real websocket request after warmup to be incremental: {initial_main_request}"
    );

    let retry_one = connections[1][0].body_json();
    let retry_two = connections[2][0].body_json();
    assert!(
        retry_one["previous_response_id"].as_str() == Some("resp-1"),
        "expected first reconnect to reuse the partial response id: {retry_one}"
    );
    assert_eq!(retry_one["input"], serde_json::json!([]));
    assert_eq!(retry_two["type"].as_str(), Some("response.create"));

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_partial_resume_rejection_retries_fresh() {
    skip_if_no_network!();

    let first_connection = vec![
        vec![ev_response_created("resp-warm"), ev_completed("resp-warm")],
        vec![
            ev_response_created("resp-1"),
            ev_message_item_added("msg-1", ""),
            ev_output_text_delta("Routing: orchestrator-led"),
            ev_commentary_message_item_done("msg-1", "Routing: orchestrator-led"),
        ],
    ];
    let previous_response_not_found = vec![json!({
        "type": "error",
        "status": 400,
        "error": {
            "type": "invalid_request_error",
            "code": "previous_response_not_found",
            "message": "Previous response with id 'resp-1' not found."
        }
    })];
    let fresh_retry = vec![
        ev_response_created("resp-ok"),
        ev_message_item_added("msg-ok", ""),
        ev_output_text_delta("final after fresh retry"),
        ev_message_item_done("msg-ok", "final after fresh retry"),
        ev_completed("resp-ok"),
    ];
    let server = start_websocket_server(vec![
        first_connection,
        vec![previous_response_not_found],
        vec![fresh_retry],
    ])
    .await;

    let mut builder = test_codex().with_config(|config| {
        config.model_provider.request_max_retries = Some(0);
        config.model_provider.stream_max_retries = Some(5);
        config.model_provider.stream_idle_timeout_ms = Some(50);
    });
    let TestCodex { codex, .. } = builder.build_with_websocket_server(&server).await.unwrap();

    codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await
        .unwrap();

    wait_for_event(
        &codex,
        |event| matches!(event, EventMsg::AgentMessage(message) if message.message == "final after fresh retry"),
    )
    .await;

    let connections = server.connections();
    assert_eq!(connections.len(), 3);
    let partial_resume = connections[1][0].body_json();
    let fresh_fallback = connections[2][0].body_json();

    assert_eq!(partial_resume["previous_response_id"].as_str(), Some("resp-1"));
    assert_eq!(partial_resume["input"], serde_json::json!([]));
    assert!(fresh_fallback["previous_response_id"].is_null());

    server.shutdown().await;
}
