use std::collections::VecDeque;
use std::io;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Console::{
    GetConsoleMode, GetStdHandle, INPUT_RECORD, KEY_EVENT, KEY_EVENT_RECORD, LEFT_ALT_PRESSED,
    LEFT_CTRL_PRESSED, RIGHT_ALT_PRESSED, RIGHT_CTRL_PRESSED, ReadConsoleInputW, STD_INPUT_HANDLE,
};

use crate::shape_lab_cli::ShapeLabInput;

const VK_BACK: u16 = 0x08;
const VK_TAB: u16 = 0x09;
const VK_ESCAPE: u16 = 0x1b;
const VK_RETURN: u16 = 0x0d;
const VK_0: u16 = 0x30;
const VK_9: u16 = 0x39;
const VK_NUMPAD0: u16 = 0x60;
const VK_NUMPAD9: u16 = 0x69;
const VK_A: u16 = 0x41;
const VK_Z: u16 = 0x5a;

pub struct WindowsShapeKeyReader {
    input: HANDLE,
    pending: VecDeque<ShapeLabInput>,
}

impl WindowsShapeKeyReader {
    pub fn open() -> io::Result<Self> {
        // SAFETY: GetStdHandle returns a borrowed process handle; GetConsoleMode only validates
        // that the handle is attached to a console. No console mode is modified.
        let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) }.map_err(windows_error)?;
        let mut mode = Default::default();
        // SAFETY: `mode` is a valid writable CONSOLE_MODE and `input` was returned above.
        unsafe { GetConsoleMode(input, &mut mode) }.map_err(windows_error)?;
        Ok(Self {
            input,
            pending: VecDeque::new(),
        })
    }

    pub fn read(&mut self) -> io::Result<ShapeLabInput> {
        if let Some(input) = self.pending.pop_front() {
            return Ok(input);
        }

        loop {
            let mut records = [INPUT_RECORD::default(); 16];
            let mut count = 0u32;
            // SAFETY: the record slice and count pointer remain valid for the duration of the
            // blocking call; the console handle was validated by `open`.
            unsafe { ReadConsoleInputW(self.input, &mut records, &mut count) }
                .map_err(windows_error)?;

            for record in records.into_iter().take(count as usize) {
                if record.EventType != KEY_EVENT as u16 {
                    continue;
                }
                // SAFETY: EventType identifies the active INPUT_RECORD union member.
                let event = unsafe { record.Event.KeyEvent };
                let Some(input) = decode_key_event(&event) else {
                    continue;
                };
                let repeat =
                    if matches!(input, ShapeLabInput::Letters(_) | ShapeLabInput::Backspace) {
                        usize::from(event.wRepeatCount.max(1)).min(32)
                    } else {
                        1
                    };
                self.pending.extend(std::iter::repeat_n(input, repeat));
            }

            if let Some(input) = self.pending.pop_front() {
                return Ok(input);
            }
        }
    }
}

fn decode_key_event(event: &KEY_EVENT_RECORD) -> Option<ShapeLabInput> {
    if !event.bKeyDown.as_bool() {
        return None;
    }
    let modified = LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED | LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED;
    if event.dwControlKeyState & modified != 0 {
        return None;
    }

    let key = event.wVirtualKeyCode;
    match key {
        VK_BACK => Some(ShapeLabInput::Backspace),
        VK_TAB => Some(ShapeLabInput::EnterTab),
        VK_ESCAPE => Some(ShapeLabInput::LeaveTab),
        VK_RETURN => Some(ShapeLabInput::Skip),
        VK_A..=VK_Z => Some(ShapeLabInput::Letters(
            char::from(b'a' + u8::try_from(key - VK_A).expect("A-Z offset fits u8")).to_string(),
        )),
        VK_0..=VK_9 => Some(digit_selection(key - VK_0)),
        VK_NUMPAD0..=VK_NUMPAD9 => Some(digit_selection(key - VK_NUMPAD0)),
        _ => None,
    }
}

fn digit_selection(digit: u16) -> ShapeLabInput {
    ShapeLabInput::Select(if digit == 0 { 10 } else { usize::from(digit) })
}

fn windows_error(error: windows_core::Error) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use windows::Win32::System::Console::{KEY_EVENT_RECORD, LEFT_CTRL_PRESSED};
    use windows_core::BOOL;

    use super::{VK_A, VK_BACK, VK_RETURN, VK_TAB, decode_key_event};
    use crate::shape_lab_cli::ShapeLabInput;

    fn pressed(key: u16) -> KEY_EVENT_RECORD {
        KEY_EVENT_RECORD {
            bKeyDown: BOOL(1),
            wRepeatCount: 1,
            wVirtualKeyCode: key,
            ..Default::default()
        }
    }

    #[test]
    fn decodes_the_direct_session_keys() {
        assert_eq!(
            decode_key_event(&pressed(VK_TAB)),
            Some(ShapeLabInput::EnterTab)
        );
        assert_eq!(
            decode_key_event(&pressed(VK_A + u16::from(b'h' - b'a'))),
            Some(ShapeLabInput::Letters("h".to_owned()))
        );
        assert_eq!(
            decode_key_event(&pressed(VK_BACK)),
            Some(ShapeLabInput::Backspace)
        );
        assert_eq!(
            decode_key_event(&pressed(VK_RETURN)),
            Some(ShapeLabInput::Skip)
        );
        assert_eq!(
            decode_key_event(&pressed(b'3'.into())),
            Some(ShapeLabInput::Select(3))
        );
        assert_eq!(
            decode_key_event(&pressed(b'0'.into())),
            Some(ShapeLabInput::Select(10))
        );
    }

    #[test]
    fn ignores_key_releases_and_control_shortcuts() {
        let h = VK_A + u16::from(b'h' - b'a');
        let mut released = pressed(h);
        released.bKeyDown = BOOL(0);
        assert_eq!(decode_key_event(&released), None);

        let mut control_h = pressed(h);
        control_h.dwControlKeyState = LEFT_CTRL_PRESSED;
        assert_eq!(decode_key_event(&control_h), None);
    }
}
