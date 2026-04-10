/// HTTP client for the GitHub Copilot Chat API.
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::Value;

use crate::auth::CopilotAuth;

const COPILOT_CHAT_URL: &str = "https://api.githubcopilot.com/chat/completions";
const COPILOT_MODELS_URL: &str = "https://api.githubcopilot.com/models";
const COPILOT_RESPONSES_URL: &str = "https://api.githubcopilot.com/responses";

/// Models that Copilot only exposes via /responses, not /chat/completions.
/// Detected by checking `supported_endpoints` in the models list.
const RESPONSES_ONLY_MODELS: &[&str] = &[
    "gpt-5.4-mini",
    "gpt-5.3-codex",
    "gpt-5.2-codex",
    "goldeneye",
];

const DEFAULT_MODELS: &[&str] = &[
    "gpt-4o",
    "gpt-4o-mini",
    "gpt-4",
    "gpt-3.5-turbo",
    "claude-3.5-sonnet",
    "claude-3.5-haiku",
    "o1-preview",
    "o1-mini",
    "o3-mini",
];

pub struct CopilotClient {
    auth: CopilotAuth,
    http: Client,
    chat_url: String,
    models_url: String,
    responses_url: String,
}

impl CopilotClient {
    pub fn new(auth: CopilotAuth) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        Self {
            auth,
            http,
            chat_url: COPILOT_CHAT_URL.into(),
            models_url: COPILOT_MODELS_URL.into(),
            responses_url: COPILOT_RESPONSES_URL.into(),
        }
    }

    /// Create a client with custom base URLs (for testing with mock servers).
    /// The responses URL is derived from chat_url by replacing `/chat/completions` with `/responses`.
    pub fn new_with_base_urls(auth: CopilotAuth, chat_url: &str, models_url: &str) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let responses_url = chat_url
            .trim_end_matches("/chat/completions")
            .to_string()
            + "/responses";
        Self {
            auth,
            http,
            chat_url: chat_url.into(),
            models_url: models_url.into(),
            responses_url,
        }
    }

    async fn headers(&self) -> Result<Vec<(&'static str, String)>, String> {
        let token = self.auth.get_copilot_token().await?;
        Ok(vec![
            ("Authorization", format!("Bearer {token}")),
            ("Content-Type", "application/json".into()),
            ("Accept", "application/json".into()),
            ("User-Agent", "GitHubCopilotChat/0.1".into()),
            ("Editor-Version", "vscode/1.95.0".into()),
            ("Editor-Plugin-Version", "copilot-chat/0.22.0".into()),
            ("Openai-Intent", "conversation-panel".into()),
            ("Copilot-Integration-Id", "vscode-chat".into()),
        ])
    }

    /// Non-streaming chat completion — returns raw JSON value.
    pub async fn chat_completions(&self, payload: &mut Value) -> Result<Value, String> {
        let model = payload["model"].as_str().unwrap_or("").to_string();
        if is_responses_only(&model) {
            return self.chat_completions_via_responses(payload).await;
        }
        payload["stream"] = Value::Bool(false);
        normalize_token_param(payload);
        let headers = self.headers().await?;

        let mut req = self.http.post(&self.chat_url);
        for (k, v) in &headers {
            req = req.header(*k, v);
        }

        let resp = req
            .json(payload)
            .send()
            .await
            .map_err(|e| format!("Copilot API error: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Copilot API error (HTTP {status}): {body}"));
        }

        resp.json().await.map_err(|e| format!("JSON parse error: {e}"))
    }

    /// Streaming chat completion — returns a stream of SSE `data: ...` strings.
    pub async fn chat_completions_stream(
        &self,
        mut payload: Value,
    ) -> Result<std::pin::Pin<Box<dyn futures_util::Stream<Item = String> + Send>>, String> {
        let model = payload["model"].as_str().unwrap_or("").to_string();
        if is_responses_only(&model) {
            let s = self.chat_completions_stream_via_responses(payload).await?;
            return Ok(Box::pin(s));
        }
        payload["stream"] = Value::Bool(true);
        normalize_token_param(&mut payload);
        let mut headers = self.headers().await?;
        // Override Accept for SSE
        for h in &mut headers {
            if h.0 == "Accept" {
                h.1 = "text/event-stream".into();
            }
        }

        let mut req = self.http.post(&self.chat_url);
        for (k, v) in &headers {
            req = req.header(*k, v);
        }

        let resp = req
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("Copilot streaming error: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Copilot API error (HTTP {status}): {body}"));
        }

        let byte_stream = resp.bytes_stream();
        Ok(Box::pin(async_stream(byte_stream)))
    }

    /// List available models, with fallback to defaults.
    pub async fn list_models(&self) -> Vec<Value> {
        let result: Result<Vec<Value>, _> = async {
            let headers = self.headers().await?;
            let mut req = self.http.get(&self.models_url);
            for (k, v) in &headers {
                req = req.header(*k, v);
            }
            let resp = req.send().await.map_err(|e| format!("{e}"))?;
            if resp.status().as_u16() != 200 {
                return Err("non-200".into());
            }
            let data: Value = resp.json().await.map_err(|e| format!("{e}"))?;
            if let Some(arr) = data.get("data").and_then(|d| d.as_array()) {
                return Ok(arr.clone());
            }
            if let Some(arr) = data.as_array() {
                return Ok(arr.clone());
            }
            Err("unexpected format".into())
        }
        .await;

        result.unwrap_or_else(|_: String| {
            DEFAULT_MODELS
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m,
                        "object": "model",
                        "created": 0,
                        "owned_by": "github-copilot",
                    })
                })
                .collect()
        })
    }
    /// Non-streaming route via /responses for models that don't support /chat/completions.
    async fn chat_completions_via_responses(&self, payload: &Value) -> Result<Value, String> {
        let model = payload["model"].as_str().unwrap_or("unknown").to_string();
        let req_payload = to_responses_payload(payload, false);
        let headers = self.headers().await?;

        let mut req = self.http.post(&self.responses_url);
        for (k, v) in &headers {
            req = req.header(*k, v);
        }

        let resp = req
            .json(&req_payload)
            .send()
            .await
            .map_err(|e| format!("Copilot responses API error: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Copilot API error (HTTP {status}): {body}"));
        }

        let raw: Value = resp.json().await.map_err(|e| format!("JSON parse error: {e}"))?;
        Ok(from_responses_response(raw, &model))
    }

    /// Streaming route via /responses.
    async fn chat_completions_stream_via_responses(
        &self,
        payload: Value,
    ) -> Result<impl futures_util::Stream<Item = String> + use<>, String> {
        let model = payload["model"].as_str().unwrap_or("unknown").to_string();
        let req_payload = to_responses_payload(&payload, true);
        let mut headers = self.headers().await?;
        for h in &mut headers {
            if h.0 == "Accept" {
                h.1 = "text/event-stream".into();
            }
        }

        let mut req = self.http.post(&self.responses_url);
        for (k, v) in &headers {
            req = req.header(*k, v);
        }

        let resp = req
            .json(&req_payload)
            .send()
            .await
            .map_err(|e| format!("Copilot streaming error: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Copilot API error (HTTP {status}): {body}"));
        }

        Ok(responses_sse_to_chat_stream(resp.bytes_stream(), model))
    }
}

fn is_responses_only(model: &str) -> bool {
    RESPONSES_ONLY_MODELS.contains(&model)
}

/// Translate a chat/completions payload into a /responses API payload.
fn to_responses_payload(payload: &Value, stream: bool) -> Value {
    let mut out = serde_json::json!({});
    if let Some(m) = payload.get("model") {
        out["model"] = m.clone();
    }
    // messages → input (same structure, both accept role/content)
    if let Some(msgs) = payload.get("messages") {
        out["input"] = msgs.clone();
    }
    // max_tokens / max_completion_tokens → max_output_tokens (minimum 50 per /responses API)
    if let Some(v) = payload.get("max_completion_tokens").or_else(|| payload.get("max_tokens")) {
        let n = v.as_u64().unwrap_or(50).max(50);
        out["max_output_tokens"] = Value::Number(n.into());
    }
    // Note: temperature and top_p are NOT supported by /responses models (e.g. gpt-5.4-mini)
    // Do not forward them to avoid 400 errors.
    out["stream"] = Value::Bool(stream);
    out
}

/// Translate a /responses API response into chat/completions format.
fn from_responses_response(resp: Value, model: &str) -> Value {
    let id = resp["id"].as_str().unwrap_or("resp_unknown").to_string();
    let created = resp["created_at"].as_u64().unwrap_or(0);
    let resp_model = resp["model"].as_str().unwrap_or(model).to_string();
    let content = resp["output"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|item| item["content"].as_array())
        .and_then(|arr| arr.first())
        .and_then(|part| part["text"].as_str())
        .unwrap_or("")
        .to_string();
    let input_tokens = resp["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let output_tokens = resp["usage"]["output_tokens"].as_u64().unwrap_or(0);
    serde_json::json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": resp_model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": input_tokens,
            "completion_tokens": output_tokens,
            "total_tokens": input_tokens + output_tokens
        }
    })
}

/// Convert a /responses SSE stream into a chat/completions SSE stream.
fn responses_sse_to_chat_stream(
    byte_stream: impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
    model: String,
) -> impl futures_util::Stream<Item = String> {
    futures_util::stream::unfold(
        (byte_stream, String::new(), String::from("resp_unknown"), model),
        |(mut stream, mut buf, mut resp_id, model)| async move {
            loop {
                if let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim().to_string();
                    buf = buf[pos + 1..].to_string();
                    if line.is_empty() || line.starts_with("event: ") {
                        continue;
                    }
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            return None;
                        }
                        if let Ok(event) = serde_json::from_str::<Value>(data) {
                            match event["type"].as_str().unwrap_or("") {
                                "response.created" => {
                                    if let Some(id) = event["response"]["id"].as_str() {
                                        resp_id = id.to_string();
                                    }
                                    let chunk = make_chat_chunk(
                                        &resp_id, &model,
                                        serde_json::json!({"role": "assistant", "content": ""}),
                                        false,
                                    );
                                    return Some((
                                        format!("data: {chunk}\n\n"),
                                        (stream, buf, resp_id, model),
                                    ));
                                }
                                "response.output_text.delta" => {
                                    if let Some(delta) = event["delta"].as_str() {
                                        let chunk = make_chat_chunk(
                                            &resp_id, &model,
                                            serde_json::json!({"content": delta}),
                                            false,
                                        );
                                        return Some((
                                            format!("data: {chunk}\n\n"),
                                            (stream, buf, resp_id, model),
                                        ));
                                    }
                                }
                                "response.completed" => {
                                    let stop = make_chat_chunk(
                                        &resp_id, &model,
                                        serde_json::json!({}),
                                        true,
                                    );
                                    return Some((
                                        format!("data: {stop}\n\ndata: [DONE]\n\n"),
                                        (stream, buf, resp_id, model),
                                    ));
                                }
                                _ => {}
                            }
                        }
                    }
                    continue;
                }
                match stream.next().await {
                    Some(Ok(chunk)) => buf.push_str(&String::from_utf8_lossy(&chunk)),
                    _ => return None,
                }
            }
        },
    )
}

fn make_chat_chunk(id: &str, model: &str, delta: Value, finish: bool) -> String {
    let finish_reason = if finish { serde_json::json!("stop") } else { Value::Null };
    serde_json::to_string(&serde_json::json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": 0,
        "model": model,
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}]
    }))
    .unwrap()
}

/// Rename `max_tokens` → `max_completion_tokens` in the outgoing payload.
/// Newer Copilot-backed models (gpt-5.x, goldeneye, minimax, …) reject
/// `max_tokens` with HTTP 400; all models accept `max_completion_tokens`.
fn normalize_token_param(payload: &mut Value) {
    if let Some(obj) = payload.as_object_mut() {
        if let Some(v) = obj.remove("max_tokens") {
            obj.entry("max_completion_tokens").or_insert(v);
        }
    }
}

/// Convert a reqwest byte stream into an SSE line stream.
fn async_stream(
    byte_stream: impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
) -> impl futures_util::Stream<Item = String> {
    // Accumulate partial lines
    let buffer = String::new();

    futures_util::stream::unfold(
        (byte_stream, buffer),
        |(mut stream, mut buf)| async move {
            loop {
                // Try to extract a complete line from the buffer
                if let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim().to_string();
                    buf = buf[pos + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            return Some(("data: [DONE]\n\n".to_string(), (stream, buf)));
                        }
                        // Validate JSON
                        if serde_json::from_str::<Value>(data).is_ok() {
                            return Some((
                                format!("data: {data}\n\n"),
                                (stream, buf),
                            ));
                        }
                        continue;
                    }
                    continue;
                }

                // Need more data
                match stream.next().await {
                    Some(Ok(chunk)) => {
                        buf.push_str(&String::from_utf8_lossy(&chunk));
                    }
                    _ => return None, // Stream ended
                }
            }
        },
    )
}
