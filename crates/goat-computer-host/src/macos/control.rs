use std::{sync::mpsc, thread};

use goat_api::{ComputerCommand, ComputerTarget, HostComputerOutput, Point};

use crate::{Desktop, DesktopError};

#[path = "accessibility.rs"]
mod accessibility;
#[path = "input.rs"]
mod input;

use accessibility::Accessibility;

type Reply = Result<HostComputerOutput, DesktopError>;

struct Request {
    command: ComputerCommand,
    reply: mpsc::SyncSender<Reply>,
}

pub struct MacDesktop {
    requests: mpsc::Sender<Request>,
}

impl MacDesktop {
    pub fn new() -> Result<Self, DesktopError> {
        let (requests, incoming) = mpsc::channel::<Request>();
        let (ready, started) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("goat-computer".into())
            .spawn(move || {
                let devices = match input::Devices::new() {
                    Ok(devices) => devices,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                let mut desktop = NativeDesktop {
                    devices,
                    accessibility: Accessibility::default(),
                };
                if ready.send(Ok(())).is_err() {
                    return;
                }
                for request in incoming {
                    let result = desktop.run(request.command);
                    let _ = request.reply.send(result);
                }
            })
            .map_err(|error| {
                DesktopError::NotStarted(format!("could not start native desktop worker: {error}"))
            })?;
        started.recv().map_err(|error| {
            DesktopError::NotStarted(format!(
                "native desktop worker failed to initialize: {error}"
            ))
        })??;
        Ok(Self { requests })
    }
}

impl Desktop for MacDesktop {
    fn run(&self, command: ComputerCommand) -> Reply {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request { command, reply })
            .map_err(|_| DesktopError::NotStarted("native desktop worker is unavailable".into()))?;
        response.recv().map_err(|_| {
            DesktopError::OutcomeUnknown("native desktop worker stopped during the command".into())
        })?
    }
}

struct NativeDesktop {
    devices: input::Devices,
    accessibility: Accessibility,
}

impl NativeDesktop {
    fn run(&mut self, command: ComputerCommand) -> Reply {
        match command {
            ComputerCommand::Screenshot { max_width } => super::capture::capture(None, max_width),
            ComputerCommand::Zoom { region, max_width } => {
                super::capture::capture(Some(&region), max_width)
            }
            ComputerCommand::Snapshot { app } => self.accessibility.snapshot(app.as_deref()),
            ComputerCommand::Apps {} => Ok(HostComputerOutput::Apps {
                apps: goat_computer_host_apps::running_apps(),
            }),
            ComputerCommand::FocusApp { app } => {
                let app = accessibility::find_app(goat_computer_host_apps::running_apps(), &app)?;
                goat_computer_host_apps::focus_app(app.pid)
                    .map_err(|error| DesktopError::NotStarted(error.to_string()))?;
                Ok(HostComputerOutput::Done {})
            }
            ComputerCommand::Press {
                snapshot_id,
                reference,
            } => {
                self.accessibility.press(&snapshot_id, &reference)?;
                Ok(HostComputerOutput::Done {})
            }
            ComputerCommand::SetValue {
                snapshot_id,
                reference,
                value,
            } => {
                self.accessibility
                    .set_value(&snapshot_id, &reference, &value)?;
                Ok(HostComputerOutput::Done {})
            }
            ComputerCommand::Click { at, button, count } => {
                let at = match at {
                    ComputerTarget::Point { x, y } => Point { x, y },
                    ComputerTarget::Ref {
                        snapshot_id,
                        reference,
                    } => {
                        let element = self.accessibility.resolve(&snapshot_id, &reference)?;
                        let frame = accessibility::frame(element)?;
                        Point {
                            x: frame.x + frame.width / 2.0,
                            y: frame.y + frame.height / 2.0,
                        }
                    }
                };
                let at = input::coordinates(&at)?;
                accessibility::require_accessibility()?;
                input::click(&mut self.devices, at, &button, count)?;
                Ok(HostComputerOutput::Done {})
            }
            ComputerCommand::MouseMove { to } => {
                let to = input::coordinates(&to)?;
                accessibility::require_accessibility()?;
                input::move_mouse(&mut self.devices, to)?;
                Ok(HostComputerOutput::Done {})
            }
            ComputerCommand::Drag { from, to } => {
                let from = input::coordinates(&from)?;
                let to = input::coordinates(&to)?;
                accessibility::require_accessibility()?;
                input::drag(&mut self.devices, from, to)?;
                Ok(HostComputerOutput::Done {})
            }
            ComputerCommand::Scroll { at, dx, dy } => {
                let at = input::coordinates(&at)?;
                accessibility::require_accessibility()?;
                input::scroll(&mut self.devices, at, dx, dy)?;
                Ok(HostComputerOutput::Done {})
            }
            ComputerCommand::Type { text } => {
                accessibility::require_accessibility()?;
                input::text(&mut self.devices, &text)?;
                Ok(HostComputerOutput::Done {})
            }
            ComputerCommand::Key { chord } => {
                let keys = input::parse_chord(&chord)?;
                accessibility::require_accessibility()?;
                input::chord(&mut self.devices, &keys)?;
                Ok(HostComputerOutput::Done {})
            }
            ComputerCommand::CursorPosition {} => {
                let point = self
                    .devices
                    .mouse
                    .location()
                    .map_err(|error| DesktopError::NotStarted(error.to_string()))?;
                Ok(HostComputerOutput::Point {
                    x: point.x,
                    y: point.y,
                })
            }
        }
    }
}
