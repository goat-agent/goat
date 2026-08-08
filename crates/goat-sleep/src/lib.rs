use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::StreamExt;
use goat_memory::facts::FactOrigin;
use goat_memory::{Audience, MemoryEngine, NewFact, Scope};
use goat_model::Model;
use goat_provider::{
    ChunkStream, ContentBlock, Message, MessageRole, Provider, Request, StreamChunk, StreamError,
    ToolChoice,
};
use serde::Deserialize;
use tracing::{info, warn};

mod schedule;
pub use schedule::{SleepConfig, spawn};

const LLM_IDLE: Duration = Duration::from_mins(1);
const DECAY_FACTOR: f32 = 0.98;
const MAX_MESSAGES: usize = 200;

const DISTILL_SYSTEM: &str = "You are the memory of a personal assistant, doing \
nightly consolidation. Given a transcript, write a terse third-person daily note: \
what happened, decisions made, open loops, and anything durable worth remembering. \
Use short markdown with `##` section headings. No preamble, no filler.";

const EXTRACT_SYSTEM: &str = "Extract durable facts about the owner from the note. \
A durable fact is a stable preference, relationship, commitment, or attribute — not \
small talk or one-off events. Return a JSON array of objects with fields: \
\"text\" (the fact, third person), \"subject\" (optional short entity slug), and \
\"importance\" (0.0-1.0). Return [] if nothing is durable. Return ONLY the JSON.";

pub struct TranscriptLine {
    pub role: &'static str,
    pub text: String,
}

#[derive(Debug, Deserialize)]
struct ExtractedFact {
    text: String,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    importance: Option<f32>,
}

pub async fn run_once(
    engine: &MemoryEngine,
    provider: &Arc<dyn Provider>,
    model: &Model,
    scope: &Scope,
    audience: &Audience,
    transcript: &[TranscriptLine],
) -> anyhow::Result<String> {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let year = Utc::now().format("%Y").to_string();
    let note_path = format!("notes/{year}/{today}.md");

    let mut changes = Vec::new();

    if !transcript.is_empty() {
        let body = render_transcript(transcript);
        let note = complete(
            provider,
            model,
            DISTILL_SYSTEM,
            &format!("Transcript:\n{body}"),
        )
        .await?;
        if !note.trim().is_empty() {
            let block = format!("\n## {today}\n{}\n", note.trim());
            engine.append_file(scope, &note_path, &block).await?;
            changes.push(format!(
                "distilled {} messages into {note_path}",
                transcript.len()
            ));

            let extracted = extract_facts(provider, model, &note).await;
            let mut added = 0;
            for f in extracted {
                if f.text.trim().is_empty() {
                    continue;
                }
                if fact_exists(engine, audience, scope, &f.text).await {
                    continue;
                }
                let nf = NewFact {
                    scope: scope.clone(),
                    audience: audience.clone(),
                    subject: f.subject,
                    text: f.text.trim().to_string(),
                    origin: FactOrigin::Consolidated,
                    source_kind: "note".into(),
                    source_ref: note_path.clone(),
                    importance: f.importance.unwrap_or(0.5).clamp(0.0, 1.0),
                };
                if let Err(e) = engine.assert_fact(&nf).await {
                    warn!(error = %e, "sleep: assert_fact failed");
                } else {
                    added += 1;
                }
            }
            if added > 0 {
                changes.push(format!("recorded {added} new fact(s)"));
            }
        }
    }

    if let Ok(n) = engine.decay_scope(scope, DECAY_FACTOR).await
        && n > 0
    {
        changes.push(format!("decayed {n} fact(s)"));
    }

    let summary = if changes.is_empty() {
        "nothing to consolidate".to_string()
    } else {
        changes.join("; ")
    };
    let entry = format!("\n## {today}\n{summary}\n");
    if let Err(e) = engine.append_file(scope, "journal.md", &entry).await {
        warn!(error = %e, "sleep: journal append failed");
    }

    info!(scope = %scope.as_key(), summary = %summary, "sleep consolidation done");
    Ok(summary)
}

async fn extract_facts(
    provider: &Arc<dyn Provider>,
    model: &Model,
    note: &str,
) -> Vec<ExtractedFact> {
    let raw = match complete(provider, model, EXTRACT_SYSTEM, note).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "sleep: fact extraction failed");
            return Vec::new();
        }
    };
    let cleaned = strip_code_fence(&raw);
    match serde_json::from_str::<Vec<ExtractedFact>>(&cleaned) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, raw = %cleaned, "sleep: fact JSON parse failed");
            Vec::new()
        }
    }
}

async fn fact_exists(
    engine: &MemoryEngine,
    audience: &Audience,
    scope: &Scope,
    text: &str,
) -> bool {
    let needle = text.trim().to_lowercase();
    match engine.current_facts(audience, scope, None, 200).await {
        Ok(facts) => facts.iter().any(|f| {
            f.text.to_lowercase().contains(&needle) || needle.contains(&f.text.to_lowercase())
        }),
        Err(_) => false,
    }
}

async fn complete(
    provider: &Arc<dyn Provider>,
    model: &Model,
    system: &str,
    user: &str,
) -> anyhow::Result<String> {
    let req = Request {
        model: model.id.clone(),
        messages: vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: user.to_string(),
            }],
        }],
        tools: Vec::new(),
        effort: None,
        tool_choice: ToolChoice::Auto,
        temperature: None,
        max_tokens: Some(2048),
        system: Some(system.to_string()),
    };
    let stream = provider.stream(req).await?;
    let text = drain_to_text(stream, LLM_IDLE).await?;
    Ok(text.trim().to_string())
}

async fn drain_to_text(mut stream: ChunkStream, idle: Duration) -> Result<String, StreamError> {
    let mut text = String::new();
    loop {
        match tokio::time::timeout(idle, stream.next()).await {
            Err(_elapsed) => return Err(StreamError::transport("llm stream idle timeout")),
            Ok(None) => return Ok(text),
            Ok(Some(Err(e))) => return Err(e),
            Ok(Some(Ok(chunk))) => {
                if let StreamChunk::TextDelta { text: delta } = chunk {
                    text.push_str(&delta);
                }
            }
        }
    }
}

fn render_transcript(lines: &[TranscriptLine]) -> String {
    lines
        .iter()
        .take(MAX_MESSAGES)
        .map(|l| format!("{}: {}", l.role, l.text))
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_code_fence(s: &str) -> String {
    let t = s.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t);
    t.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_fence_variants() {
        assert_eq!(strip_code_fence("```json\n[]\n```"), "[]");
        assert_eq!(strip_code_fence("```\n[1]\n```"), "[1]");
        assert_eq!(strip_code_fence("[2]"), "[2]");
    }

    #[test]
    fn render_caps_messages() {
        let lines: Vec<TranscriptLine> = (0..300)
            .map(|i| TranscriptLine {
                role: "user",
                text: format!("m{i}"),
            })
            .collect();
        let out = render_transcript(&lines);
        assert_eq!(out.lines().count(), MAX_MESSAGES);
    }
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use async_trait::async_trait;
    use goat_provider::{AuthMethod, Capabilities, ProviderId, ProviderMetadata};
    use std::sync::Arc;

    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: false,
                auth: AuthMethod::None,
                images: false,
            }
        }
        fn metadata(&self) -> ProviderMetadata {
            ProviderMetadata::default()
        }
        async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError> {
            let sys = req.system.unwrap_or_default();
            let text = if sys.contains("consolidation") {
                "## Summary\nThe owner mentioned they moved to Berlin.".to_string()
            } else {
                r#"[{"text":"the owner lives in Berlin","subject":"location","importance":0.8}]"#
                    .to_string()
            };
            Ok(Box::pin(async_stream::try_stream! {
                yield StreamChunk::TextDelta { text };
            }))
        }
        fn discover(
            &self,
            _out: tokio::sync::mpsc::Sender<goat_provider::Model>,
        ) -> tokio::task::JoinHandle<()> {
            tokio::spawn(async {})
        }
    }

    async fn engine() -> (tempfile::TempDir, MemoryEngine) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        let eng = MemoryEngine::open(&path, dir.path(), None, 180.0)
            .await
            .unwrap();
        (dir, eng)
    }

    #[tokio::test]
    async fn full_pipeline_distills_extracts_and_journals() {
        let (_d, eng) = engine().await;
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let model = Model::new(ProviderId::from("mock"), "m");
        let transcript = vec![
            TranscriptLine {
                role: "user",
                text: "I just moved to Berlin".into(),
            },
            TranscriptLine {
                role: "assistant",
                text: "Congrats on the move!".into(),
            },
        ];

        let summary = run_once(
            &eng,
            &provider,
            &model,
            &Scope::Owner,
            &Audience::global(),
            &transcript,
        )
        .await
        .unwrap();
        assert!(summary.contains("distilled"), "summary: {summary}");
        assert!(summary.contains("fact"), "summary: {summary}");

        let facts = eng
            .current_facts(&Audience::global(), &Scope::Owner, None, 10)
            .await
            .unwrap();
        assert_eq!(facts.len(), 1);
        assert!(facts[0].text.contains("Berlin"));
        assert_eq!(facts[0].origin.as_str(), "consolidated");

        let files = eng.files().list(&Scope::Owner).await.unwrap();
        assert!(files.iter().any(|f| f.starts_with("notes/")));
        assert!(files.iter().any(|f| f == "journal.md"));

        let _ = run_once(
            &eng,
            &provider,
            &model,
            &Scope::Owner,
            &Audience::global(),
            &transcript,
        )
        .await
        .unwrap();
        let facts2 = eng
            .current_facts(&Audience::global(), &Scope::Owner, None, 10)
            .await
            .unwrap();
        assert_eq!(
            facts2.len(),
            1,
            "dedup should prevent a second identical fact"
        );
    }

    #[tokio::test]
    async fn empty_transcript_only_decays_and_journals() {
        let (_d, eng) = engine().await;
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let model = Model::new(ProviderId::from("mock"), "m");
        let summary = run_once(
            &eng,
            &provider,
            &model,
            &Scope::Self_,
            &Audience::global(),
            &[],
        )
        .await
        .unwrap();
        assert!(
            summary.contains("nothing") || summary.contains("decay"),
            "{summary}"
        );
        let files = eng.files().list(&Scope::Self_).await.unwrap();
        assert!(files.iter().any(|f| f == "journal.md"));
        assert!(
            !files.iter().any(|f| f.starts_with("notes/")),
            "no note without transcript"
        );
    }
}
