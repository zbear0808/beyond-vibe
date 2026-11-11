use std::time::Instant;

#[derive(Debug, Clone)]
pub enum MidiInputMessage {
    NoteOn { channel: u8, note: u8, velocity: u8 },
    NoteOff { channel: u8, note: u8 },
    ControlChange { channel: u8, controller: u8, value: u8 },
}

impl MidiInputMessage {
    /// 0-based MIDI channel (0..=15)
    pub fn channel(&self) -> u8 {
        match self {
            MidiInputMessage::NoteOn { channel, .. }
            | MidiInputMessage::NoteOff { channel, .. }
            | MidiInputMessage::ControlChange { channel, .. } => *channel,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiStatus {
    Searching,
    Connected(String),
    Disconnected(String),
    Error(String),
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    MidiInput {
        timestamp: Instant,
        message: MidiInputMessage,
    },
    MidiStatus(MidiStatus),
    ForceNextVibe,
    ToggleVibeLock,
    BlackoutToggle,
    Quit,
}
