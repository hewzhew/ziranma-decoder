use std::collections::VecDeque;
use std::io;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Console::{
    GetConsoleMode, GetStdHandle, INPUT_RECORD, KEY_EVENT, KEY_EVENT_RECORD, LEFT_ALT_PRESSED,
    LEFT_CTRL_PRESSED, RIGHT_ALT_PRESSED, RIGHT_CTRL_PRESSED, ReadConsoleInputW, SHIFT_PRESSED,
    STD_INPUT_HANDLE,
};

use crate::typing_lab_cli::TypingLabInput;

const VK_BACK: u16 = 0x08;
const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0d;
const VK_ESCAPE: u16 = 0x1b;
const VK_SPACE: u16 = 0x20;
const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_F2: u16 = 0x71;
const VK_OEM_PLUS: u16 = 0xbb;
const VK_OEM_MINUS: u16 = 0xbd;
const VK_0: u16 = 0x30;
const VK_9: u16 = 0x39;
const VK_A: u16 = 0x41;
const VK_Z: u16 = 0x5a;
const VK_NUMPAD0: u16 = 0x60;
const VK_NUMPAD9: u16 = 0x69;

pub struct WindowsTypingKeyReader {
    input: HANDLE,
    pending: VecDeque<TypingLabInput>,
}

impl WindowsTypingKeyReader {
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

    pub fn read(&mut self) -> io::Result<TypingLabInput> {
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
                let repeat = if matches!(
                    input,
                    TypingLabInput::Letters(_)
                        | TypingLabInput::Backspace
                        | TypingLabInput::PreviousPage
                        | TypingLabInput::NextPage
                ) {
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

fn decode_key_event(event: &KEY_EVENT_RECORD) -> Option<TypingLabInput> {
    if !event.bKeyDown.as_bool() {
        return None;
    }
    let modified = LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED | LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED;
    if event.dwControlKeyState & modified != 0 {
        return None;
    }

    let key = event.wVirtualKeyCode;
    if key == VK_TAB && event.dwControlKeyState & SHIFT_PRESSED != 0 {
        return Some(TypingLabInput::EnterRecovery);
    }
    match key {
        VK_BACK => Some(TypingLabInput::Backspace),
        VK_TAB => Some(TypingLabInput::EnterTab),
        VK_RETURN | VK_SPACE => Some(TypingLabInput::Confirm),
        VK_ESCAPE => Some(TypingLabInput::Escape),
        VK_PRIOR => Some(TypingLabInput::PreviousPage),
        VK_NEXT => Some(TypingLabInput::NextPage),
        VK_F2 => Some(TypingLabInput::Mark),
        VK_OEM_MINUS => Some(TypingLabInput::PreviousPage),
        VK_OEM_PLUS => Some(TypingLabInput::NextPage),
        VK_A..=VK_Z => Some(TypingLabInput::Letters(
            char::from(b'a' + u8::try_from(key - VK_A).expect("A-Z offset fits u8")).to_string(),
        )),
        VK_0..=VK_9 => Some(digit_selection(key - VK_0)),
        VK_NUMPAD0..=VK_NUMPAD9 => Some(digit_selection(key - VK_NUMPAD0)),
        _ => None,
    }
}

fn digit_selection(digit: u16) -> TypingLabInput {
    TypingLabInput::Select(if digit == 0 { 10 } else { usize::from(digit) })
}

fn windows_error(error: windows_core::Error) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use windows::Win32::System::Console::{KEY_EVENT_RECORD, LEFT_CTRL_PRESSED, SHIFT_PRESSED};
    use windows_core::BOOL;

    use super::{
        VK_A, VK_BACK, VK_ESCAPE, VK_F2, VK_NEXT, VK_OEM_MINUS, VK_OEM_PLUS, VK_PRIOR, VK_RETURN,
        VK_SPACE, VK_TAB, decode_key_event,
    };
    use crate::typing_lab_cli::TypingLabInput;

    fn pressed(key: u16) -> KEY_EVENT_RECORD {
        KEY_EVENT_RECORD {
            bKeyDown: BOOL(1),
            wRepeatCount: 1,
            wVirtualKeyCode: key,
            ..Default::default()
        }
    }

    #[test]
    fn decodes_typing_controls_without_stealing_q() {
        assert_eq!(
            decode_key_event(&pressed(VK_TAB)),
            Some(TypingLabInput::EnterTab)
        );
        let mut shifted_tab = pressed(VK_TAB);
        shifted_tab.dwControlKeyState = SHIFT_PRESSED;
        assert_eq!(
            decode_key_event(&shifted_tab),
            Some(TypingLabInput::EnterRecovery)
        );
        assert_eq!(
            decode_key_event(&pressed(VK_SPACE)),
            Some(TypingLabInput::Confirm)
        );
        assert_eq!(
            decode_key_event(&pressed(VK_RETURN)),
            Some(TypingLabInput::Confirm)
        );
        assert_eq!(
            decode_key_event(&pressed(VK_ESCAPE)),
            Some(TypingLabInput::Escape)
        );
        assert_eq!(
            decode_key_event(&pressed(VK_BACK)),
            Some(TypingLabInput::Backspace)
        );
        assert_eq!(
            decode_key_event(&pressed(VK_PRIOR)),
            Some(TypingLabInput::PreviousPage)
        );
        assert_eq!(
            decode_key_event(&pressed(VK_NEXT)),
            Some(TypingLabInput::NextPage)
        );
        assert_eq!(
            decode_key_event(&pressed(VK_F2)),
            Some(TypingLabInput::Mark)
        );
        assert_eq!(
            decode_key_event(&pressed(VK_OEM_MINUS)),
            Some(TypingLabInput::PreviousPage)
        );
        assert_eq!(
            decode_key_event(&pressed(VK_OEM_PLUS)),
            Some(TypingLabInput::NextPage)
        );
        let mut shifted_plus = pressed(VK_OEM_PLUS);
        shifted_plus.dwControlKeyState = SHIFT_PRESSED;
        assert_eq!(
            decode_key_event(&shifted_plus),
            Some(TypingLabInput::NextPage)
        );
        assert_eq!(
            decode_key_event(&pressed(VK_A + u16::from(b'q' - b'a'))),
            Some(TypingLabInput::Letters("q".to_owned()))
        );
        assert_eq!(
            decode_key_event(&pressed(b'3'.into())),
            Some(TypingLabInput::Select(3))
        );
        assert_eq!(
            decode_key_event(&pressed(b'0'.into())),
            Some(TypingLabInput::Select(10))
        );
    }

    #[test]
    fn ignores_key_releases_and_control_shortcuts() {
        let q = VK_A + u16::from(b'q' - b'a');
        let mut released = pressed(q);
        released.bKeyDown = BOOL(0);
        assert_eq!(decode_key_event(&released), None);

        let mut control_q = pressed(q);
        control_q.dwControlKeyState = LEFT_CTRL_PRESSED;
        assert_eq!(decode_key_event(&control_q), None);
    }
}
