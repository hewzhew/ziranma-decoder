//! Text-free control words for an explicit, current-user TSF wish command.
//!
//! Only an action, a bounded sequence number, and a redacted acknowledgement
//! cross process boundaries. Input codes, candidates, notes, paths, and
//! snapshots never enter the global compartment.

const KIND_SHIFT: u32 = 28;
const SEQUENCE_MASK: u32 = (1 << KIND_SHIFT) - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum WishCommand {
    Start = 1,
    SaveRecent = 2,
    Stop = 3,
    ClearStopped = 4,
}

impl WishCommand {
    fn from_kind(kind: u32) -> Option<Self> {
        match kind {
            1 => Some(Self::Start),
            2 => Some(Self::SaveRecent),
            3 => Some(Self::Stop),
            4 => Some(Self::ClearStopped),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WishCommandWord {
    command: WishCommand,
    sequence: u32,
}

impl WishCommandWord {
    pub fn next(previous: Option<Self>, command: WishCommand) -> Self {
        let sequence = previous
            .map_or(1, |previous| {
                previous.sequence.saturating_add(1) & SEQUENCE_MASK
            })
            .max(1);
        Self { command, sequence }
    }

    pub fn parse(raw: u32) -> Option<Self> {
        let sequence = raw & SEQUENCE_MASK;
        let command = WishCommand::from_kind(raw >> KIND_SHIFT)?;
        (sequence != 0).then_some(Self { command, sequence })
    }

    pub fn raw(self) -> u32 {
        (self.command as u32) << KIND_SHIFT | self.sequence
    }

    pub fn command(self) -> WishCommand {
        self.command
    }

    pub fn sequence(self) -> u32 {
        self.sequence
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u32)]
pub enum WishCommandAckStatus {
    NoChange = 1,
    Failed = 2,
    Applied = 3,
}

impl WishCommandAckStatus {
    fn from_kind(kind: u32) -> Option<Self> {
        match kind {
            1 => Some(Self::NoChange),
            2 => Some(Self::Failed),
            3 => Some(Self::Applied),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WishCommandAck {
    status: WishCommandAckStatus,
    sequence: u32,
}

impl WishCommandAck {
    pub fn new(sequence: u32, status: WishCommandAckStatus) -> Option<Self> {
        (sequence != 0 && sequence <= SEQUENCE_MASK).then_some(Self { status, sequence })
    }

    pub fn parse(raw: u32) -> Option<Self> {
        let sequence = raw & SEQUENCE_MASK;
        let status = WishCommandAckStatus::from_kind(raw >> KIND_SHIFT)?;
        Self::new(sequence, status)
    }

    pub fn raw(self) -> u32 {
        (self.status as u32) << KIND_SHIFT | self.sequence
    }

    pub fn status(self) -> WishCommandAckStatus {
        self.status
    }

    pub fn sequence(self) -> u32 {
        self.sequence
    }
}

#[cfg(windows)]
use windows::core::GUID;

#[cfg(windows)]
pub const WISH_COMMAND_COMPARTMENT_GUID: GUID =
    GUID::from_u128(0xcbd9c825_30d4_43c4_a609_294e6c946565);
#[cfg(windows)]
pub const WISH_ACK_COMPARTMENT_GUID: GUID = GUID::from_u128(0x3528ddb7_2bb2_4fb3_aa3c_504af00c5d1d);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WishCommandDispatchError {
    UnsupportedPlatform,
    ComInitialization,
    ThreadManager,
    GlobalCompartment,
    Publish,
}

impl std::fmt::Display for WishCommandDispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedPlatform => "wish commands are supported only on Windows",
            Self::ComInitialization => "cannot initialize the local COM apartment",
            Self::ThreadManager => "cannot activate a local TSF thread manager",
            Self::GlobalCompartment => "cannot access the current-user TSF command compartment",
            Self::Publish => "cannot publish the local TSF wish command",
        })
    }
}

impl std::error::Error for WishCommandDispatchError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WishCommandDispatchReceipt {
    command: WishCommand,
    sequence: u32,
    acknowledgement: Option<WishCommandAckStatus>,
}

impl WishCommandDispatchReceipt {
    pub fn command(self) -> WishCommand {
        self.command
    }

    pub fn sequence(self) -> u32 {
        self.sequence
    }

    pub fn acknowledgement(self) -> Option<WishCommandAckStatus> {
        self.acknowledgement
    }
}

#[cfg(windows)]
pub fn dispatch_wish_command(
    command: WishCommand,
) -> Result<WishCommandDispatchReceipt, WishCommandDispatchError> {
    use std::thread;
    use std::time::{Duration, Instant};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::TextServices::{CLSID_TF_ThreadMgr, ITfThreadMgr};
    use windows::core::IUnknown;

    // SAFETY: this command owns the matching CoUninitialize below. The helper
    // is called on a fresh CLI/tray command thread, never from a TSF callback.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(|_| WishCommandDispatchError::ComInitialization)?;
    let result = (|| {
        // SAFETY: the system TSF thread manager is an in-process COM service.
        let manager: ITfThreadMgr = unsafe {
            CoCreateInstance(&CLSID_TF_ThreadMgr, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
        }
        .map_err(|_| WishCommandDispatchError::ThreadManager)?;
        // SAFETY: the returned client id is released by Deactivate below.
        let client_id =
            unsafe { manager.Activate() }.map_err(|_| WishCommandDispatchError::ThreadManager)?;
        let dispatched = (|| {
            // SAFETY: this obtains the current interactive user's global TSF
            // compartment manager. No document or input content is accessed.
            let compartments = unsafe { manager.GetGlobalCompartment() }
                .map_err(|_| WishCommandDispatchError::GlobalCompartment)?;
            // SAFETY: both project GUIDs carry only bounded integer words.
            let command_compartment =
                unsafe { compartments.GetCompartment(&WISH_COMMAND_COMPARTMENT_GUID) }
                    .map_err(|_| WishCommandDispatchError::GlobalCompartment)?;
            let ack_compartment =
                unsafe { compartments.GetCompartment(&WISH_ACK_COMPARTMENT_GUID) }
                    .map_err(|_| WishCommandDispatchError::GlobalCompartment)?;

            let previous = unsafe { command_compartment.GetValue() }
                .ok()
                .and_then(|value| i32::try_from(&value).ok())
                .and_then(|value| u32::try_from(value).ok())
                .and_then(WishCommandWord::parse);
            let word = WishCommandWord::next(previous, command);
            let empty_ack = VARIANT::from(0_i32);
            // SAFETY: clearing the acknowledgement prevents a stale response
            // from being mistaken for this new sequence.
            unsafe { ack_compartment.SetValue(client_id, &empty_ack) }
                .map_err(|_| WishCommandDispatchError::Publish)?;
            let value = VARIANT::from(
                i32::try_from(word.raw()).expect("wish command words always fit in VT_I4"),
            );
            // SAFETY: the VARIANT remains live for this synchronous SetValue.
            unsafe { command_compartment.SetValue(client_id, &value) }
                .map_err(|_| WishCommandDispatchError::Publish)?;

            let deadline = Instant::now() + Duration::from_millis(600);
            let acknowledgement = loop {
                let ack = unsafe { ack_compartment.GetValue() }
                    .ok()
                    .and_then(|value| i32::try_from(&value).ok())
                    .and_then(|value| u32::try_from(value).ok())
                    .and_then(WishCommandAck::parse)
                    .filter(|ack| ack.sequence() == word.sequence());
                if let Some(ack) = ack {
                    break Some(ack.status());
                }
                if Instant::now() >= deadline {
                    break None;
                }
                thread::sleep(Duration::from_millis(10));
            };
            Ok(WishCommandDispatchReceipt {
                command,
                sequence: word.sequence(),
                acknowledgement,
            })
        })();
        // SAFETY: balances this helper's successful Activate call.
        let _ = unsafe { manager.Deactivate() };
        dispatched
    })();
    // SAFETY: balances the successful CoInitializeEx above.
    unsafe { CoUninitialize() };
    result
}

#[cfg(not(windows))]
pub fn dispatch_wish_command(
    _command: WishCommand,
) -> Result<WishCommandDispatchReceipt, WishCommandDispatchError> {
    Err(WishCommandDispatchError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_words_are_bounded_and_round_trip() {
        let first = WishCommandWord::next(None, WishCommand::Start);
        assert_eq!(first.sequence(), 1);
        assert_eq!(WishCommandWord::parse(first.raw()), Some(first));
        let second = WishCommandWord::next(Some(first), WishCommand::SaveRecent);
        assert_eq!(second.sequence(), 2);
        assert_eq!(WishCommandWord::parse(second.raw()), Some(second));
        assert!(WishCommandWord::parse(0).is_none());
        assert!(WishCommandWord::parse(0xf000_0001).is_none());
    }

    #[test]
    fn acknowledgement_priority_and_sequence_are_explicit() {
        assert!(WishCommandAckStatus::Applied > WishCommandAckStatus::Failed);
        assert!(WishCommandAckStatus::Failed > WishCommandAckStatus::NoChange);
        let ack = WishCommandAck::new(17, WishCommandAckStatus::Applied).unwrap();
        assert_eq!(WishCommandAck::parse(ack.raw()), Some(ack));
        assert!(WishCommandAck::new(0, WishCommandAckStatus::Applied).is_none());
    }
}
