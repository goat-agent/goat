use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

use axuielement::{AXError, AXUIElement};
use goat_api::{AxNode, HostComputerOutput, Rect, RunningApp};

use crate::DesktopError;

static SNAPSHOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MAX_NODES: usize = 400;
const MAX_DEPTH: u8 = 24;

#[derive(Default)]
pub(super) struct Accessibility {
    snapshot_id: String,
    refs: HashMap<String, AXUIElement>,
}

impl Accessibility {
    pub(super) fn snapshot(
        &mut self,
        name: Option<&str>,
    ) -> Result<HostComputerOutput, DesktopError> {
        require_accessibility()?;
        let apps = goat_computer_host_apps::running_apps();
        let (app, element) = if let Some(name) = name {
            let app = find_app(apps, name)?;
            let element = AXUIElement::from_pid(app.pid).ok_or_else(|| {
                DesktopError::NotStarted("application has no accessibility element".into())
            })?;
            (app, element)
        } else {
            let element = axuielement::system_wide()
                .ok_or_else(|| {
                    DesktopError::NotStarted("system accessibility is unavailable".into())
                })?
                .focused_application()
                .map_err(read_error)?
                .ok_or_else(|| DesktopError::NotStarted("no application is focused".into()))?;
            let pid = element.pid().map_err(read_error)?;
            let app = apps.into_iter().find(|app| app.pid == pid).ok_or_else(|| {
                DesktopError::NotStarted("focused application is no longer running".into())
            })?;
            (app, element)
        };
        element.set_timeout(1.0).map_err(read_error)?;
        let window = element
            .element_attribute("AXFocusedWindow")
            .map_err(read_error)?
            .ok_or_else(|| DesktopError::NotStarted("application has no focused window".into()))?;
        let bounds = frame(&window)?;
        let window_title = window.string_attribute("AXTitle").map_err(read_error)?;
        let mut refs = HashMap::new();
        let mut nodes = Vec::new();
        let mut remaining = MAX_NODES;
        walk(&window, 0, &bounds, &mut remaining, &mut nodes, &mut refs)?;
        let sequence = SNAPSHOT_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
        self.snapshot_id = format!("s{sequence}");
        self.refs = refs;
        Ok(HostComputerOutput::Snapshot {
            snapshot_id: self.snapshot_id.clone(),
            app,
            window_title,
            window: bounds,
            nodes,
        })
    }

    pub(super) fn resolve(
        &self,
        snapshot_id: &str,
        reference: &str,
    ) -> Result<&AXUIElement, DesktopError> {
        if self.snapshot_id.is_empty() || snapshot_id != self.snapshot_id {
            let current = if self.snapshot_id.is_empty() {
                "none"
            } else {
                &self.snapshot_id
            };
            return Err(DesktopError::NotStarted(format!(
                "refs come from the latest snapshot ({current}); take a new snapshot"
            )));
        }
        self.refs.get(reference).ok_or_else(|| {
            DesktopError::NotStarted(format!(
                "unknown accessibility ref {reference:?}; take a new snapshot"
            ))
        })
    }

    pub(super) fn press(&self, snapshot_id: &str, reference: &str) -> Result<(), DesktopError> {
        let element = self.resolve(snapshot_id, reference)?;
        require_accessibility()?;
        if !element
            .action_names()
            .map_err(read_error)?
            .iter()
            .any(|name| name == "AXPress")
        {
            return Err(DesktopError::NotStarted(format!(
                "ref {reference:?} does not support AXPress"
            )));
        }
        element
            .perform_action("AXPress")
            .map_err(|error| mutation_error(&error))
    }

    pub(super) fn set_value(
        &self,
        snapshot_id: &str,
        reference: &str,
        value: &str,
    ) -> Result<(), DesktopError> {
        let element = self.resolve(snapshot_id, reference)?;
        require_accessibility()?;
        if !element
            .is_attribute_settable("AXValue")
            .map_err(read_error)?
        {
            return Err(DesktopError::NotStarted(format!(
                "ref {reference:?} has no writable AXValue"
            )));
        }
        element
            .set_string_attribute("AXValue", value)
            .map_err(|error| mutation_error(&error))
    }
}

pub(super) fn find_app(apps: Vec<RunningApp>, name: &str) -> Result<RunningApp, DesktopError> {
    apps.into_iter()
        .find(|app| app.bundle_id.as_deref() == Some(name) || app.name == name)
        .ok_or_else(|| DesktopError::NotStarted(format!("no running application named {name:?}")))
}

pub(super) fn require_accessibility() -> Result<(), DesktopError> {
    if axuielement::is_process_trusted() {
        Ok(())
    } else {
        Err(DesktopError::NotStarted(
            "Accessibility permission is not granted".into(),
        ))
    }
}

pub(super) fn frame(element: &AXUIElement) -> Result<Rect, DesktopError> {
    let bounds = if let Ok(Some(rect)) = element.rect_attribute("AXFrame") {
        Rect {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        }
    } else {
        let position = element
            .point_attribute("AXPosition")
            .map_err(read_error)?
            .ok_or_else(|| {
                DesktopError::NotStarted("accessibility element has no position".into())
            })?;
        let size = element
            .size_attribute("AXSize")
            .map_err(read_error)?
            .ok_or_else(|| DesktopError::NotStarted("accessibility element has no size".into()))?;
        Rect {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        }
    };
    if [bounds.x, bounds.y, bounds.width, bounds.height]
        .into_iter()
        .all(f64::is_finite)
        && bounds.width > 0.0
        && bounds.height > 0.0
        && (bounds.x + bounds.width).is_finite()
        && (bounds.y + bounds.height).is_finite()
    {
        Ok(bounds)
    } else {
        Err(DesktopError::NotStarted(
            "accessibility element has an empty or invalid frame".into(),
        ))
    }
}

fn walk(
    element: &AXUIElement,
    depth: u8,
    window: &Rect,
    remaining: &mut usize,
    nodes: &mut Vec<AxNode>,
    refs: &mut HashMap<String, AXUIElement>,
) -> Result<(), DesktopError> {
    if *remaining == 0 {
        return Ok(());
    }
    *remaining -= 1;
    let role = element
        .string_attribute("AXRole")
        .ok()
        .flatten()
        .unwrap_or_default();
    if kept_role(&role)
        && let Ok(bounds) = frame(element)
        && intersects(&bounds, window)
    {
        let reference = format!("e{}", nodes.len() + 1);
        let value = element
            .attribute("AXValue")
            .ok()
            .flatten()
            .and_then(|value| {
                value
                    .as_string()
                    .or_else(|| value.as_bool().map(|value| value.to_string()))
                    .or_else(|| value.as_i64().map(|value| value.to_string()))
                    .or_else(|| value.as_f64().map(|value| value.to_string()))
            });
        nodes.push(AxNode {
            reference: reference.clone(),
            role,
            title: element.string_attribute("AXTitle").ok().flatten(),
            value,
            depth,
            frame: bounds,
            enabled: element
                .bool_attribute("AXEnabled")
                .ok()
                .flatten()
                .unwrap_or(false),
            focused: element
                .bool_attribute("AXFocused")
                .ok()
                .flatten()
                .unwrap_or(false),
            actions: element.action_names().unwrap_or_default(),
        });
        refs.insert(reference, element.clone());
    }
    if depth < MAX_DEPTH && *remaining > 0 {
        let count = match element.attribute_value_count("AXChildren") {
            Ok(count) => count.min(*remaining),
            Err(
                AXError::AttributeUnsupported(_) | AXError::NoValue | AXError::InvalidUIElement,
            ) => 0,
            Err(error) => return Err(read_error(error)),
        };
        let children = if count == 0 {
            Vec::new()
        } else {
            element
                .element_array_attribute_range("AXChildren", 0, count)
                .map_err(read_error)?
        };
        for child in children {
            walk(&child, depth + 1, window, remaining, nodes, refs)?;
            if *remaining == 0 {
                break;
            }
        }
    }
    Ok(())
}

fn intersects(left: &Rect, right: &Rect) -> bool {
    left.x < right.x + right.width
        && right.x < left.x + left.width
        && left.y < right.y + right.height
        && right.y < left.y + left.height
}

fn kept_role(role: &str) -> bool {
    matches!(
        role,
        "AXButton"
            | "AXTextField"
            | "AXTextArea"
            | "AXSecureTextField"
            | "AXCheckBox"
            | "AXRadioButton"
            | "AXPopUpButton"
            | "AXMenuButton"
            | "AXMenuItem"
            | "AXMenuBarItem"
            | "AXLink"
            | "AXStaticText"
            | "AXTab"
            | "AXRow"
            | "AXCell"
            | "AXSlider"
            | "AXComboBox"
            | "AXImage"
            | "AXDisclosureTriangle"
            | "AXToolbar"
            | "AXOutline"
            | "AXTable"
            | "AXList"
            | "AXScrollArea"
            | "AXWebArea"
    )
}

fn read_error(error: impl std::fmt::Display) -> DesktopError {
    DesktopError::NotStarted(error.to_string())
}

fn mutation_error(error: &AXError) -> DesktopError {
    match error {
        AXError::IllegalArgument(_)
        | AXError::InvalidUIElement
        | AXError::AttributeUnsupported(_)
        | AXError::ActionUnsupported(_)
        | AXError::NotImplemented
        | AXError::APIDisabled
        | AXError::NoValue => DesktopError::NotStarted(error.to_string()),
        _ => DesktopError::OutcomeUnknown(error.to_string()),
    }
}
