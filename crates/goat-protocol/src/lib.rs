mod event;
mod op;
mod types;

pub use event::{AskOption, AskQuestion, Event, NotifyKind};
pub use op::Op;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::{
        Event, LoginCredential, Op, TaskId, ToolCallId, ToolImageData, ToolOutcome, TranscriptEntry,
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
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(!json.contains("image"));
        let back: ToolOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(outcome, back);
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
        assert_eq!(
            serde_json::to_string(&super::ProcessId(7)).unwrap(),
            r#""7""#
        );
    }

    #[test]
    fn op_process_kill_roundtrips() {
        let op = Op::ProcessKill {
            process: super::ProcessId(3),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"type":"ProcessKill","process":"3"}"#);
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn op_process_watch_roundtrips() {
        let op = Op::ProcessWatch {
            process: super::ProcessId(4),
            on: true,
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn event_process_started_roundtrips() {
        let ev = Event::ProcessStarted {
            process: super::ProcessId(1),
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
            process: super::ProcessId(1),
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
