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
}

#[derive(Debug, Deserialize)]
struct TogetherMessage {
    content: String,
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

    let request = TogetherRequest {
        model: model.to_string(),
        max_tokens: 4096,
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

    let api_response: TogetherResponse = response
        .json()
        .await
        .context("Failed to parse Together.ai response")?;

    let notes = api_response
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    log::info!("Meeting notes generated successfully (Together.ai)");

    Ok(notes)
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
