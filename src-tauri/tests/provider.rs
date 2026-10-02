//! Integration tests for the OpenAI-compatible `Provider`
//! (native-agent-harness Task 4): `complete()` against a mock HTTP server
//! (wiremock) serving canned SSE streams / canned status codes.

use archimedes_lib::agent::harness::{
    ChatMessage, ChatRole, FinishReason, MessageContent, ModelOptions, ModelRequest,
    OpenAiCompatibleProvider, Provider, ProviderError, ProviderEvent,
};
use futures_util::StreamExt;
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A provider pointed at the mock server.
fn provider(base: &MockServer) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider {
        base_url: base.uri(),
        api_key: "sk-test".to_string(),
    }
}

/// A minimal request (one user message, no tools).
fn request() -> ModelRequest {
    ModelRequest {
        model: "test-model".to_string(),
        messages: vec![ChatMessage {
            role: ChatRole::User,
            content: MessageContent::Text("hello".to_string()),
            tool_call_id: None,
            tool_calls: None,
        }],
        tools: vec![],
        options: ModelOptions {
            temperature: Some(0.5),
            max_tokens: Some(128),
            reasoning_effort: None,
            stream: true,
        },
        session_id: None,
    }
}

/// Mount a canned `text/event-stream` response on the mock server.
async fn mount_sse(server: &MockServer, body: &str) {
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(body.to_string(), "text/event-stream"),
        )
        .mount(server)
        .await;
}

/// Collect the whole `complete()` stream.
async fn collect(p: &OpenAiCompatibleProvider, req: &ModelRequest) -> Vec<ProviderEvent> {
    let mut stream = p
        .complete(req)
        .await
        .expect("complete() succeeds against the mock server");
    let mut events = Vec::new();
    loop {
        // A timeout guard: a regression that stalls the stream fails the
        // test (with a partial event list) instead of hanging the suite.
        let r = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next()).await;
        match r {
            Ok(Some(event)) => events.push(event),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    events
}

// ── (a) content deltas → TextDelta in order, then Done("stop") ──────────

#[tokio::test]
async fn content_deltas_stream_in_order_then_done_stop() {
    let server = MockServer::start().await;
    mount_sse(
        &server,
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
         data: [DONE]\n\n",
    )
    .await;

    let events = collect(&provider(&server), &request()).await;
    assert_eq!(
        events,
        vec![
            ProviderEvent::TextDelta("Hel".to_string()),
            ProviderEvent::TextDelta("lo".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ],
        "content deltas in order, then Done(Stop)"
    );
}

// ── (b) tool_calls split across chunks → accumulated ToolCall, Done ─────

#[tokio::test]
async fn tool_call_fragments_accumulate_then_done_tool_calls() {
    let server = MockServer::start().await;
    // The name and arguments are split across three chunks (as the real
    // API streams them), and the `id` arrives only in the first.
    mount_sse(
        &server,
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"re\"}}]}}]}\n\n\
         data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"ad\",\"arguments\":\"{\"}}]}}]}\n\n\
         data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"path\\\": \\\"/x\\\"}\"}}]}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
         data: [DONE]\n\n",
    )
    .await;

    let events = collect(&provider(&server), &request()).await;

    let tool_call_idx = events
        .iter()
        .position(|e| matches!(e, ProviderEvent::ToolCall(_)))
        .expect("an accumulated ToolCall event is emitted");
    let done_idx = events
        .iter()
        .position(|e| matches!(e, ProviderEvent::Done(_)))
        .expect("a Done event is emitted");
    assert!(tool_call_idx < done_idx, "the ToolCall precedes the Done");

    match &events[tool_call_idx] {
        ProviderEvent::ToolCall(tc) => {
            assert_eq!(tc.id, "call_1", "the id from the first fragment");
            assert_eq!(tc.name, "read", "name fragments accumulated");
            assert_eq!(
                tc.arguments,
                serde_json::json!({ "path": "/x" }),
                "argument fragments accumulated into valid JSON"
            );
        }
        other => panic!("expected a ToolCall, got {other:?}"),
    }
    assert_eq!(
        events[done_idx],
        ProviderEvent::Done(FinishReason::ToolCalls)
    );

    // The per-chunk deltas were also emitted (index-keyed).
    let deltas: Vec<&ProviderEvent> = events
        .iter()
        .take_while(|e| !matches!(e, ProviderEvent::ToolCall(_)))
        .filter(|e| matches!(e, ProviderEvent::ToolCallDelta(_)))
        .collect();
    assert_eq!(deltas.len(), 3, "one ToolCallDelta per chunk");
}

// ── (c) a trailing usage chunk → Usage event ─────────────────────────────

#[tokio::test]
async fn usage_chunk_emits_usage_event() {
    let server = MockServer::start().await;
    mount_sse(
        &server,
        "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
         data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n\
         data: [DONE]\n\n",
    )
    .await;

    let events = collect(&provider(&server), &request()).await;
    let usage = events
        .iter()
        .find_map(|e| match e {
            ProviderEvent::Usage(u) => Some(*u),
            _ => None,
        })
        .expect("a Usage event is emitted");
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 5);
}

// ── (d) 429 → Retryable ──────────────────────────────────────────────────

#[tokio::test]
async fn http_429_is_retryable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .mount(&server)
        .await;

    let err = match provider(&server).complete(&request()).await {
        Err(e) => e,
        Ok(_) => panic!("a 429 response is an error"),
    };
    assert!(
        matches!(err, ProviderError::Retryable(_)),
        "a 429 is retryable, got {err:?}"
    );
}

// ── (e) 401 → Auth ───────────────────────────────────────────────────────

#[tokio::test]
async fn http_401_is_auth() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
        .mount(&server)
        .await;

    let err = match provider(&server).complete(&request()).await {
        Err(e) => e,
        Ok(_) => panic!("a 401 response is an error"),
    };
    assert!(
        matches!(err, ProviderError::Auth(_)),
        "a 401 is an auth error, got {err:?}"
    );
}

// ── (f) session_id present → x-litellm-session-id & x-request-id headers ──

#[tokio::test]
async fn openai_provider_sends_session_id_headers_when_present() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("x-litellm-session-id", "sess-xyz-123"))
        .and(header("x-request-id", "sess-xyz-123"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
                 data: [DONE]\n\n",
            "text/event-stream",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let mut req = request();
    req.session_id = Some("sess-xyz-123".to_string());

    let events = collect(&provider(&server), &req).await;
    assert_eq!(
        events,
        vec![
            ProviderEvent::TextDelta("ok".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ],
    );
}

// ── (g) session_id absent → omit headers ─────────────────────────────────

#[tokio::test]
async fn openai_provider_omits_session_id_headers_when_absent() {
    let server = MockServer::start().await;
    mount_sse(
        &server,
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
         data: [DONE]\n\n",
    )
    .await;

    let req = request();
    let events = collect(&provider(&server), &req).await;
    assert_eq!(
        events,
        vec![
            ProviderEvent::TextDelta("ok".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ],
    );

    let requests = server.received_requests().await.expect("recorded requests");
    assert_eq!(requests.len(), 1);
    let sent_headers = &requests[0].headers;
    assert!(
        !sent_headers.contains_key(wiremock::http::HeaderName::from_static(
            "x-litellm-session-id"
        )),
        "x-litellm-session-id header should not be present"
    );
    assert!(
        !sent_headers.contains_key(wiremock::http::HeaderName::from_static("x-request-id")),
        "x-request-id header should not be present"
    );
}
