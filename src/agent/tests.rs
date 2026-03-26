//! Comprehensive agent-loop test suite.
//!
//! Tests exercise the full `Agent.turn()` cycle with mock providers and tools,
//! covering every edge case an agentic tool loop must handle:
//!
//!   1. Simple text response (no tools)
//!   2. Single tool call → final response
//!   3. Multi-step tool chain (tool A → tool B → response)
//!   4. Max-iteration bailout
//!   5. Unknown tool name recovery
//!   6. Tool execution failure recovery
//!   7. Provider error propagation
//!   8. History trimming during long conversations
//!   9. Memory auto-save round-trip
//!  10. Turn integration with XML-formatted response
//!  11. Empty / whitespace-only LLM responses
//!  12. Mixed text + tool call responses
//!  13. Multi-tool batch in a single response
//!  14. System prompt generation & tool instructions
//!  15. Conversation history fidelity
//!  16. Builder validation (missing required fields)
//!  17. Multi-turn conversation maintains context
//!  18. Clear history
//!  19. run_single delegates to turn
//!  20. Idempotent system prompt insertion

use crate::agent::agent::Agent;
use crate::config::{AgentConfig, MemoryConfig};
use crate::memory::{self, Memory};
use crate::observability::{NoopObserver, Observer};
use crate::providers::{ChatMessage, ChatRequest, ChatResponse, Provider, ToolCall};
use crate::tools::{Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

// ═══════════════════════════════════════════════════════════════════════════
// Test Helpers — Mock Provider, Mock Tool, Mock Memory
// ═══════════════════════════════════════════════════════════════════════════

/// A mock LLM provider that returns pre-scripted responses in order.
/// When the queue is exhausted it returns a simple "done" text response.
struct ScriptedProvider {
    responses: Mutex<Vec<ChatResponse>>,
    /// Records every request for assertion.
    requests: Mutex<Vec<Vec<ChatMessage>>>,
}

impl ScriptedProvider {
    fn new(responses: Vec<ChatResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> Result<String> {
        Ok("fallback".into())
    }

    async fn chat(
        &self,
        request: ChatRequest<'_>,
        _model: &str,
        _temperature: f64,
    ) -> Result<ChatResponse> {
        self.requests
            .lock()
            .unwrap()
            .push(request.messages.to_vec());

        let mut guard = self.responses.lock().unwrap();
        if guard.is_empty() {
            return Ok(ChatResponse {
                text: Some("done".into()),
                tool_calls: vec![],
                usage: None,
                reasoning_content: None,
            });
        }
        Ok(guard.remove(0))
    }
}

/// A mock provider that always returns an error.
struct FailingProvider;

#[async_trait]
impl Provider for FailingProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> Result<String> {
        anyhow::bail!("provider error")
    }

    async fn chat(
        &self,
        _request: ChatRequest<'_>,
        _model: &str,
        _temperature: f64,
    ) -> Result<ChatResponse> {
        anyhow::bail!("provider error")
    }
}

/// A simple echo tool that returns its arguments as output.
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes the input"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {"type": "string"}
            }
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let msg = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("(empty)")
            .to_string();
        Ok(ToolResult {
            success: true,
            output: msg,
            error: None,
        })
    }
}

/// A tool that always fails execution.
struct FailingTool;

#[async_trait]
impl Tool for FailingTool {
    fn name(&self) -> &str {
        "fail"
    }

    fn description(&self) -> &str {
        "Always fails"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(&self, _args: serde_json::Value) -> Result<ToolResult> {
        Ok(ToolResult {
            success: false,
            output: String::new(),
            error: Some("intentional failure".into()),
        })
    }
}

/// A tool that panics (tests error propagation).
struct PanickingTool;

#[async_trait]
impl Tool for PanickingTool {
    fn name(&self) -> &str {
        "panicker"
    }

    fn description(&self) -> &str {
        "Panics on execution"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(&self, _args: serde_json::Value) -> Result<ToolResult> {
        anyhow::bail!("catastrophic tool failure")
    }
}

/// A tool that tracks how many times it was called.
struct CountingTool {
    count: Arc<Mutex<usize>>,
}

impl CountingTool {
    fn new() -> (Self, Arc<Mutex<usize>>) {
        let count = Arc::new(Mutex::new(0));
        (
            Self {
                count: count.clone(),
            },
            count,
        )
    }
}

#[async_trait]
impl Tool for CountingTool {
    fn name(&self) -> &str {
        "counter"
    }

    fn description(&self) -> &str {
        "Counts calls"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(&self, _args: serde_json::Value) -> Result<ToolResult> {
        let mut c = self.count.lock().unwrap();
        *c += 1;
        Ok(ToolResult {
            success: true,
            output: format!("call #{}", *c),
            error: None,
        })
    }
}

fn make_memory() -> Arc<dyn Memory> {
    let cfg = MemoryConfig {
        backend: "none".into(),
        ..MemoryConfig::default()
    };
    Arc::from(memory::create_memory(&cfg, &std::env::temp_dir(), None).unwrap())
}

fn make_sqlite_memory() -> (Arc<dyn Memory>, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = MemoryConfig {
        backend: "sqlite".into(),
        ..MemoryConfig::default()
    };
    let mem = Arc::from(memory::create_memory(&cfg, tmp.path(), None).unwrap());
    (mem, tmp)
}

fn make_observer() -> Arc<dyn Observer> {
    Arc::from(NoopObserver {})
}

fn build_agent_with(
    provider: Box<dyn Provider>,
    tools: Vec<Box<dyn Tool>>,
) -> Agent {
    Agent::builder()
        .provider(provider)
        .tools(tools)
        .memory(make_memory())
        .observer(make_observer())
        .workspace_dir(std::env::temp_dir())
        .build()
        .unwrap()
}

fn build_agent_with_memory(
    provider: Box<dyn Provider>,
    tools: Vec<Box<dyn Tool>>,
    mem: Arc<dyn Memory>,
    auto_save: bool,
) -> Agent {
    Agent::builder()
        .provider(provider)
        .tools(tools)
        .memory(mem)
        .observer(make_observer())
        .workspace_dir(std::env::temp_dir())
        .auto_save(auto_save)
        .build()
        .unwrap()
}

fn build_agent_with_config(
    provider: Box<dyn Provider>,
    tools: Vec<Box<dyn Tool>>,
    config: AgentConfig,
) -> Agent {
    Agent::builder()
        .provider(provider)
        .tools(tools)
        .memory(make_memory())
        .observer(make_observer())
        .workspace_dir(std::env::temp_dir())
        .config(config)
        .build()
        .unwrap()
}

/// Helper: create a ChatResponse with tool calls (native format).
fn tool_response(calls: Vec<ToolCall>) -> ChatResponse {
    ChatResponse {
        text: Some(String::new()),
        tool_calls: calls,
        usage: None,
        reasoning_content: None,
    }
}

/// Helper: create a plain text ChatResponse.
fn text_response(text: &str) -> ChatResponse {
    ChatResponse {
        text: Some(text.into()),
        tool_calls: vec![],
        usage: None,
        reasoning_content: None,
    }
}

/// Helper: create an XML-style tool call response.
fn xml_tool_response(name: &str, args: &str) -> ChatResponse {
    ChatResponse {
        text: Some(format!(
            "<tool_call>\n{{\"name\": \"{name}\", \"arguments\": {args}}}\n</tool_call>"
        )),
        tool_calls: vec![],
        usage: None,
        reasoning_content: None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. Simple text response (no tools)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn turn_returns_text_when_no_tools_called() {
    let provider = Box::new(ScriptedProvider::new(vec![text_response("Hello world")]));
    let mut agent = build_agent_with(provider, vec![Box::new(EchoTool)]);

    let response = agent.turn("hi").await.unwrap();
    assert!(
        !response.is_empty(),
        "Expected non-empty text response from provider"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Single tool call → final response
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn turn_executes_single_tool_then_returns() {
    let provider = Box::new(ScriptedProvider::new(vec![
        tool_response(vec![ToolCall {
            id: "tc1".into(),
            name: "echo".into(),
            arguments: r#"{"message": "hello from tool"}"#.into(),
        }]),
        text_response("I ran the tool"),
    ]));

    let mut agent = build_agent_with(provider, vec![Box::new(EchoTool)]);

    let response = agent.turn("run echo").await.unwrap();
    assert!(
        !response.is_empty(),
        "Expected non-empty response after tool execution"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Multi-step tool chain (tool A → tool B → response)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn turn_handles_multi_step_tool_chain() {
    let (counting_tool, count) = CountingTool::new();

    let provider = Box::new(ScriptedProvider::new(vec![
        tool_response(vec![ToolCall {
            id: "tc1".into(),
            name: "counter".into(),
            arguments: r#"{"step":1}"#.into(),
        }]),
        tool_response(vec![ToolCall {
            id: "tc2".into(),
            name: "counter".into(),
            arguments: r#"{"step":2}"#.into(),
        }]),
        tool_response(vec![ToolCall {
            id: "tc3".into(),
            name: "counter".into(),
            arguments: r#"{"step":3}"#.into(),
        }]),
        text_response("Done after 3 calls"),
    ]));

    let mut agent = build_agent_with(provider, vec![Box::new(counting_tool)]);

    let response = agent.turn("count 3 times").await.unwrap();
    assert!(
        !response.is_empty(),
        "Expected non-empty response after multi-step chain"
    );
    assert_eq!(*count.lock().unwrap(), 3);
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Max-iteration bailout
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn turn_bails_out_at_max_iterations() {
    let max_iters = 3;
    let mut responses = Vec::new();
    for i in 0..max_iters + 5 {
        responses.push(tool_response(vec![ToolCall {
            id: format!("tc{i}"),
            name: "echo".into(),
            arguments: format!(r#"{{"message": "loop-{i}"}}"#),
        }]));
    }

    let provider = Box::new(ScriptedProvider::new(responses));

    let config = AgentConfig {
        max_tool_iterations: max_iters,
        ..AgentConfig::default()
    };

    let mut agent = build_agent_with_config(provider, vec![Box::new(EchoTool)], config);

    let result = agent.turn("infinite loop").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("maximum tool iterations"),
        "Expected max iterations error, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. Unknown tool name recovery
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn turn_handles_unknown_tool_gracefully() {
    let provider = Box::new(ScriptedProvider::new(vec![
        tool_response(vec![ToolCall {
            id: "tc1".into(),
            name: "nonexistent_tool".into(),
            arguments: "{}".into(),
        }]),
        text_response("I couldn't find that tool"),
    ]));

    let mut agent = build_agent_with(provider, vec![Box::new(EchoTool)]);

    let response = agent.turn("use nonexistent").await.unwrap();
    assert!(
        !response.is_empty(),
        "Expected non-empty response after unknown tool recovery"
    );

    let has_unknown_tool_msg = agent
        .history()
        .iter()
        .any(|msg| msg.content.contains("Unknown tool"));
    assert!(
        has_unknown_tool_msg,
        "Expected history entry with 'Unknown tool' message"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. Tool execution failure recovery
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn turn_recovers_from_tool_failure() {
    let provider = Box::new(ScriptedProvider::new(vec![
        tool_response(vec![ToolCall {
            id: "tc1".into(),
            name: "fail".into(),
            arguments: "{}".into(),
        }]),
        text_response("Tool failed but I recovered"),
    ]));

    let mut agent = build_agent_with(provider, vec![Box::new(FailingTool)]);

    let response = agent.turn("try failing tool").await.unwrap();
    assert!(
        !response.is_empty(),
        "Expected non-empty response after tool failure recovery"
    );
}

#[tokio::test]
async fn turn_recovers_from_tool_error() {
    let provider = Box::new(ScriptedProvider::new(vec![
        tool_response(vec![ToolCall {
            id: "tc1".into(),
            name: "panicker".into(),
            arguments: "{}".into(),
        }]),
        text_response("I recovered from the error"),
    ]));

    let mut agent = build_agent_with(provider, vec![Box::new(PanickingTool)]);

    let response = agent.turn("try panicking").await.unwrap();
    assert!(
        !response.is_empty(),
        "Expected non-empty response after tool error recovery"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 7. Provider error propagation
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn turn_propagates_provider_error() {
    let mut agent = build_agent_with(Box::new(FailingProvider), vec![]);

    let result = agent.turn("hello").await;
    assert!(result.is_err(), "Expected provider error to propagate");
}

// ═══════════════════════════════════════════════════════════════════════════
// 8. History trimming during long conversations
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn history_trims_after_max_messages() {
    let max_history = 6;
    let mut responses = vec![];
    for _ in 0..max_history + 5 {
        responses.push(text_response("ok"));
    }

    let provider = Box::new(ScriptedProvider::new(responses));
    let config = AgentConfig {
        max_history_messages: max_history,
        ..AgentConfig::default()
    };

    let mut agent = build_agent_with_config(provider, vec![], config);

    for i in 0..max_history + 5 {
        let _ = agent.turn(&format!("msg {i}")).await.unwrap();
    }

    assert!(
        agent.history().len() <= max_history + 1,
        "History length {} exceeds max {} + 1 (system)",
        agent.history().len(),
        max_history,
    );

    let first = &agent.history()[0];
    assert_eq!(first.role, "system", "First history entry should be system prompt");
}

// ═══════════════════════════════════════════════════════════════════════════
// 9. Memory auto-save round-trip
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn auto_save_stores_only_user_messages_in_memory() {
    let (mem, _tmp) = make_sqlite_memory();
    let provider = Box::new(ScriptedProvider::new(vec![text_response(
        "I remember everything",
    )]));

    let mut agent = build_agent_with_memory(
        provider,
        vec![],
        mem.clone(),
        true, // auto_save enabled
    );

    let _ = agent.turn("Remember this fact").await.unwrap();

    let count = mem.count().await.unwrap();
    assert_eq!(
        count, 1,
        "Expected exactly 1 user memory entry, got {count}"
    );

    let stored = mem.get("user_msg").await.unwrap();
    assert!(stored.is_some(), "Expected user_msg key to be present");
    assert_eq!(
        stored.unwrap().content,
        "Remember this fact",
        "Stored memory should match the original user message"
    );

    let assistant = mem.get("assistant_resp").await.unwrap();
    assert!(
        assistant.is_none(),
        "assistant_resp should not be auto-saved anymore"
    );
}

#[tokio::test]
async fn auto_save_disabled_does_not_store() {
    let (mem, _tmp) = make_sqlite_memory();
    let provider = Box::new(ScriptedProvider::new(vec![text_response("hello")]));

    let mut agent = build_agent_with_memory(
        provider,
        vec![],
        mem.clone(),
        false, // auto_save disabled
    );

    let _ = agent.turn("test message").await.unwrap();

    let count = mem.count().await.unwrap();
    assert_eq!(count, 0, "Expected 0 memory entries with auto_save off");
}

// ═══════════════════════════════════════════════════════════════════════════
// 10. Turn integration with XML-formatted response
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn xml_formatted_response_completes_turn() {
    let provider = Box::new(ScriptedProvider::new(vec![
        xml_tool_response("echo", r#"{"message": "xml-test"}"#),
        text_response("XML tool completed"),
    ]));

    let mut agent = build_agent_with(provider, vec![Box::new(EchoTool)]);

    let response = agent.turn("test xml").await.unwrap();
    assert!(
        !response.is_empty(),
        "Expected non-empty response from turn"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 11. Empty / whitespace-only LLM responses
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn turn_handles_empty_text_response() {
    let provider = Box::new(ScriptedProvider::new(vec![ChatResponse {
        text: Some(String::new()),
        tool_calls: vec![],
        usage: None,
        reasoning_content: None,
    }]));

    let mut agent = build_agent_with(provider, vec![]);

    let response = agent.turn("hi").await.unwrap();
    assert!(response.is_empty());
}

#[tokio::test]
async fn turn_handles_none_text_response() {
    let provider = Box::new(ScriptedProvider::new(vec![ChatResponse {
        text: None,
        tool_calls: vec![],
        usage: None,
        reasoning_content: None,
    }]));

    let mut agent = build_agent_with(provider, vec![]);

    let response = agent.turn("hi").await.unwrap();
    assert!(response.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════
// 12. Mixed text + tool call responses
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn turn_preserves_text_alongside_tool_calls() {
    let provider = Box::new(ScriptedProvider::new(vec![
        ChatResponse {
            text: Some("Let me check...".into()),
            tool_calls: vec![ToolCall {
                id: "tc1".into(),
                name: "echo".into(),
                arguments: r#"{"message": "hi"}"#.into(),
            }],
            usage: None,
            reasoning_content: None,
        },
        text_response("Here are the results"),
    ]));

    let mut agent = build_agent_with(provider, vec![Box::new(EchoTool)]);

    let response = agent.turn("check something").await.unwrap();
    assert!(
        !response.is_empty(),
        "Expected non-empty final response after mixed text+tool"
    );

    let has_intermediate = agent
        .history()
        .iter()
        .any(|msg| msg.role == "assistant" && msg.content.contains("Let me check"));
    assert!(has_intermediate, "Intermediate text should be in history");
}

// ═══════════════════════════════════════════════════════════════════════════
// 13. Multi-tool batch in a single response
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn turn_handles_multiple_tools_in_one_response() {
    let (counting_tool, count) = CountingTool::new();

    let provider = Box::new(ScriptedProvider::new(vec![
        tool_response(vec![
            ToolCall {
                id: "tc1".into(),
                name: "counter".into(),
                arguments: r#"{"batch":1}"#.into(),
            },
            ToolCall {
                id: "tc2".into(),
                name: "counter".into(),
                arguments: r#"{"batch":2}"#.into(),
            },
            ToolCall {
                id: "tc3".into(),
                name: "counter".into(),
                arguments: r#"{"batch":3}"#.into(),
            },
        ]),
        text_response("All 3 done"),
    ]));

    let mut agent = build_agent_with(provider, vec![Box::new(counting_tool)]);

    let response = agent.turn("batch").await.unwrap();
    assert!(
        !response.is_empty(),
        "Expected non-empty response after multi-tool batch"
    );
    assert_eq!(
        *count.lock().unwrap(),
        3,
        "All 3 tools should have been called"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 14. System prompt generation & tool instructions
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn system_prompt_injected_on_first_turn() {
    let provider = Box::new(ScriptedProvider::new(vec![text_response("ok")]));
    let mut agent = build_agent_with(provider, vec![Box::new(EchoTool)]);

    assert!(agent.history().is_empty(), "History should start empty");

    let _ = agent.turn("hi").await.unwrap();

    let first = &agent.history()[0];
    assert_eq!(
        first.role, "system",
        "First history entry should be system prompt"
    );
}

#[tokio::test]
async fn system_prompt_not_duplicated_on_second_turn() {
    let provider = Box::new(ScriptedProvider::new(vec![
        text_response("first"),
        text_response("second"),
    ]));
    let mut agent = build_agent_with(provider, vec![Box::new(EchoTool)]);

    let _ = agent.turn("hi").await.unwrap();
    let _ = agent.turn("hello again").await.unwrap();

    let system_count = agent
        .history()
        .iter()
        .filter(|msg| msg.role == "system")
        .count();
    assert_eq!(system_count, 1, "System prompt should appear exactly once");
}

// ═══════════════════════════════════════════════════════════════════════════
// 15. Conversation history fidelity
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn history_contains_all_expected_entries_after_tool_loop() {
    let provider = Box::new(ScriptedProvider::new(vec![
        tool_response(vec![ToolCall {
            id: "tc1".into(),
            name: "echo".into(),
            arguments: r#"{"message": "tool-out"}"#.into(),
        }]),
        text_response("final answer"),
    ]));

    let mut agent = build_agent_with(provider, vec![Box::new(EchoTool)]);

    let _ = agent.turn("test").await.unwrap();

    // Expected history: system, user, assistant (tool call), tool result(s), assistant (final)
    let history = agent.history();
    assert!(
        history.len() >= 5,
        "Expected at least 5 history entries, got {}",
        history.len()
    );

    assert_eq!(history[0].role, "system");
    assert_eq!(history[1].role, "user");
    assert_eq!(history[2].role, "assistant");
    // history[3] is tool result — could be role "tool" or "user" depending on format
    assert!(
        history[3].role == "tool" || history[3].role == "user",
        "Expected tool result entry at index 3, got role='{}'",
        history[3].role
    );
    assert_eq!(history[4].role, "assistant");
    assert!(
        history[4].content.contains("final answer"),
        "Last assistant message should contain 'final answer'"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 16. Builder validation
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn builder_fails_without_provider() {
    let result = Agent::builder()
        .tools(vec![])
        .memory(make_memory())
        .observer(make_observer())
        .workspace_dir(std::path::PathBuf::from("/tmp"))
        .build();

    assert!(result.is_err(), "Building without provider should fail");
}

// ═══════════════════════════════════════════════════════════════════════════
// 17. Multi-turn conversation maintains context
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn multi_turn_maintains_growing_history() {
    let provider = Box::new(ScriptedProvider::new(vec![
        text_response("response 1"),
        text_response("response 2"),
        text_response("response 3"),
    ]));

    let mut agent = build_agent_with(provider, vec![]);

    let r1 = agent.turn("msg 1").await.unwrap();
    let len_after_1 = agent.history().len();

    let r2 = agent.turn("msg 2").await.unwrap();
    let len_after_2 = agent.history().len();

    let r3 = agent.turn("msg 3").await.unwrap();
    let len_after_3 = agent.history().len();

    assert_eq!(r1, "response 1");
    assert_eq!(r2, "response 2");
    assert_eq!(r3, "response 3");

    assert!(
        len_after_2 > len_after_1,
        "History should grow after turn 2"
    );
    assert!(
        len_after_3 > len_after_2,
        "History should grow after turn 3"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 18. Clear history
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn clear_history_resets_conversation() {
    let provider = Box::new(ScriptedProvider::new(vec![
        text_response("first"),
        text_response("second"),
    ]));

    let mut agent = build_agent_with(provider, vec![]);

    let _ = agent.turn("hi").await.unwrap();
    assert!(!agent.history().is_empty());

    agent.clear_history();
    assert!(agent.history().is_empty());

    let _ = agent.turn("hello again").await.unwrap();
    assert_eq!(
        agent.history()[0].role, "system",
        "First entry after clear should be system prompt"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 19. run_single delegates to turn
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn run_single_delegates_to_turn() {
    let provider = Box::new(ScriptedProvider::new(vec![text_response("via run_single")]));
    let mut agent = build_agent_with(provider, vec![]);

    let response = agent.run_single("test").await.unwrap();
    assert!(
        !response.is_empty(),
        "Expected non-empty response from run_single"
    );
}
