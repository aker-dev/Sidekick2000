use crate::transcribe::TranscriptSegment;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedSegment {
    pub speaker: String,
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Merge two speaker-labeled transcript streams into a single time-sorted list.
/// Since each stream comes from a distinct device with a known speaker, no
/// diarization is needed — segments are labeled directly.
pub fn merge_dual_transcripts(
    local: &[TranscriptSegment],
    local_speaker: &str,
    remote: &[TranscriptSegment],
    remote_speaker: &str,
) -> Vec<MergedSegment> {
    let mut result: Vec<MergedSegment> = local
        .iter()
        .map(|s| MergedSegment {
            speaker: local_speaker.to_string(),
            start: s.start,
            end: s.end,
            text: s.text.trim().to_string(),
        })
        .chain(remote.iter().map(|s| MergedSegment {
            speaker: remote_speaker.to_string(),
            start: s.start,
            end: s.end,
            text: s.text.trim().to_string(),
        }))
        .collect();

    result.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));

    log::info!(
        "Merged dual transcripts: {} local + {} remote = {} segments",
        local.len(),
        remote.len(),
        result.len()
    );

    result
}
