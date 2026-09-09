use core_graphics::{
    event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField},
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGPoint,
};
use goat_api::{MouseButton, Point};

pub struct NativeMouse {
    source: CGEventSource,
    position: Option<CGPoint>,
    pressed: Option<(CGMouseButton, u8)>,
}

impl NativeMouse {
    pub fn new() -> Result<Self, crate::NativeError> {
        let source = CGEventSource::new(CGEventSourceStateID::Private)
            .map_err(|()| crate::NativeError::EventSource)?;
        Ok(Self {
            source,
            position: None,
            pressed: None,
        })
    }

    pub fn location(&self) -> Result<Point, crate::NativeError> {
        let event = CGEvent::new(self.source.clone()).map_err(|()| crate::NativeError::Cursor)?;
        let point = event.location();
        Ok(Point {
            x: point.x,
            y: point.y,
        })
    }

    pub fn move_to(&mut self, x: i32, y: i32) -> Result<(), crate::NativeError> {
        let position = CGPoint::new(f64::from(x), f64::from(y));
        let (kind, button) = match self.pressed {
            Some((CGMouseButton::Left, _)) => (CGEventType::LeftMouseDragged, CGMouseButton::Left),
            Some((CGMouseButton::Right, _)) => {
                (CGEventType::RightMouseDragged, CGMouseButton::Right)
            }
            Some((CGMouseButton::Center, _)) => {
                (CGEventType::OtherMouseDragged, CGMouseButton::Center)
            }
            None => (CGEventType::MouseMoved, CGMouseButton::Left),
        };
        self.post(kind, position, button, 0)?;
        self.position = Some(position);
        Ok(())
    }

    pub fn press(&mut self, button: &MouseButton, count: u8) -> Result<(), crate::NativeError> {
        let (kind, button) = match button {
            MouseButton::Left => (CGEventType::LeftMouseDown, CGMouseButton::Left),
            MouseButton::Right => (CGEventType::RightMouseDown, CGMouseButton::Right),
            MouseButton::Middle => (CGEventType::OtherMouseDown, CGMouseButton::Center),
        };
        let position = self.event_position()?;
        self.post(kind, position, button, count)?;
        self.pressed = Some((button, count));
        Ok(())
    }

    pub fn release(&mut self) -> Result<(), crate::NativeError> {
        if let Some((button, count)) = self.pressed {
            let kind = match button {
                CGMouseButton::Left => CGEventType::LeftMouseUp,
                CGMouseButton::Right => CGEventType::RightMouseUp,
                CGMouseButton::Center => CGEventType::OtherMouseUp,
            };
            self.post(kind, self.event_position()?, button, count)?;
            self.pressed = None;
        }
        Ok(())
    }

    fn event_position(&self) -> Result<CGPoint, crate::NativeError> {
        if let Some(position) = self.position {
            Ok(position)
        } else {
            let point = self.location()?;
            Ok(CGPoint::new(point.x, point.y))
        }
    }

    fn post(
        &self,
        kind: CGEventType,
        position: CGPoint,
        button: CGMouseButton,
        count: u8,
    ) -> Result<(), crate::NativeError> {
        let event = CGEvent::new_mouse_event(self.source.clone(), kind, position, button)
            .map_err(|()| crate::NativeError::MouseEvent)?;
        event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i64::from(count));
        event.set_flags(CGEventFlags::CGEventFlagNonCoalesced);
        event.post(CGEventTapLocation::HID);
        Ok(())
    }
}

impl Drop for NativeMouse {
    fn drop(&mut self) {
        let _ = self.release();
    }
}
