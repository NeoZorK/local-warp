//! In-process local Agent Mode backend.
//!
//! This module bypasses Warp's remote MAA endpoint and talks directly to a loopback-only
//! OpenAI Chat Completions endpoint (MLX OptiQ). It emits the same MAA response events the
//! existing local action executors consume, so file, shell, and approved computer-use actions
//! continue through Warp's existing UI/permission path.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context as _, anyhow, bail};
use base64::Engine as _;
use futures::stream;
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;
use warp_multi_agent_api as api;

use ai::agent::action_result::{RequestComputerUseResult, ScreenshotSource, UseComputerResult};
use crate::ai::agent::{AIAgentActionResult, AIAgentActionResultType, AIAgentInput};
use crate::server::server_api::AIApiError;

use super::{Event, RequestParams, ResponseStream};

const DEFAULT_OPTIQ_BASE_URL: &str = "http://127.0.0.1:9163/v1";
const DEFAULT_MODEL_ID: &str = "mlx-community/Qwen3.5-9B-OptiQ-4bit";
const MAX_CONTEXT_CHARS: usize = 48_000;
const MAX_TOOL_RESULT_CHARS: usize = 8_000;
const MAX_HISTORY_MESSAGES: usize = 60;

/// A native Warp response stream backed entirely by the local OptiQ server.
pub(crate) async fn generate_local_multi_agent_output(
    params: RequestParams,
    _cancellation_rx: futures::channel::oneshot::Receiver<()>,
) -> ResponseStream {
    let result = run_local_turn(params).await;
    let events = match result {
        Ok(events) => events.into_iter().map(Ok).collect::<Vec<Event>>(),
        Err(error) => {
            let error = Arc::new(AIApiError::Other(error));
            vec![Err(error)]
        }
    };
    Box::pin(stream::iter(events))
}

async fn run_local_turn(params: RequestParams) -> anyhow::Result<Vec<api::ResponseEvent>> {
    anyhow::ensure!(
        warp_core::telemetry::is_local_offline_mode(),
        "The native local Agent backend must be launched with WARP_LOCAL_OFFLINE=1"
    );

    let base_url = local_base_url()?;
    let client = http_client::Client::new_external();
    let api_key = std::env::var("OPTIQ_API_KEY").ok().filter(|key| !key.trim().is_empty());
    let model = model_id(&client, &base_url, api_key.as_deref()).await?;

    let request_id = Uuid::new_v4().to_string();
    let conversation_id = params
        .conversation_token
        .as_ref()
        .map(|token| token.as_str().to_owned())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let run_id = params
        .ambient_agent_task_id
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| conversation_id.clone());

    let root_task = params
        .tasks
        .iter()
        .find(|task| task.dependencies.as_ref().is_none_or(|deps| deps.parent_task_id.is_empty()))
        .or_else(|| params.tasks.first());
    let task_id = root_task
        .map(|task| task.id.clone())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let create_root_task = !params.tasks.iter().any(|task| task.id == task_id);

    let mut messages = vec![json!({
        "role": "system",
        "content": local_system_prompt()
    })];

    append_history(&params.tasks, &mut messages);
    append_current_inputs(&params.input, &params.tasks, &mut messages);

    if messages.len() <= 1 {
        messages.push(json!({
            "role": "user",
            "content": "Continue the current task using the conversation history. If a tool is needed, request it through the available local tools."
        }));
    }
    bound_messages(&mut messages);

    let computer_approved = has_computer_use_approval(&params);
    let tools = local_tools(params.computer_use_enabled, computer_approved);
    let body = json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "stream": false,
        "temperature": 0.1
    });

    let response = local_post_json(
        &client,
        &format!("{base_url}/chat/completions"),
        &body,
        api_key.as_deref(),
    )
    .await
    .context("Local Qwen Chat Completions request failed")?;

    let message = response
        .pointer("/choices/0/message")
        .ok_or_else(|| anyhow!("OptiQ response is missing choices[0].message: {}", truncate(&response.to_string(), 2_000)))?;

    let response_text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut actions: Vec<api::ClientAction> = Vec::new();
    if create_root_task {
        actions.push(api::ClientAction {
            action: Some(api::client_action::Action::CreateTask(
                api::client_action::CreateTask {
                    task: Some(api::Task {
                        id: task_id.clone(),
                        ..Default::default()
                    }),
                },
            )),
        });
    }

    let mut task_messages: Vec<api::Message> = Vec::new();

    for input in &params.input {
        if let AIAgentInput::UserQuery { query, .. } = input {
            if !params.tasks.iter().any(|task| task.messages.iter().any(|message| {
                matches!(message.message.as_ref(), Some(api::message::Message::UserQuery(existing)) if existing.query == *query)
            })) {
                task_messages.push(new_message(
                    &task_id,
                    &request_id,
                    api::message::Message::UserQuery(api::message::UserQuery {
                        query: query.clone(),
                        ..Default::default()
                    }),
                ));
            }
        }
    }

    if !response_text.trim().is_empty() {
        task_messages.push(new_message(
            &task_id,
            &request_id,
            api::message::Message::AgentOutput(api::message::AgentOutput {
                text: response_text,
            }),
        ));
    }

    let mut rejected_tools = Vec::new();
    for call in tool_calls {
        match make_tool_call(&call, params.computer_use_enabled, computer_approved) {
            Ok(Some(tool_call)) => {
                task_messages.push(new_message(
                    &task_id,
                    &request_id,
                    api::message::Message::ToolCall(tool_call),
                ));
            }
            Ok(None) => {}
            Err(error) => rejected_tools.push(error),
        }
    }

    if !rejected_tools.is_empty() {
        let text = format!(
            "The local model requested unsupported or malformed tool calls; no action was run:\n{}",
            rejected_tools.join("\n")
        );
        task_messages.push(new_message(
            &task_id,
            &request_id,
            api::message::Message::AgentOutput(api::message::AgentOutput { text }),
        ));
    }

    if task_messages.is_empty() {
        task_messages.push(new_message(
            &task_id,
            &request_id,
            api::message::Message::AgentOutput(api::message::AgentOutput {
                text: "Qwen returned neither text nor a supported local tool call. Try rephrasing the request.".to_owned(),
            }),
        ));
    }

    actions.push(api::ClientAction {
        action: Some(api::client_action::Action::AddMessagesToTask(
            api::client_action::AddMessagesToTask {
                task_id: task_id.clone(),
                messages: task_messages,
            },
        )),
    });

    Ok(vec![
        api::ResponseEvent {
            r#type: Some(api::response_event::Type::Init(api::response_event::StreamInit {
                conversation_id,
                request_id,
                run_id,
            })),
        },
        api::ResponseEvent {
            r#type: Some(api::response_event::Type::ClientActions(
                api::response_event::ClientActions { actions },
            )),
        },
        api::ResponseEvent {
            r#type: Some(api::response_event::Type::Finished(api::response_event::StreamFinished {
                reason: Some(api::response_event::stream_finished::Reason::Done(
                    api::response_event::stream_finished::Done {},
                )),
                ..Default::default()
            })),
        },
    ])
}

fn local_system_prompt() -> &'static str {
    "You are the native local coding agent inside this Warp fork. All inference is performed by the local Qwen model through MLX OptiQ. Use the provided local tools for file reading, searching, edits, shell commands, and computer interaction. Never claim an action succeeded unless a tool result confirms it. Do not use web search, remote agents, external MCP servers, cloud APIs, or network access. Prefer small, reviewable diffs. Use edit_file for existing files and write_file only for new files. Request computer-use permission before using the computer; use_computer is only available after Warp reports that the user approved it. All shell commands and file edits still pass through Warp's own local execution and permission UI."
}

fn local_base_url() -> anyhow::Result<String> {
    let value = std::env::var("OPTIQ_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_OPTIQ_BASE_URL.to_owned())
        .trim_end_matches('/')
        .to_owned();
    let parsed = Url::parse(&value).context("Invalid OPTIQ_BASE_URL")?;
    anyhow::ensure!(parsed.scheme() == "http", "OptiQ must use plain HTTP on loopback; TLS is not needed locally");
    let host = parsed.host_str().unwrap_or_default();
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1";
    anyhow::ensure!(is_loopback, "Refusing non-loopback OptiQ URL: {host}");
    anyhow::ensure!(
        parsed.query().is_none() && parsed.username().is_empty() && parsed.password().is_none(),
        "OPTIQ_BASE_URL must not contain credentials or query parameters"
    );
    anyhow::ensure!(
        parsed.path().is_empty() || parsed.path() == "/" || parsed.path().trim_end_matches('/') == "/v1",
        "OPTIQ_BASE_URL must be the server root or end in /v1"
    );
    let origin = format!("{}://{}{}", parsed.scheme(), host, parsed.port().map(|p| format!(":{p}")).unwrap_or_default());
    let path = parsed.path().trim_end_matches('/');
    let path = if path.is_empty() { "/v1" } else { path };
    Ok(format!("{origin}{path}").trim_end_matches('/').to_owned())
}

async fn model_id(
    client: &http_client::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> anyhow::Result<String> {
    if let Ok(model) = std::env::var("OPTIQ_MODEL_ID") {
        if !model.trim().is_empty() {
            return Ok(model);
        }
    }
    match local_get_json(client, &format!("{base_url}/models"), api_key).await {
        Ok(models) => {
            let id = models
                .get("data")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .map(str::to_owned);
            Ok(id.unwrap_or_else(|| DEFAULT_MODEL_ID.to_owned()))
        }
        Err(_) => Ok(DEFAULT_MODEL_ID.to_owned()),
    }
}

async fn local_get_json(
    client: &http_client::Client,
    url: &str,
    api_key: Option<&str>,
) -> anyhow::Result<Value> {
    let mut request = client.get(url).timeout(std::time::Duration::from_secs(8));
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.context("Could not connect to local OptiQ")?;
    let status = response.status();
    let body = response.text().await.context("Could not read local OptiQ response")?;
    anyhow::ensure!(status.is_success(), "Local OptiQ returned HTTP {status}: {}", truncate(&body, 2_000));
    serde_json::from_str(&body).context("Local OptiQ returned invalid JSON")
}

async fn local_post_json(
    client: &http_client::Client,
    url: &str,
    body: &Value,
    api_key: Option<&str>,
) -> anyhow::Result<Value> {
    let mut request = client
        .post(url)
        .json(body)
        .timeout(std::time::Duration::from_secs(600));
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.context("Could not connect to local OptiQ")?;
    let status = response.status();
    let text = response.text().await.context("Could not read local OptiQ response")?;
    anyhow::ensure!(status.is_success(), "Local OptiQ returned HTTP {status}: {}", truncate(&text, 4_000));
    serde_json::from_str(&text).context("Local OptiQ returned invalid JSON")
}

fn new_message(task_id: &str, request_id: &str, message: api::message::Message) -> api::Message {
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        request_id: request_id.to_owned(),
        timestamp: Some(prost_types::Timestamp::from(SystemTime::now())),
        message: Some(message),
        ..Default::default()
    }
}

fn append_history(tasks: &[api::Task], messages: &mut Vec<Value>) {
    let mut emitted = 0usize;
    for task in tasks {
        for item in &task.messages {
            if emitted >= MAX_HISTORY_MESSAGES {
                return;
            }
            match item.message.as_ref() {
                Some(api::message::Message::UserQuery(query)) => {
                    messages.push(json!({"role":"user","content": truncate(&query.query, MAX_TOOL_RESULT_CHARS)}));
                    emitted += 1;
                }
                Some(api::message::Message::AgentOutput(output)) if !output.text.trim().is_empty() => {
                    messages.push(json!({"role":"assistant","content": truncate(&output.text, MAX_TOOL_RESULT_CHARS)}));
                    emitted += 1;
                }
                Some(api::message::Message::ToolCall(call)) => {
                    if let Some((name, arguments)) = local_tool_from_proto(call) {
                        let arguments_text = serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_owned());
                        messages.push(json!({
                            "role":"assistant",
                            "tool_calls":[{
                                "id": call.tool_call_id,
                                "type":"function",
                                "function":{"name":name,"arguments":arguments_text}
                            }]
                        }));
                        emitted += 1;
                    }
                }
                Some(api::message::Message::ToolCallResult(result)) => {
                    let text = local_tool_result_text(result);
                    messages.push(json!({
                        "role":"tool",
                        "tool_call_id": result.tool_call_id,
                        "content": truncate(&text, MAX_TOOL_RESULT_CHARS)
                    }));
                    emitted += 1;
                }
                Some(api::message::Message::AgentReasoning(reasoning)) if !reasoning.reasoning.is_empty() => {
                    messages.push(json!({"role":"assistant","content": truncate(&reasoning.reasoning, 2_000)}));
                    emitted += 1;
                }
                _ => {}
            }
        }
    }
}

fn append_current_inputs(
    inputs: &[AIAgentInput],
    tasks: &[api::Task],
    messages: &mut Vec<Value>,
) {
    for input in inputs {
        match input {
            AIAgentInput::UserQuery { query, context, .. } => {
                let already_in_history = tasks.iter().any(|task| task.messages.iter().any(|message| {
                    matches!(message.message.as_ref(), Some(api::message::Message::UserQuery(existing)) if existing.query == *query)
                }));
                if !already_in_history {
                    messages.push(json!({"role":"user","content": truncate(query, MAX_CONTEXT_CHARS)}));
                }
                let context_text = truncate(&format!("{context:#?}"), 16_000);
                if !context_text.trim().is_empty() {
                    messages.push(json!({"role":"user","content":format!("Local context from Warp:\\n{context_text}")}));
                }
            }
            AIAgentInput::ActionResult { result, .. } => {
                if !history_has_tool_result(tasks, &result.id.to_string()) {
                    append_action_result(result, messages);
                }
            }
            AIAgentInput::AutoCodeDiffQuery { query, context } => {
                let context_text = truncate(&format!("{context:#?}"), 12_000);
                messages.push(json!({"role":"user","content": truncate(&format!("{query}\n\nLocal context:\n{context_text}"), MAX_CONTEXT_CHARS)}));
            }
            AIAgentInput::ResumeConversation { context } => {
                messages.push(json!({"role":"user","content":truncate(&format!("Continue the task. Current local context:\n{context:#?}"), 16_000)}));
            }
            _ => {}
        }
    }
}

fn history_has_tool_result(tasks: &[api::Task], action_id: &str) -> bool {
    tasks.iter().any(|task| task.messages.iter().any(|message| {
        matches!(message.message.as_ref(), Some(api::message::Message::ToolCallResult(result)) if result.tool_call_id == action_id)
    }))
}

fn append_action_result(result: &AIAgentActionResult, messages: &mut Vec<Value>) {
    messages.push(json!({
        "role": "tool",
        "tool_call_id": result.id.to_string(),
        "content": truncate(&result.to_string(), MAX_TOOL_RESULT_CHARS)
    }));

    let screenshot = match &result.result {
        AIAgentActionResultType::RequestComputerUse(RequestComputerUseResult::Approved { screenshot, .. }) => {
            Some((screenshot.mime_type.to_string(), screenshot.data.as_slice()))
        }
        AIAgentActionResultType::UseComputer(UseComputerResult::Success {
            screenshot: Some(ScreenshotSource::Inline(screenshot)),
            ..
        }) => Some((screenshot.mime_type.to_string(), screenshot.data.as_slice())),
        _ => None,
    };
    if let Some((mime_type, bytes)) = screenshot {
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        messages.push(json!({
            "role": "user",
            "content": [
                {"type":"text","text":"Latest screenshot from the local computer-use executor. Interpret it and continue only with approved computer-use actions."},
                {"type":"image_url","image_url":{"url":format!("data:{mime_type};base64,{encoded}")}}
            ]
        }));
    }
}

fn has_computer_use_approval(params: &RequestParams) -> bool {
    if params.input.iter().any(|input| matches!(
        input,
        AIAgentInput::ActionResult {
            result: AIAgentActionResult {
                result: AIAgentActionResultType::RequestComputerUse(RequestComputerUseResult::Approved { .. }),
                ..
            },
            ..
        }
    )) {
        return true;
    }
    params.tasks.iter().any(|task| task.messages.iter().any(|message| {
        let Some(api::message::Message::ToolCallResult(result)) = message.message.as_ref() else {
            return false;
        };
        let Some(api::message::tool_call_result::Result::RequestComputerUseResult(request)) = result.result.as_ref() else {
            return false;
        };
        matches!(
            request.result.as_ref(),
            Some(api::message::tool_call_result::request_computer_use_result::Result::Approved(_))
        )
    }))
}

fn local_tools(computer_use_enabled: bool, computer_approved: bool) -> Vec<Value> {
    let mut tools = vec![
        function_tool("run_shell_command", "Run a command in the current local Warp session. The existing Warp command permission UI applies; never assume a command ran until its result returns.", json!({
            "type":"object","properties":{"command":{"type":"string","description":"Shell command to execute"}},
            "required":["command"],"additionalProperties":false
        })),
        function_tool("read_file", "Read a local file using Warp's existing file-reading executor.", json!({
            "type":"object","properties":{"path":{"type":"string","description":"Absolute or workspace-relative path"}},
            "required":["path"],"additionalProperties":false
        })),
        function_tool("write_file", "Create a new local file using Warp's existing file-diff approval UI. Use only for files that do not already exist.", json!({
            "type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},
            "required":["path","content"],"additionalProperties":false
        })),
        function_tool("edit_file", "Modify an existing local file with an exact search/replace through Warp's existing edit review UI.", json!({
            "type":"object","properties":{"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"},"summary":{"type":"string"}},
            "required":["path","old_text","new_text"],"additionalProperties":false
        })),
        function_tool("grep", "Search local files with Warp's grep executor.", json!({
            "type":"object","properties":{"query":{"type":"string"},"path":{"type":"string"}},
            "required":["query"],"additionalProperties":false
        })),
        function_tool("file_glob", "Find local files by glob pattern with Warp's local file glob executor.", json!({
            "type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},
            "required":["pattern"],"additionalProperties":false
        })),
        function_tool("search_codebase", "Search the local codebase/index with Warp's existing codebase search executor. This does not perform web search.", json!({
            "type":"object","properties":{"query":{"type":"string"},"path_filters":{"type":"array","items":{"type":"string"}},"codebase_path":{"type":"string"}},
            "required":["query"],"additionalProperties":false
        }))
    ];

    if computer_use_enabled && !computer_approved {
        tools.push(function_tool("request_computer_use", "Ask Warp to request permission for local computer control. The user must approve before using the screen, mouse, or keyboard.", json!({
            "type":"object","properties":{"task_summary":{"type":"string"}},
            "required":["task_summary"],"additionalProperties":false
        })));
    }
    if computer_use_enabled && computer_approved {
        tools.push(function_tool("use_computer", "Use Warp's existing local computer-use executor after user approval. Actions are applied to the local desktop; a screenshot is returned to this local model.", json!({
            "type":"object","properties":{
                "action_summary":{"type":"string"},
                "actions":{"type":"array","items":{"type":"object","properties":{
                    "type":{"type":"string","enum":["mouse_move","mouse_down","mouse_up","mouse_wheel","wait","type_text","key_down","key_up"]},
                    "x":{"type":"integer"},"y":{"type":"integer"},
                    "button":{"type":"string","enum":["left","right","middle"]},
                    "direction":{"type":"string","enum":["up","down","left","right"]},
                    "pixels":{"type":"integer"},"milliseconds":{"type":"integer"},
                    "text":{"type":"string"},"key":{"type":"string"}
                },"required":["type"],"additionalProperties":false}},
                "screenshot_after":{"type":"boolean"}
            },
            "required":["action_summary","actions"],"additionalProperties":false
        })));
    }
    tools
}

fn function_tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type":"function",
        "function":{"name":name,"description":description,"parameters":parameters}
    })
}

fn make_tool_call(
    call: &Value,
    computer_use_enabled: bool,
    computer_approved: bool,
) -> anyhow::Result<Option<api::message::ToolCall>> {
    let function = call.get("function").ok_or_else(|| anyhow!("tool call has no function object"))?;
    let name = function.get("name").and_then(Value::as_str).ok_or_else(|| anyhow!("tool call has no function name"))?;
    let arguments = parse_tool_arguments(function.get("arguments"))?;
    let id = Uuid::new_v4().to_string();

    use api::message::tool_call::Tool;
    let tool = match name {
        "run_shell_command" => {
            let command = string_arg(&arguments, "command")?;
            Tool::RunShellCommand(api::message::tool_call::RunShellCommand {
                command,
                is_read_only: false,
                is_risky: true,
                ..Default::default()
            })
        }
        "read_file" => {
            let path = string_arg(&arguments, "path")?;
            Tool::ReadFiles(api::message::tool_call::ReadFiles {
                files: vec![api::message::tool_call::read_files::File {
                    name: path,
                    line_ranges: Vec::new(),
                }],
            })
        }
        "write_file" => {
            let path = string_arg(&arguments, "path")?;
            let content = string_arg(&arguments, "content")?;
            Tool::ApplyFileDiffs(api::message::tool_call::ApplyFileDiffs {
                summary: format!("Create {path}"),
                new_files: vec![api::message::tool_call::apply_file_diffs::NewFile {
                    file_path: path,
                    content,
                    allow_overwrite: false,
                }],
                ..Default::default()
            })
        }
        "edit_file" => {
            let path = string_arg(&arguments, "path")?;
            let old_text = string_arg(&arguments, "old_text")?;
            let new_text = string_arg(&arguments, "new_text")?;
            let summary = arguments.get("summary").and_then(Value::as_str).unwrap_or("Edit file").to_owned();
            Tool::ApplyFileDiffs(api::message::tool_call::ApplyFileDiffs {
                summary,
                diffs: vec![api::message::tool_call::apply_file_diffs::FileDiff {
                    file_path: path,
                    search: old_text,
                    replace: new_text,
                }],
                ..Default::default()
            })
        }
        "grep" => {
            let query = string_arg(&arguments, "query")?;
            let path = arguments.get("path").and_then(Value::as_str).unwrap_or(".").to_owned();
            Tool::Grep(api::message::tool_call::Grep {
                queries: vec![query],
                path,
            })
        }
        "file_glob" => {
            let pattern = string_arg(&arguments, "pattern")?;
            let path = arguments.get("path").and_then(Value::as_str).unwrap_or(".").to_owned();
            Tool::FileGlobV2(api::message::tool_call::FileGlobV2 {
                patterns: vec![pattern],
                search_dir: path,
                max_matches: 200,
                ..Default::default()
            })
        }
        "search_codebase" => {
            let query = string_arg(&arguments, "query")?;
            let path_filters = arguments.get("path_filters").and_then(Value::as_array)
                .map(|values| values.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default();
            let codebase_path = arguments.get("codebase_path").and_then(Value::as_str).unwrap_or_default().to_owned();
            Tool::SearchCodebase(api::message::tool_call::SearchCodebase {
                query,
                path_filters,
                codebase_path,
            })
        }
        "request_computer_use" => {
            anyhow::ensure!(computer_use_enabled, "Computer use is disabled in Warp settings");
            anyhow::ensure!(!computer_approved, "Computer use is already approved; use the computer-use tool instead");
            Tool::RequestComputerUse(api::message::tool_call::RequestComputerUse {
                task_summary: string_arg(&arguments, "task_summary")?,
                screenshot_params: Some(api::message::tool_call::ScreenshotParams {
                    max_long_edge_px: 1280,
                    max_total_px: 1_600_000,
                    ..Default::default()
                }),
            })
        }
        "use_computer" => {
            anyhow::ensure!(computer_use_enabled && computer_approved, "Computer use requires explicit user approval in Warp");
            Tool::UseComputer(parse_use_computer(&arguments)?)
        }
        other => bail!("Unsupported local tool: {other}"),
    };

    Ok(Some(api::message::ToolCall {
        tool_call_id: id,
        tool: Some(tool),
    }))
}

fn parse_use_computer(arguments: &Value) -> anyhow::Result<api::message::tool_call::UseComputer> {
    use api::message::tool_call::use_computer::action::Type as ActionType;
    use api::message::tool_call::use_computer::Action as ComputerAction;

    let mut actions = Vec::new();
    let requested = arguments.get("actions").and_then(Value::as_array).ok_or_else(|| anyhow!("use_computer.actions must be an array"))?;
    anyhow::ensure!(requested.len() <= 30, "Refusing more than 30 computer actions in one call");
    for action in requested {
        let kind = action.get("type").and_then(Value::as_str).ok_or_else(|| anyhow!("computer action has no type"))?;
        let x = action.get("x").and_then(Value::as_i64).unwrap_or(0) as i32;
        let y = action.get("y").and_then(Value::as_i64).unwrap_or(0) as i32;
        let typ = match kind {
            "mouse_move" => ActionType::MouseMove(api::message::tool_call::use_computer::action::MouseMove {
                to: Some(api::Coordinates { x, y }),
            }),
            "mouse_down" => ActionType::MouseDown(api::message::tool_call::use_computer::action::MouseDown {
                button: mouse_button(action),
                at: Some(api::Coordinates { x, y }),
            }),
            "mouse_up" => ActionType::MouseUp(api::message::tool_call::use_computer::action::MouseUp {
                button: mouse_button(action),
            }),
            "mouse_wheel" => {
                let direction = match action.get("direction").and_then(Value::as_str).unwrap_or("down") {
                    "up" => 0,
                    "down" => 1,
                    "left" => 2,
                    "right" => 3,
                    _ => bail!("Invalid computer scroll direction"),
                };
                ActionType::MouseWheel(api::message::tool_call::use_computer::action::MouseWheel {
                    at: Some(api::Coordinates { x, y }),
                    direction,
                    distance: action.get("pixels").and_then(Value::as_i64).map(|pixels| {
                        api::message::tool_call::use_computer::action::mouse_wheel::Distance::Pixels(pixels as i32)
                    }),
                })
            }
            "wait" => {
                let millis = action.get("milliseconds").and_then(Value::as_i64).unwrap_or(500).clamp(0, 10_000);
                ActionType::Wait(api::message::tool_call::use_computer::action::Wait {
                    duration: Some(prost_types::Duration {
                        seconds: millis / 1000,
                        nanos: ((millis % 1000) * 1_000_000) as i32,
                    }),
                })
            }
            "type_text" => ActionType::TypeText(api::message::tool_call::use_computer::action::TypeText {
                text: string_arg(action, "text")?,
            }),
            "key_down" => ActionType::KeyDown(api::message::tool_call::use_computer::action::KeyDown {
                key: Some(parse_computer_key(&string_arg(action, "key")?)),
            }),
            "key_up" => ActionType::KeyUp(api::message::tool_call::use_computer::action::KeyUp {
                key: Some(parse_computer_key(&string_arg(action, "key")?)),
            }),
            other => bail!("Unsupported computer action: {other}"),
        };
        actions.push(ComputerAction {
            r#type: Some(typ),
            target: None,
        });
    }

    let screenshot_after = arguments.get("screenshot_after").and_then(Value::as_bool).unwrap_or(true);
    Ok(api::message::tool_call::UseComputer {
        actions,
        post_actions_screenshot_params: screenshot_after.then_some(api::message::tool_call::ScreenshotParams {
            max_long_edge_px: 1280,
            max_total_px: 1_600_000,
            ..Default::default()
        }),
        action_summary: arguments.get("action_summary").and_then(Value::as_str).unwrap_or("Local computer use").to_owned(),
    })
}

fn mouse_button(action: &Value) -> i32 {
    match action.get("button").and_then(Value::as_str).unwrap_or("left") {
        "left" => 0,
        "right" => 1,
        "middle" => 2,
        _ => 0,
    }
}

fn parse_computer_key(key: &str) -> api::message::tool_call::use_computer::action::Key {
    use api::message::tool_call::use_computer::action::key::Data;
    let data = if key.chars().count() == 1 {
        Data::Char(key.to_owned())
    } else {
        let keycode = match key.to_ascii_lowercase().as_str() {
            "enter" | "return" => 36,
            "tab" => 48,
            "escape" | "esc" => 53,
            "space" => 49,
            "backspace" => 51,
            "delete" => 117,
            "left" => 123,
            "right" => 124,
            "down" => 125,
            "up" => 126,
            _ => 36,
        };
        Data::Keycode(keycode)
    };
    api::message::tool_call::use_computer::action::Key { data: Some(data) }
}

fn parse_tool_arguments(value: Option<&Value>) -> anyhow::Result<Value> {
    match value {
        Some(Value::Object(_)) => Ok(value.cloned().unwrap_or(Value::Null)),
        Some(Value::String(text)) => serde_json::from_str(text).context("Tool arguments are not valid JSON"),
        _ => bail!("Tool arguments are missing or have the wrong type"),
    }
}

fn string_arg(arguments: &Value, key: &str) -> anyhow::Result<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("Missing string argument: {key}"))
}

fn local_tool_from_proto(call: &api::message::ToolCall) -> Option<(String, Value)> {
    use api::message::tool_call::Tool;
    let tool = call.tool.as_ref()?;
    Some(match tool {
        Tool::RunShellCommand(t) => ("run_shell_command".to_owned(), json!({"command":t.command})),
        Tool::ReadFiles(t) => {
            let first = t.files.first()?;
            ("read_file".to_owned(), json!({"path":first.name}))
        }
        Tool::ApplyFileDiffs(t) => {
            if let Some(diff) = t.diffs.first() {
                ("edit_file".to_owned(), json!({
                    "path":diff.file_path,"old_text":diff.search,"new_text":diff.replace,"summary":t.summary
                }))
            } else if let Some(new_file) = t.new_files.first() {
                ("write_file".to_owned(), json!({"path":new_file.file_path,"content":new_file.content}))
            } else {
                return None;
            }
        }
        Tool::Grep(t) => ("grep".to_owned(), json!({"query":t.queries.join("\n"),"path":t.path})),
        Tool::FileGlobV2(t) => ("file_glob".to_owned(), json!({"pattern":t.patterns.join("\n"),"path":t.search_dir})),
        Tool::SearchCodebase(t) => ("search_codebase".to_owned(), json!({
            "query":t.query,"path_filters":t.path_filters,"codebase_path":t.codebase_path
        })),
        Tool::RequestComputerUse(t) => ("request_computer_use".to_owned(), json!({"task_summary":t.task_summary})),
        Tool::UseComputer(t) => ("use_computer".to_owned(), json!({"action_summary":t.action_summary,"actions":[] })),
        _ => return None,
    })
}

fn local_tool_result_text(result: &api::message::ToolCallResult) -> String {
    use api::message::tool_call_result::Result as ToolResult;
    match result.result.as_ref() {
        Some(ToolResult::RunShellCommand(value)) => format!("{value:?}"),
        Some(ToolResult::ReadFiles(value)) => format!("{value:?}"),
        Some(ToolResult::ApplyFileDiffs(value)) => format!("{value:?}"),
        Some(ToolResult::Grep(value)) => format!("{value:?}"),
        Some(ToolResult::FileGlobV2(value)) => format!("{value:?}"),
        Some(ToolResult::SearchCodebase(value)) => format!("{value:?}"),
        Some(ToolResult::RequestComputerUseResult(value)) => format!("{value:?}"),
        Some(ToolResult::UseComputer(value)) => "Local computer-use action completed.".to_owned(),
        Some(other) => format!("Local tool result: {other:?}"),
        None => "Local tool produced no result.".to_owned(),
    }
}
fn bound_messages(messages: &mut Vec<Value>) {
    // Keep the system message and most recent history; avoid exhausting the local model's context.
    if messages.len() > MAX_HISTORY_MESSAGES + 1 {
        let keep_from = messages.len() - MAX_HISTORY_MESSAGES;
        let system = messages.first().cloned();
        let tail = messages.split_off(keep_from);
        messages.clear();
        if let Some(system) = system {
            messages.push(system);
        }
        messages.extend(tail);
    }
    let mut remaining = MAX_CONTEXT_CHARS;
    for message in messages.iter_mut().skip(1) {
        let content = match message.get("content") {
            Some(Value::String(text)) => Some(text.clone()),
            _ => None,
        };
        if let Some(content) = content {
            let bounded = truncate(&content, remaining);
            message["content"] = Value::String(bounded.clone());
            remaining = remaining.saturating_sub(bounded.chars().count());
            if remaining == 0 {
                break;
            }
        }
    }
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}
