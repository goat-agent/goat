mod action;
mod computer;
mod state;
pub mod transport;

use std::sync::Arc;

use goat_tool::{Tool, ToolFuture, ToolSandbox};
use serde_json::{Value, json};

pub use computer::Computer;
pub use transport::{ComputerError, Transport, TransportFuture};

pub const NAME: &str = "Computer";
pub const DESCRIPTION: &str = "Control the desktop through screenshots, accessibility snapshots, and input. Coordinates and zoom regions are pixels of the latest screenshot, including a zoomed screenshot; take a screenshot before using coordinates. Prefer snapshot plus ref for text fields and buttons, and coordinates for everything else. Refs are eN from the latest snapshot or qualified sN:eN; take a new snapshot when refs are stale. Snapshot frames use logical display points, not screenshot pixels. Click accepts either x and y or ref, never both. Drag uses x,y to to_x,to_y. Scroll uses signed dx and dy in lines, with positive dy scrolling down. Key chords are lowercase tokens joined by +: distinct modifiers cmd ctrl alt shift fn, followed by exactly one key. Keys are a single character, enter/return, tab, escape/esc, space, backspace, delete, up/down/left/right, home/end, pageup/pagedown, or f1 through f12. Mutating actions wait 300ms and take a screenshot unless observe is false. Wait sleeps ms, capped at 10000, then takes a screenshot unless observe is false. Screenshots default to max_width 1280.";

pub fn parameters() -> Value {
    json!({
        "type": "object",
        "required": ["action"],
        "additionalProperties": false,
        "properties": {
            "action": {
                "type": "string",
                "enum": ["screenshot", "zoom", "snapshot", "apps", "focus_app", "click",
                    "double_click", "triple_click", "right_click", "move", "drag", "scroll",
                    "type", "key", "press", "set_value", "wait"]
            },
            "x": {"type": "number", "minimum": 0, "description": "Horizontal pixel in the latest screenshot; required with y for coordinate actions."},
            "y": {"type": "number", "minimum": 0, "description": "Vertical pixel in the latest screenshot; required with x for coordinate actions."},
            "ref": {"type": "string", "description": "Accessibility ref eN or sN:eN; required for press and set_value, or used instead of x,y for clicks."},
            "snapshot_id": {"type": "string", "description": "Optional snapshot sN for ref; must agree with a qualified ref and the latest snapshot."},
            "to_x": {"type": "number", "minimum": 0, "description": "Required drag destination horizontal pixel."},
            "to_y": {"type": "number", "minimum": 0, "description": "Required drag destination vertical pixel."},
            "dx": {"type": "integer", "minimum": i32::MIN, "maximum": i32::MAX, "description": "Required scroll horizontal lines; use 0 for vertical-only scrolling."},
            "dy": {"type": "integer", "minimum": i32::MIN, "maximum": i32::MAX, "description": "Required scroll vertical lines; positive scrolls down, use 0 for horizontal-only scrolling."},
            "text": {"type": "string", "description": "Required text for type."},
            "keys": {"type": "string", "description": "Required key chord, such as cmd+shift+t or enter."},
            "value": {"type": "string", "description": "Required new accessibility value for set_value."},
            "app": {"type": "string", "description": "App localized name or bundle ID; required for focus_app, optional for snapshot (defaults to frontmost app)."},
            "region": {"type": "array", "items": {"type": "number"}, "minItems": 4, "maxItems": 4, "description": "Required zoom bounds [x0,y0,x1,y1] in latest screenshot pixels, with positive area."},
            "ms": {"type": "integer", "minimum": 0, "description": "Required wait duration in milliseconds, capped at 10000."},
            "observe": {"type": "boolean", "default": true, "description": "Take a fresh screenshot after input or wait; does not affect screenshot, zoom, snapshot, or apps."},
            "max_width": {"type": "integer", "minimum": 1, "maximum": u32::MAX, "default": 1280, "description": "Maximum screenshot width, preserving aspect ratio."}
        }
    })
}

pub fn computer_tool(transport: Arc<dyn Transport>) -> Box<dyn Tool> {
    Box::new(Computer::new(transport))
}

impl Tool for Computer {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn parameters(&self) -> Value {
        parameters()
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolSandbox) -> ToolFuture<'a> {
        Box::pin(Computer::run(self, input))
    }
}

#[cfg(test)]
mod tests {
    use goat_api::{
        AxNode, ComputerCommand, ComputerTarget, HostComputerOutput, MouseButton, Rect, RunningApp,
    };
    use goat_tool::{ToolContent, ToolErrorClass};
    use tokio::time::{Duration, Instant};

    use super::transport::fake::FakeComputer;
    use super::*;

    fn display_bounds() -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1440.0,
            height: 900.0,
        }
    }

    fn screenshot(width: u32, height: u32, display: Rect) -> HostComputerOutput {
        HostComputerOutput::Screenshot {
            media_type: "image/png".into(),
            data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aX1sAAAAASUVORK5CYII=".into(),
            width,
            height,
            display,
        }
    }

    fn app() -> RunningApp {
        RunningApp {
            name: "Safari".into(),
            pid: 812,
            bundle_id: Some("com.apple.Safari".into()),
            frontmost: true,
        }
    }

    fn snapshot(id: &str) -> HostComputerOutput {
        HostComputerOutput::Snapshot {
            snapshot_id: id.into(),
            app: app(),
            window_title: Some("GitHub".into()),
            window: display_bounds(),
            nodes: vec![],
        }
    }

    #[tokio::test]
    async fn coordinates_require_a_screenshot_without_posting_input() {
        let fake = Arc::new(FakeComputer::default());
        let computer = Computer::new(fake.clone());
        let error = computer
            .run(r#"{"action":"click","x":1,"y":2,"observe":false}"#)
            .await
            .err()
            .unwrap();
        assert_eq!(error.class(), ToolErrorClass::InvalidInput);
        assert!(error.to_string().contains("take a screenshot first"));
        assert!(fake.commands().is_empty());
    }

    #[tokio::test]
    async fn screenshot_pixels_map_to_logical_display_points() {
        let fake = Arc::new(FakeComputer::default());
        fake.push(Ok(screenshot(1280, 800, display_bounds())));
        fake.push(Ok(HostComputerOutput::Done {}));
        let computer = Computer::new(fake.clone());
        let image = computer.run(r#"{"action":"screenshot"}"#).await.unwrap();
        assert_eq!(image.summary.as_deref(), Some("screenshot 1280x800"));
        assert!(
            matches!(image.content, ToolContent::Image(image) if image.media_type == "image/png")
        );
        let output = computer
            .run(r#"{"action":"click","x":640,"y":400,"observe":false}"#)
            .await
            .unwrap();
        assert_eq!(output.as_text(), Some("done"));
        assert_eq!(
            fake.commands(),
            vec![
                ComputerCommand::Screenshot { max_width: 1280 },
                ComputerCommand::Click {
                    at: ComputerTarget::Point { x: 720.0, y: 450.0 },
                    button: MouseButton::Left,
                    count: 1,
                },
            ]
        );
    }

    #[tokio::test]
    async fn refs_resolve_only_against_the_latest_snapshot() {
        let fake = Arc::new(FakeComputer::default());
        fake.push(Ok(snapshot("s1")));
        fake.push(Ok(snapshot("s2")));
        fake.push(Ok(HostComputerOutput::Done {}));
        fake.push(Ok(HostComputerOutput::Done {}));
        let computer = Computer::new(fake.clone());
        computer.run(r#"{"action":"snapshot"}"#).await.unwrap();
        computer.run(r#"{"action":"snapshot"}"#).await.unwrap();
        let error = computer
            .run(r#"{"action":"click","ref":"s1:e2","observe":false}"#)
            .await
            .err()
            .unwrap();
        assert_eq!(error.class(), ToolErrorClass::InvalidInput);
        assert!(error.to_string().contains("latest snapshot (s2)"));
        computer
            .run(r#"{"action":"press","ref":"e2","observe":false}"#)
            .await
            .unwrap();
        computer
            .run(r#"{"action":"set_value","ref":"s2:e2","value":"updated","observe":false}"#)
            .await
            .unwrap();
        assert_eq!(
            fake.commands(),
            vec![
                ComputerCommand::Snapshot { app: None },
                ComputerCommand::Snapshot { app: None },
                ComputerCommand::Press {
                    snapshot_id: "s2".into(),
                    reference: "e2".into()
                },
                ComputerCommand::SetValue {
                    snapshot_id: "s2".into(),
                    reference: "e2".into(),
                    value: "updated".into(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn zoom_replaces_coordinate_scale_and_origin() {
        let fake = Arc::new(FakeComputer::default());
        let display = Rect {
            x: 100.0,
            y: 50.0,
            ..display_bounds()
        };
        let region = Rect {
            x: 460.0,
            y: 275.0,
            width: 720.0,
            height: 450.0,
        };
        fake.push(Ok(screenshot(1280, 800, display)));
        fake.push(Ok(screenshot(400, 250, region.clone())));
        fake.push(Ok(HostComputerOutput::Done {}));
        let computer = Computer::new(fake.clone());
        computer.run(r#"{"action":"screenshot"}"#).await.unwrap();
        let zoom = computer
            .run(r#"{"action":"zoom","region":[320,200,960,600],"max_width":400}"#)
            .await
            .unwrap();
        assert_eq!(zoom.summary.as_deref(), Some("screenshot 400x250"));
        computer
            .run(r#"{"action":"click","x":200,"y":125,"observe":false}"#)
            .await
            .unwrap();
        assert_eq!(
            fake.commands(),
            vec![
                ComputerCommand::Screenshot { max_width: 1280 },
                ComputerCommand::Zoom {
                    region,
                    max_width: 400
                },
                ComputerCommand::Click {
                    at: ComputerTarget::Point { x: 820.0, y: 500.0 },
                    button: MouseButton::Left,
                    count: 1,
                },
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_actions_wait_for_the_preceding_observation() {
        let fake = Arc::new(FakeComputer::default());
        fake.push(Ok(screenshot(1280, 800, display_bounds())));
        fake.push(Ok(HostComputerOutput::Done {}));
        fake.push(Ok(screenshot(640, 400, display_bounds())));
        fake.push(Ok(HostComputerOutput::Done {}));
        fake.push(Ok(screenshot(1280, 800, display_bounds())));
        let computer = Computer::new(fake.clone());
        computer.run(r#"{"action":"screenshot"}"#).await.unwrap();
        let started = Instant::now();
        let (typed, clicked) = tokio::join!(
            computer.run(r#"{"action":"type","text":"hello","max_width":640}"#),
            computer.run(r#"{"action":"click","x":320,"y":200}"#),
        );
        assert_eq!(
            typed.unwrap().summary.as_deref(),
            Some("done; screenshot 640x400")
        );
        assert_eq!(
            clicked.unwrap().summary.as_deref(),
            Some("done; screenshot 1280x800")
        );
        assert_eq!(started.elapsed(), Duration::from_millis(600));
        assert_eq!(
            fake.commands(),
            vec![
                ComputerCommand::Screenshot { max_width: 1280 },
                ComputerCommand::Type {
                    text: "hello".into()
                },
                ComputerCommand::Screenshot { max_width: 640 },
                ComputerCommand::Click {
                    at: ComputerTarget::Point { x: 720.0, y: 450.0 },
                    button: MouseButton::Left,
                    count: 1,
                },
                ComputerCommand::Screenshot { max_width: 1280 },
            ]
        );
    }

    #[tokio::test]
    async fn invalid_parameters_never_post_partial_actions() {
        let fake = Arc::new(FakeComputer::default());
        fake.push(Ok(screenshot(1280, 800, display_bounds())));
        let computer = Computer::new(fake.clone());
        computer.run(r#"{"action":"screenshot"}"#).await.unwrap();
        for input in [
            r#"{"action":"click","x":2,"y":2,"max_width":0}"#,
            r#"{"action":"type","text":"hello","observe":"false"}"#,
            r#"{"action":"drag","x":10,"y":20,"to_x":-1,"to_y":20}"#,
            r#"{"action":"zoom","region":[20,20,10,10]}"#,
            r#"{"action":"click","x":2,"y":2,"ref":"e1"}"#,
            r#"{"action":"key","keys":"cmd+unknown"}"#,
            r#"{"action":"key","keys":"cmd+cmd+a"}"#,
            r#"{"action":"key","keys":"a+shift"}"#,
            r#"{"action":"press","ref":"s1:e2","snapshot_id":"s2"}"#,
            r#"{"action":"set_value","ref":"e2"}"#,
            r#"{"action":"type","text":"hello","unexpected":true}"#,
        ] {
            let error = computer.run(input).await.err().unwrap();
            assert_eq!(error.class(), ToolErrorClass::InvalidInput, "{input}");
        }
        assert_eq!(
            fake.commands(),
            vec![ComputerCommand::Screenshot { max_width: 1280 }]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn wait_caps_duration_and_observes_without_an_extra_settle_delay() {
        let fake = Arc::new(FakeComputer::default());
        fake.push(Ok(screenshot(1280, 800, display_bounds())));
        let computer = Computer::new(fake.clone());
        let started = Instant::now();
        let output = computer
            .run(r#"{"action":"wait","ms":20000}"#)
            .await
            .unwrap();
        assert_eq!(started.elapsed(), Duration::from_secs(10));
        assert_eq!(output.summary.as_deref(), Some("done; screenshot 1280x800"));
        let started = Instant::now();
        let output = computer
            .run(r#"{"action":"wait","ms":25,"observe":false}"#)
            .await
            .unwrap();
        assert_eq!(started.elapsed(), Duration::from_millis(25));
        assert_eq!(output.as_text(), Some("done"));
        assert_eq!(
            fake.commands(),
            vec![ComputerCommand::Screenshot { max_width: 1280 }]
        );
    }

    #[tokio::test]
    async fn snapshots_preserve_accessibility_context_and_truncate_unicode_values() {
        let fake = Arc::new(FakeComputer::default());
        fake.push(Ok(HostComputerOutput::Snapshot {
            snapshot_id: "s3".into(),
            app: app(),
            window_title: Some("GitHub".into()),
            window: display_bounds(),
            nodes: vec![AxNode {
                reference: "e2".into(),
                role: "AXTextField".into(),
                title: Some("Address".into()),
                value: Some("é".repeat(121)),
                depth: 0,
                frame: Rect {
                    x: 12.2,
                    y: 40.1,
                    width: 24.0,
                    height: 24.0,
                },
                enabled: true,
                focused: true,
                actions: vec!["AXPress".into()],
            }],
        }));
        fake.push(Ok(HostComputerOutput::Apps {
            apps: vec![
                app(),
                RunningApp {
                    name: "Terminal".into(),
                    pid: 813,
                    bundle_id: None,
                    frontmost: false,
                },
            ],
        }));
        let computer = Computer::new(fake);
        let output = computer.run(r#"{"action":"snapshot"}"#).await.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains("snapshot_id: s3\napp: Safari (pid 812)\nwindow: \"GitHub\""));
        assert!(text.contains("- e2 AXTextField \"Address\""));
        assert!(text.contains("[12,40 24x24] focused"));
        assert!(text.contains(&"é".repeat(120)));
        assert!(!text.contains(&"é".repeat(121)));
        let output = computer.run(r#"{"action":"apps"}"#).await.unwrap();
        assert_eq!(
            output.as_text(),
            Some("* Safari (pid 812, com.apple.Safari)\n  Terminal (pid 813)\n")
        );
    }
}
