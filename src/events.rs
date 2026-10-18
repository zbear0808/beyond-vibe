use std::time::Instant;

#[derive(Debug, Clone)]
pub enum MidiInputMessage {
    NoteOn { channel: u8, note: u8, velocity: u8 },
    NoteOff { channel: u8, note: u8 },
    ControlChange { channel: u8, controller: u8, value: u8 },
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    MidiInput {
        timestamp: Instant,
        message: MidiInputMessage,
    },
    Tick(Instant),
    ForceNextVibe,
    ToggleVibeLock,
    BlackoutToggle,
    Quit,
}

