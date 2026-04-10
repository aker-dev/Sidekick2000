use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const SYSTEM_PROMPT: &str = r#"You are a professional meeting-notes assistant.
Given a raw speaker-labelled transcript, produce concise meeting notes in Markdown with the following sections:
1. **Participants** – list of speakers identified
2. **Summary** – 3-5 sentence overview
3. **Key Discussion Points** – bullet list
4. **Decisions Made** – bullet list (if any)
5. **Action Items** – checkboxes, formatted as: `- [ ] **Assignee**: Task description` when the responsible person is identifiable, or `- [ ] Task description` otherwise

Be factual. Do not invent information absent from the transcript."#;

const LANGUAGE_INSTRUCTION: &str =
    "Always write the meeting notes in {language}, regardless of the transcript language.";

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<Message>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    text: Option<String>,
}

/// Build the full system prompt with context, speaker info, and language instruction
fn build_system_prompt(
    context: &str,
    speakers: &[(String, String)], // (name, organization)
    language: &str,
) -> String {
    let mut prompt = String::new();

    // Add context file content
    if !context.is_empty() {
        prompt.push_str(context);
        prompt.push_str("\n\n");
    }

    prompt.push_str(SYSTEM_PROMPT);

    // Add speaker info
    if !speakers.is_empty() {
        prompt.push_str("\n\nThe following named participants are expected in this meeting:\n");
        for (name, org) in speakers {
            if org.is_empty() {
                prompt.push_str(&format!("- {}\n", name));
            } else {
                prompt.push_str(&format!("- {} ({})\n", name, org));
            }
        }
        prompt.push_str(
            "\nThe transcript uses diarization labels (SPEAKER_00, SPEAKER_01, …). \
            Before writing the notes:\n\
            1. Infer which label corresponds to which named participant based on the content and context clues.\n\
            2. Replace every SPEAKER_XX label with the actual participant name throughout the notes.\n\
            3. If a label cannot be confidently matched to a name, use \"Unknown Speaker\".\n\
            Never leave SPEAKER_XX labels in the output.",
        );
    }

    // Add language instruction
    if !language.is_empty() {
        let lang_instr = LANGUAGE_INSTRUCTION.replace("{language}", language);
        prompt.push_str("\n");
        prompt.push_str(&lang_instr);
    }

    prompt
}

// ── Together.ai structs ────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct TogetherRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<Message>,
}

#[derive(Debug, Deserialize)]
struct TogetherResponse {
    choices: Vec<TogetherChoice>,
}

#[derive(Debug, Deserialize)]
struct TogetherChoice {
    message: TogetherMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TogetherMessage {
    #[serde(default)]
    content: Option<String>,
    /// Some reasoning models (GLM, DeepSeek-R1, QwQ, …) return the chain of
    /// thought in a separate field and leave `content` empty or truncated.
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
}

/// Strip a leading `<think>…</think>` block emitted by reasoning models.
/// If the block is unclosed (response got truncated mid-reasoning), returns
/// an empty string so the caller can detect the failure.
fn strip_think_block(raw: &str) -> String {
    let trimmed = raw.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<think>") {
        if let Some(end) = rest.find("</think>") {
            return rest[end + "</think>".len()..].trim().to_string();
        }
        // Unclosed think block → truncation
        return String::new();
    }
    raw.trim().to_string()
}

/// Summarize a transcript using a Together.ai chat model
pub async fn summarize_with_together(
    transcript_md: &str,
    context: &str,
    speakers: &[(String, String)],
    language: &str,
    api_key: &str,
    model: &str,
) -> Result<String> {
    let system_prompt = build_system_prompt(context, speakers, language);

    log::info!("Calling Together.ai ({}) to generate meeting notes", model);

    // Reasoning models (GLM-*, DeepSeek-R1, QwQ, …) spend thousands of tokens
    // thinking before the answer. 4096 caps out mid-<think> and returns empty
    // content. Give them room; Together caps responses server-side anyway.
    let is_reasoning_model = model_is_reasoning(model);
    let max_tokens = if is_reasoning_model { 16384 } else { 4096 };

    let request = TogetherRequest {
        model: model.to_string(),
        max_tokens,
        messages: vec![
            Message {
                role: "system".to_string(),
                content: system_prompt,
            },
            Message {
                role: "user".to_string(),
                content: transcript_md.to_string(),
            },
        ],
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .context("Failed to build HTTP client")?;
    let response = client
        .post("https://api.together.xyz/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Failed to send request to Together.ai API")?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Together.ai API error ({}): {}", status, error_text);
    }

    // Read as raw text first so we can log the body on parse failure — the
    // Together API sometimes returns usage/finish_reason info that helps
    // diagnose empty summaries with reasoning models.
    let body = response
        .text()
        .await
        .context("Failed to read Together.ai response body")?;

    let api_response: TogetherResponse = serde_json::from_str(&body).with_context(|| {
        format!(
            "Failed to parse Together.ai response. Raw body: {}",
            truncate_for_log(&body, 2000)
        )
    })?;

    let choice = api_response
        .choices
        .first()
        .context("Together.ai returned no choices")?;

    let finish = choice.finish_reason.as_deref().unwrap_or("unknown");
    let raw_content = choice.message.content.as_deref().unwrap_or("");
    let cleaned = strip_think_block(raw_content);

    // Fallback ladder for reasoning models: stripped content → reasoning_content → reasoning
    let notes = if !cleaned.is_empty() {
        cleaned
    } else if let Some(rc) = choice.message.reasoning_content.as_deref() {
        strip_think_block(rc)
    } else if let Some(r) = choice.message.reasoning.as_deref() {
        strip_think_block(r)
    } else {
        String::new()
    };

    if notes.trim().is_empty() {
        log::error!(
            "Together.ai returned empty notes (finish_reason={}, model={}, raw content len={}). \
             Likely the response was truncated inside a <think> block — try a non-reasoning model \
             or a higher max_tokens.",
            finish,
            model,
            raw_content.len()
        );
        log::debug!("Together.ai raw body: {}", truncate_for_log(&body, 2000));
        anyhow::bail!(
            "Together.ai returned an empty summary (finish_reason={}). \
             The model {} is likely a reasoning model whose response was truncated. \
             Try a non-reasoning model (e.g. meta-llama/Llama-3.3-70B-Instruct-Turbo).",
            finish,
            model
        );
    }

    log::info!(
        "Meeting notes generated successfully (Together.ai, finish_reason={})",
        finish
    );

    Ok(notes)
}

fn model_is_reasoning(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("glm-4.5")
        || m.contains("glm-4.6")
        || m.contains("glm-5")
        || m.contains("deepseek-r1")
        || m.contains("qwq")
        || m.contains("qwen3")
        || m.contains("-thinking")
        || m.contains("-reasoning")
}

fn truncate_for_log(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}… ({} more bytes)", &s[..max], s.len() - max)
    }
}

// ── Anthropic / Claude ─────────────────────────────────────────────────────

/// Summarize a transcript using Claude Sonnet 4.6
pub async fn summarize_with_claude(
    transcript_md: &str,
    context: &str,
    speakers: &[(String, String)],
    language: &str,
    api_key: &str,
) -> Result<String> {
    let system_prompt = build_system_prompt(context, speakers, language);

    log::info!("Calling Claude (claude-sonnet-4-6) to generate meeting notes");

    let request = AnthropicRequest {
        model: "claude-sonnet-4-6".to_string(),
        max_tokens: 4096,
        system: system_prompt,
        messages: vec![Message {
            role: "user".to_string(),
            content: transcript_md.to_string(),
        }],
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .context("Failed to build HTTP client")?;
    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Failed to send request to Anthropic API")?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Anthropic API error ({}): {}", status, error_text);
    }

    let api_response: AnthropicResponse = response
        .json()
        .await
        .context("Failed to parse Anthropic response")?;

    let notes = api_response
        .content
        .first()
        .and_then(|c| c.text.clone())
        .unwrap_or_default();

    log::info!("Meeting notes generated successfully (Claude)");

    Ok(notes)
}
