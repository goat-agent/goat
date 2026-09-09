use goat_api::{ComputerCommand, ComputerTarget, MouseButton, Point, Rect};
use goat_tool::ToolError;

use crate::action::{Action, Target, validate_app, validate_chord};

#[derive(Default)]
pub(crate) struct State {
    pub last_shot: Option<Shot>,
    pub snapshot_id: Option<String>,
}

pub(crate) struct Shot {
    pub width: u32,
    pub height: u32,
    pub display: Rect,
}

impl State {
    pub fn command(
        &self,
        action: Action,
        max_width: u32,
    ) -> Result<Option<ComputerCommand>, ToolError> {
        let command = match action {
            Action::Screenshot {} => ComputerCommand::Screenshot { max_width },
            Action::Zoom { region } => ComputerCommand::Zoom {
                region: self.shot()?.region(region)?,
                max_width,
            },
            Action::Snapshot { app } => {
                if let Some(app) = &app {
                    validate_app(app)?;
                }
                ComputerCommand::Snapshot { app }
            }
            Action::Apps {} => ComputerCommand::Apps {},
            Action::FocusApp { app } => {
                validate_app(&app)?;
                ComputerCommand::FocusApp { app }
            }
            Action::Click(target) => self.click(target, MouseButton::Left, 1)?,
            Action::DoubleClick(target) => self.click(target, MouseButton::Left, 2)?,
            Action::TripleClick(target) => self.click(target, MouseButton::Left, 3)?,
            Action::RightClick(target) => self.click(target, MouseButton::Right, 1)?,
            Action::Move { x, y } => ComputerCommand::MouseMove {
                to: self.shot()?.point(x, y)?,
            },
            Action::Drag { x, y, to_x, to_y } => {
                let shot = self.shot()?;
                ComputerCommand::Drag {
                    from: shot.point(x, y)?,
                    to: shot.point(to_x, to_y)?,
                }
            }
            Action::Scroll { x, y, dx, dy } => ComputerCommand::Scroll {
                at: self.shot()?.point(x, y)?,
                dx,
                dy,
            },
            Action::Type { text } => ComputerCommand::Type { text },
            Action::Key { keys } => {
                validate_chord(&keys)?;
                ComputerCommand::Key { chord: keys }
            }
            Action::Press {
                reference,
                snapshot_id,
            } => {
                let (snapshot_id, reference) = self.reference(reference, snapshot_id)?;
                ComputerCommand::Press {
                    snapshot_id,
                    reference,
                }
            }
            Action::SetValue {
                reference,
                snapshot_id,
                value,
            } => {
                let (snapshot_id, reference) = self.reference(reference, snapshot_id)?;
                ComputerCommand::SetValue {
                    snapshot_id,
                    reference,
                    value,
                }
            }
            Action::Wait { .. } => return Ok(None),
        };
        Ok(Some(command))
    }

    fn shot(&self) -> Result<&Shot, ToolError> {
        self.last_shot.as_ref().ok_or_else(|| {
            ToolError::invalid_input(
                "take a screenshot first; coordinates are pixels of the latest screenshot",
            )
        })
    }

    fn click(
        &self,
        target: Target,
        button: MouseButton,
        count: u8,
    ) -> Result<ComputerCommand, ToolError> {
        let at = match (target.x, target.y, target.reference, target.snapshot_id) {
            (Some(x), Some(y), None, None) => {
                let Point { x, y } = self.shot()?.point(x, y)?;
                ComputerTarget::Point { x, y }
            }
            (None, None, Some(reference), snapshot_id) => {
                let (snapshot_id, reference) = self.reference(reference, snapshot_id)?;
                ComputerTarget::Ref {
                    snapshot_id,
                    reference,
                }
            }
            _ => {
                return Err(ToolError::invalid_input(
                    "provide either x and y, or ref with an optional snapshot_id; do not mix them",
                ));
            }
        };
        Ok(ComputerCommand::Click { at, button, count })
    }

    fn reference(
        &self,
        mut reference: String,
        snapshot_id: Option<String>,
    ) -> Result<(String, String), ToolError> {
        let (qualified, element) = reference
            .split_once(':')
            .map_or((None, reference.as_str()), |(snapshot, element)| {
                (Some(snapshot), element)
            });
        if !identifier(element, 'e')
            || qualified.is_some_and(|id| !identifier(id, 's'))
            || snapshot_id
                .as_deref()
                .is_some_and(|id| !identifier(id, 's'))
        {
            return Err(ToolError::invalid_input(
                "ref must be eN or sN:eN; snapshot_id must be sN",
            ));
        }
        if let (Some(qualified), Some(explicit)) = (qualified, snapshot_id.as_deref())
            && qualified != explicit
        {
            return Err(ToolError::invalid_input(
                "ref and snapshot_id name different snapshots",
            ));
        }
        let current = self.snapshot_id.as_deref().ok_or_else(|| {
            ToolError::invalid_input("take a snapshot first; refs come from the latest snapshot")
        })?;
        if qualified
            .or(snapshot_id.as_deref())
            .is_some_and(|id| id != current)
        {
            return Err(ToolError::invalid_input(format!(
                "refs come from the latest snapshot ({current}); take a new snapshot"
            )));
        }
        if let Some(qualified) = qualified {
            let end = qualified.len() + 1;
            reference.replace_range(..end, "");
        }
        Ok((snapshot_id.unwrap_or_else(|| current.to_owned()), reference))
    }
}

impl Shot {
    pub fn point(&self, x: f64, y: f64) -> Result<Point, ToolError> {
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.0
            || y < 0.0
            || x >= f64::from(self.width)
            || y >= f64::from(self.height)
        {
            return Err(ToolError::invalid_input(
                "coordinates must be inside the latest screenshot",
            ));
        }
        Ok(self.map(x, y))
    }

    fn map(&self, x: f64, y: f64) -> Point {
        Point {
            x: self.display.x + x / f64::from(self.width) * self.display.width,
            y: self.display.y + y / f64::from(self.height) * self.display.height,
        }
    }

    fn region(&self, [x0, y0, x1, y1]: [f64; 4]) -> Result<Rect, ToolError> {
        if ![x0, y0, x1, y1].into_iter().all(f64::is_finite)
            || x0 < 0.0
            || y0 < 0.0
            || x1 <= x0
            || y1 <= y0
            || x1 > f64::from(self.width)
            || y1 > f64::from(self.height)
        {
            return Err(ToolError::invalid_input(
                "region must be [x0,y0,x1,y1] with positive area inside the latest screenshot",
            ));
        }
        let Point { x, y } = self.map(x0, y0);
        Ok(Rect {
            x,
            y,
            width: (x1 - x0) / f64::from(self.width) * self.display.width,
            height: (y1 - y0) / f64::from(self.height) * self.display.height,
        })
    }
}

fn identifier(value: &str, prefix: char) -> bool {
    value.strip_prefix(prefix).is_some_and(|digits| {
        !digits.is_empty()
            && digits.bytes().all(|byte| byte.is_ascii_digit())
            && digits.bytes().any(|byte| byte != b'0')
    })
}
