mod event;
mod op;
mod types;

pub use event::{AskOption, AskQuestion, Event, NotifyKind};
pub use op::Op;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::{
        Event, GitFacts, LoginCredential, Op, TaskId, ToolCallId, ToolImageData, ToolOutcome,
        TranscriptEntry,
    };

    #[test]
    fn tool_outcome_image_round_trips() {
        let outcome = ToolOutcome {
            ok: true,
            summary: Some("captured".to_owned()),
            image: Some(ToolImageData {
                media_type: "image/png".to_owned(),
                data: "AAAA".to_owned(),
            }),
            git: None,
        };
        let json = serde_json::to_string(&outcome).unwrap();
        let back: ToolOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(outcome, back);
    }

    #[test]
    fn tool_outcome_without_image_omits_field() {
        let outcome = ToolOutcome {
            ok: false,
            summary: None,
            image: None,
            git: None,
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(!json.contains("image"));
        assert!(!json.contains("git"));
        let back: ToolOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(outcome, back);
    }

    #[test]
    fn tool_outcome_git_facts_round_trip_and_tolerate_older_payloads() {
        let outcome = ToolOutcome {
            ok: true,
            summary: None,
            image: None,
            git: Some(Box::new(GitFacts {
                head: Some("a1b2c3d".to_owned()),
                subject: Some("feat: git-aware transcript rows".to_owned()),
                branch: Some("feat/git-ui".to_owned()),
                upstream: Some("origin/feat/git-ui".to_owned()),
                pr: Some(59),
                pr_url: Some("https://github.com/goat-agent/goat/pull/59".to_owned()),
            })),
        };
        let json = serde_json::to_string(&outcome).unwrap();
        let back: ToolOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(outcome, back);

        let older: ToolOutcome = serde_json::from_str(r#"{"ok":true,"summary":null}"#).unwrap();
        assert_eq!(older.git, None);
    }

    #[test]
    fn op_unit_variants_serialize_as_type_object() {
        assert_eq!(
            serde_json::to_string(&Op::Clear {}).unwrap(),
            r#"{"type":"Clear"}"#
        );
        assert_eq!(
            serde_json::to_string(&Op::ListThreads {}).unwrap(),
            r#"{"type":"ListThreads"}"#
        );
        assert_eq!(
            serde_json::to_string(&Op::ResumeLatest {}).unwrap(),
            r#"{"type":"ResumeLatest"}"#
        );
        assert_eq!(
            serde_json::to_string(&Op::Shutdown {}).unwrap(),
            r#"{"type":"Shutdown"}"#
        );
    }

    #[test]
    fn op_struct_variants_serialize_flat_with_type() {
        let op = Op::SubmitMessage {
            id: TaskId(1),
            text: "hi".to_owned(),
            display: None,
            attachments: Vec::new(),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"type":"SubmitMessage","id":"1","text":"hi"}"#);
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn event_serializes_flat_with_type() {
        let ev = Event::TextDelta {
            id: TaskId(1),
            chunk: "x".to_owned(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(json, r#"{"type":"TextDelta","id":"1","chunk":"x"}"#);
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn subagent_group_event_round_trips() {
        let event = Event::SubagentGroupStarted {
            id: TaskId(3),
            group: ToolCallId(1),
            members: vec![crate::SubagentGroupMember {
                call: ToolCallId(1),
                subagent_type: "explore".to_owned(),
                label: "map engine".to_owned(),
                background: false,
            }],
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn transcript_entry_user_serializes_with_type() {
        let entry = TranscriptEntry::User {
            text: "hello".to_owned(),
            attachments: Vec::new(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert_eq!(json, r#"{"type":"User","text":"hello"}"#);
        let back: TranscriptEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn login_credential_api_key_serializes_with_type() {
        let cred = LoginCredential::ApiKey {
            key: "sk-x".to_owned(),
        };
        let json = serde_json::to_string(&cred).unwrap();
        assert_eq!(json, r#"{"type":"ApiKey","key":"sk-x"}"#);
        let back: LoginCredential = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cred);
    }

    #[test]
    fn op_answer_roundtrips() {
        let op = Op::Answer {
            id: TaskId(2),
            call: ToolCallId(5),
            answers: vec!["yes".to_owned()],
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn op_set_mode_roundtrips() {
        let op = Op::SetMode {
            mode: super::Mode::Plan,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"type":"SetMode","mode":"plan"}"#);
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn plan_decision_unit_variant_serializes_as_type_object() {
        let op = Op::ResolvePlan {
            call: ToolCallId(3),
            decision: super::PlanDecision::Approve {},
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains(r#""decision":{"type":"Approve"}"#));
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn plan_decision_reject_carries_feedback() {
        let op = Op::ResolvePlan {
            call: ToolCallId(4),
            decision: super::PlanDecision::Reject {
                feedback: "too big".to_owned(),
            },
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn mode_changed_omits_absent_plan_path() {
        let ev = Event::ModeChanged {
            mode: super::Mode::Normal,
            plan_path: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(!json.contains("plan_path"));
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn plan_proposed_roundtrips() {
        let ev = Event::PlanProposed {
            id: TaskId(1),
            call: ToolCallId(2),
            plan: "# Plan".to_owned(),
            path: "/plans/1-demo.md".to_owned(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn mode_toggles_and_defaults_to_normal() {
        assert_eq!(super::Mode::default(), super::Mode::Normal);
        assert!(!super::Mode::Normal.is_plan());
        assert!(super::Mode::Normal.toggled().is_plan());
        assert!(!super::Mode::Plan.toggled().is_plan());
    }

    #[test]
    fn task_id_serializes_as_string() {
        assert_eq!(serde_json::to_string(&TaskId(42)).unwrap(), r#""42""#);
    }

    #[test]
    fn task_id_deserializes_from_string_and_number() {
        let from_str: TaskId = serde_json::from_str(r#""42""#).unwrap();
        let from_num: TaskId = serde_json::from_str("42").unwrap();
        assert_eq!(from_str, TaskId(42));
        assert_eq!(from_num, TaskId(42));
    }

    #[test]
    fn task_id_above_js_safe_integer_roundtrips() {
        let big = TaskId(9_007_199_254_740_993);
        let json = serde_json::to_string(&big).unwrap();
        assert_eq!(json, r#""9007199254740993""#);
        let back: TaskId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, big);
    }

    #[test]
    fn process_id_serializes_as_string() {
        assert_eq!(serde_json::to_string(&super::RunId(7)).unwrap(), r#""7""#);
    }

    #[test]
    fn op_process_kill_roundtrips() {
        let op = Op::ProcessKill {
            process: super::RunId(3),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"type":"ProcessKill","process":"3"}"#);
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn op_process_watch_roundtrips() {
        let op = Op::ProcessWatch {
            process: super::RunId(4),
            on: true,
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn event_process_started_roundtrips() {
        let ev = Event::ProcessStarted {
            process: super::RunId(1),
            command: "pnpm dev".to_owned(),
            watched: false,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn event_process_exited_omits_code_when_absent() {
        let ev = Event::ProcessExited {
            process: super::RunId(1),
            code: None,
            reason: super::ProcessExitReason::Killed,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(!json.contains("code"));
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn transcript_entry_process_roundtrips() {
        let entry = TranscriptEntry::Process {
            command: "pnpm dev".to_owned(),
            output: "ready".to_owned(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: TranscriptEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }
}
