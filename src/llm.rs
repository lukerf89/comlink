use std::{
    io::{Read, Write},
    net::TcpStream,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{LlmProvider, LocalLlmConfig, StyleProfile};

pub const LLM_CONTEXT_POLICY: &str = "text-only; no audio; no external context";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequestRecord {
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub mode: String,
    pub input_kind: String,
    pub input_bytes: usize,
    pub context_policy: String,
    pub instruction: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRewriteRecord {
    pub attempted: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<LlmRequestRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RewriteInput<'a> {
    pub mode: &'a str,
    pub text: &'a str,
    pub instruction: &'a str,
    pub profile: Option<&'a StyleProfile>,
}

#[derive(Debug, Clone)]
pub struct RewriteSuccess {
    pub text: String,
    pub record: LlmRewriteRecord,
}

pub fn rewrite(
    config: &LocalLlmConfig,
    input: RewriteInput<'_>,
) -> Result<RewriteSuccess, Box<LlmRewriteRecord>> {
    if !config.enabled {
        return Err(Box::new(skipped_record("local LLM is disabled")));
    }
    let model = config
        .model
        .as_deref()
        .ok_or_else(|| Box::new(skipped_record("local LLM model is not configured")))?;
    let request = LlmRequestRecord {
        provider: config.provider.as_str().to_string(),
        endpoint: config.endpoint.clone(),
        model: model.to_string(),
        mode: input.mode.to_string(),
        input_kind: "text".to_string(),
        input_bytes: input.text.len(),
        context_policy: LLM_CONTEXT_POLICY.to_string(),
        instruction: input.instruction.to_string(),
        style_profile: input.profile.map(|profile| profile.name.clone()),
    };
    let prompt = build_prompt(&input);
    let body = match config.provider {
        LlmProvider::Ollama => json!({
            "model": model,
            "prompt": prompt,
            "stream": false
        }),
        LlmProvider::OpenAiCompatible => json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system_prompt(input.instruction, input.profile)},
                {"role": "user", "content": input.text}
            ],
            "stream": false
        }),
    };

    let response = post_json(&config.endpoint, &body.to_string(), config.timeout_ms)
        .map_err(|error| Box::new(fallback_record(Some(request.clone()), error)))?;
    let text = match config.provider {
        LlmProvider::Ollama => parse_ollama_response(&response),
        LlmProvider::OpenAiCompatible => parse_openai_response(&response),
    }
    .map_err(|error| Box::new(fallback_record(Some(request.clone()), error)))?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(Box::new(fallback_record(
            Some(request),
            "local LLM returned empty text".to_string(),
        )));
    }

    Ok(RewriteSuccess {
        text,
        record: LlmRewriteRecord {
            attempted: true,
            status: "rewritten".to_string(),
            request: Some(request),
            error: None,
        },
    })
}

pub fn skipped_record(reason: &str) -> LlmRewriteRecord {
    LlmRewriteRecord {
        attempted: false,
        status: "skipped".to_string(),
        request: None,
        error: Some(reason.to_string()),
    }
}

pub fn fallback_record(request: Option<LlmRequestRecord>, error: String) -> LlmRewriteRecord {
    LlmRewriteRecord {
        attempted: request.is_some(),
        status: "fallback".to_string(),
        request,
        error: Some(error),
    }
}

fn build_prompt(input: &RewriteInput<'_>) -> String {
    format!(
        "{}\n\nContext policy: {}\n\n{}\n\nText:\n{}",
        system_prompt(input.instruction, input.profile),
        LLM_CONTEXT_POLICY,
        "Return only the rewritten text. Preserve facts, technical terms, filenames, URLs, and code symbols.",
        input.text
    )
}

fn system_prompt(instruction: &str, profile: Option<&StyleProfile>) -> String {
    let mut prompt = format!(
        "You rewrite dictated speech into paste-ready text.\nMode instruction: {instruction}"
    );
    if let Some(profile) = profile {
        prompt.push_str(&format!(
            "\nStyle profile '{}': {}",
            profile.name, profile.summary
        ));
        for example in &profile.examples {
            prompt.push_str(&format!(
                "\nExample input: {}\nExample output: {}",
                example.input, example.output
            ));
        }
    }
    prompt
}

fn post_json(endpoint: &str, body: &str, timeout_ms: u64) -> Result<String, String> {
    let endpoint = parse_http_endpoint(endpoint)?;
    let address = format!("{}:{}", endpoint.host, endpoint.port);
    let mut stream =
        TcpStream::connect(address).map_err(|error| format!("LLM connection failed: {error}"))?;
    let timeout = Duration::from_millis(timeout_ms.max(1));
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| format!("LLM read timeout setup failed: {error}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| format!("LLM write timeout setup failed: {error}"))?;

    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        endpoint.path,
        endpoint.host,
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("LLM request failed: {error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("LLM response failed: {error}"))?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| "LLM response was not valid HTTP".to_string())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("000");
    if !status.starts_with('2') {
        return Err(format!("LLM endpoint returned HTTP {status}"));
    }
    Ok(body.to_string())
}

#[derive(Debug, Clone)]
struct HttpEndpoint {
    host: String,
    port: u16,
    path: String,
}

fn parse_http_endpoint(endpoint: &str) -> Result<HttpEndpoint, String> {
    let rest = endpoint
        .strip_prefix("http://")
        .ok_or_else(|| "only http local LLM endpoints are supported".to_string())?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() {
        return Err("LLM endpoint is missing a host".to_string());
    }
    let (host, port) = if let Some((host, port)) = authority.rsplit_once(':') {
        let port = port
            .parse::<u16>()
            .map_err(|_| "LLM endpoint port is invalid".to_string())?;
        (host, port)
    } else {
        (authority, 80)
    };
    if host.is_empty() {
        return Err("LLM endpoint is missing a host".to_string());
    }
    Ok(HttpEndpoint {
        host: host.to_string(),
        port,
        path: format!("/{path}"),
    })
}

fn parse_ollama_response(response: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(response).map_err(|error| format!("invalid Ollama JSON: {error}"))?;
    value["response"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "Ollama response did not include response text".to_string())
}

fn parse_openai_response(response: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(response)
        .map_err(|error| format!("invalid OpenAI-compatible JSON: {error}"))?;
    value["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "OpenAI-compatible response did not include message content".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_record_names_text_only_policy() {
        let config = LocalLlmConfig {
            enabled: true,
            model: Some("local".to_string()),
            ..Default::default()
        };
        let request = LlmRequestRecord {
            provider: config.provider.as_str().to_string(),
            endpoint: config.endpoint,
            model: "local".to_string(),
            mode: "prompt".to_string(),
            input_kind: "text".to_string(),
            input_bytes: 5,
            context_policy: LLM_CONTEXT_POLICY.to_string(),
            instruction: "rewrite".to_string(),
            style_profile: None,
        };

        assert_eq!(request.input_kind, "text");
        assert_eq!(
            request.context_policy,
            "text-only; no audio; no external context"
        );
    }

    #[test]
    fn parses_local_http_endpoint() {
        let endpoint = parse_http_endpoint("http://127.0.0.1:11434/api/generate").unwrap();

        assert_eq!(endpoint.host, "127.0.0.1");
        assert_eq!(endpoint.port, 11434);
        assert_eq!(endpoint.path, "/api/generate");
    }
}
