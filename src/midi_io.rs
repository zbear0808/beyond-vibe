use crate::config::ControllerMapping;
use crate::events::{EngineEvent, MidiInputMessage, MidiStatus};
use crossbeam_channel::Sender;
use midir::{Ignore, MidiInput, MidiInputConnection, MidiInputPort};
use std::io::{stdin, stdout, Write};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Port names that are loopback/virtual devices, skipped when no device name is configured.
const VIRTUAL_PORT_HINTS: [&str; 5] = ["midi through", "iac driver", "loopmidi", "virtual", "rtpmidi"];

struct ActiveConnection {
    _conn: MidiInputConnection<()>,
    normalized_name: String,
}

type Holder = Arc<Mutex<Option<ActiveConnection>>>;

fn lock(holder: &Holder) -> MutexGuard<'_, Option<ActiveConnection>> {
    holder.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct MidiManager {
    _conn: Holder,
}

impl MidiManager {
    /// Connects in the background and keeps reconnecting if the device is unplugged.
    /// Connection changes are reported to the engine as `EngineEvent::MidiStatus`.
    pub fn start_input_listener(device_name_filter: &str, sender: Sender<EngineEvent>) -> Self {
        let conn_holder: Holder = Arc::new(Mutex::new(None));
        let filter = device_name_filter.to_string();
        let thread_holder = Arc::clone(&conn_holder);

        thread::Builder::new()
            .name("midi-poller".into())
            .spawn(move || {
                let mut scanner = MidiInput::new("beyond-vibe-scan").ok();
                let mut last_status: Option<MidiStatus> = None;
                // Only forward changes so a retrying error isn't reported every poll
                let report = |last: &mut Option<MidiStatus>, status: MidiStatus| {
                    if last.as_ref() != Some(&status) {
                        let _ = sender.send(EngineEvent::MidiStatus(status.clone()));
                        *last = Some(status);
                    }
                };

                loop {
                    let connected_name = lock(&thread_holder).as_ref().map(|c| c.normalized_name.clone());

                    match connected_name {
                        Some(name) => {
                            // midir does not report unplugs, so check the port is still listed
                            if scanner.is_none() {
                                scanner = MidiInput::new("beyond-vibe-scan").ok();
                            }
                            if let Some(scan) = &scanner {
                                let still_present = scan
                                    .ports()
                                    .iter()
                                    .filter_map(|p| scan.port_name(p).ok())
                                    .any(|n| normalize_port_name(&n) == name);
                                if !still_present {
                                    *lock(&thread_holder) = None;
                                    report(&mut last_status, MidiStatus::Disconnected(name));
                                    continue;
                                }
                            }
                        }
                        None => match Self::try_connect(&filter, &sender, &thread_holder) {
                            Ok(Some(name)) => report(&mut last_status, MidiStatus::Connected(name)),
                            Ok(None) => {
                                if !matches!(last_status, Some(MidiStatus::Disconnected(_))) {
                                    report(&mut last_status, MidiStatus::Searching);
                                }
                            }
                            Err(e) => report(&mut last_status, MidiStatus::Error(e)),
                        },
                    }

                    thread::sleep(POLL_INTERVAL);
                }
            })
            .expect("failed to spawn MIDI poller thread");

        Self { _conn: conn_holder }
    }

    /// Returns the connected port name, `None` if no matching device is present.
    fn try_connect(filter: &str, sender: &Sender<EngineEvent>, holder: &Holder) -> Result<Option<String>, String> {
        let mut midi_in = MidiInput::new("beyond-vibe-listener").map_err(|e| format!("MIDI unavailable: {}", e))?;
        midi_in.ignore(Ignore::All);

        let Some((port, name)) = find_port(&midi_in, filter) else {
            return Ok(None);
        };

        let tx = sender.clone();
        let conn = midi_in
            .connect(
                &port,
                "beyond-vibe-in",
                move |_timestamp_us, raw_bytes, _| {
                    let timestamp = Instant::now();
                    if let Some(msg) = parse_midi_bytes(raw_bytes) {
                        let _ = tx.send(EngineEvent::MidiInput {
                            timestamp,
                            message: msg,
                        });
                    }
                },
                (),
            )
            .map_err(|e| {
                if cfg!(windows) {
                    format!("Could not open '{}': {} (is it open in another app, e.g. BEYOND's MIDI settings?)", name, e)
                } else {
                    format!("Could not open '{}': {}", name, e)
                }
            })?;

        *lock(holder) = Some(ActiveConnection {
            _conn: conn,
            normalized_name: normalize_port_name(&name),
        });
        Ok(Some(name))
    }

    /// Run an interactive CLI MIDI Learn session to map the 6 keys and slider
    pub fn run_midi_learn() -> Result<ControllerMapping, Box<dyn std::error::Error>> {
        println!("\n=== BEYOND-VIBE MIDI LEARN WIZARD ===");
        let mut midi_in = MidiInput::new("beyond-vibe-learn")?;
        midi_in.ignore(Ignore::All);

        let ports = midi_in.ports();
        if ports.is_empty() {
            return Err("No MIDI input devices detected for learn wizard.".into());
        }

        println!("Available MIDI Ports:");
        for (i, p) in ports.iter().enumerate() {
            println!(" [{}] {}", i, midi_in.port_name(p).unwrap_or_default());
        }

        let port = loop {
            print!("Select port [0]: ");
            stdout().flush()?;
            let mut input = String::new();
            stdin().read_line(&mut input)?;
            let trimmed = input.trim();
            let choice = if trimmed.is_empty() { Some(0) } else { trimmed.parse::<usize>().ok() };
            match choice.and_then(|c| ports.get(c)) {
                Some(p) => break p.clone(),
                None => println!("   Please enter a number between 0 and {}.", ports.len() - 1),
            }
        };
        let dev_name = midi_in.port_name(&port)?;

        let (tx, rx) = crossbeam_channel::bounded::<(u8, bool)>(256);

        let conn = midi_in.connect(
            &port,
            "beyond-vibe-learn-conn",
            move |_, bytes, _| {
                // Never block the OS MIDI thread
                match parse_midi_bytes(bytes) {
                    Some(MidiInputMessage::NoteOn { note, .. }) => {
                        let _ = tx.try_send((note, true));
                    }
                    Some(MidiInputMessage::ControlChange { controller, .. }) => {
                        let _ = tx.try_send((controller, false));
                    }
                    _ => {}
                }
            },
            (),
        )?;

        let drain = || while rx.try_recv().is_ok() {};

        let mut learned: Vec<u8> = Vec::new();
        let mut prompt_key = |name: &str| -> Result<u8, Box<dyn std::error::Error>> {
            drain();
            println!("\n>> Press: {}", name);
            loop {
                let (val, is_note) = rx.recv()?;
                if !is_note {
                    continue;
                }
                if learned.contains(&val) {
                    println!("   Note {} is already assigned to another key, press a different key.", val);
                    continue;
                }
                println!("   Captured Note: {}", val);
                learned.push(val);
                return Ok(val);
            }
        };

        let k1 = prompt_key("Key 1: Main Rhythm (Tap Tempo / Cue trigger)")?;
        let k2 = prompt_key("Key 2: Sub Rhythm")?;
        let k3 = prompt_key("Key 3: Flash / Solo Cue")?;
        let k4 = prompt_key("Key 4: Hold / Latch Toggle")?;
        let k5 = prompt_key("Key 5: Vibe Next (tap) / Lock (hold)")?;
        let k6 = prompt_key("Key 6: Blackout")?;

        drain();
        println!("\n>> Move: Slider 1: Parameter / Morph Fader");
        let s1 = loop {
            let (val, is_note) = rx.recv()?;
            if !is_note {
                println!("   Captured CC: {}", val);
                break val;
            }
        };

        drop(conn);

        println!("\nMIDI Learn complete!");

        Ok(ControllerMapping {
            midi_device_name: normalize_display_name(&dev_name),
            midi_channel: None,
            key_rhythm_main: k1,
            key_rhythm_sub: k2,
            key_flash: k3,
            key_hold_latch: k4,
            key_vibe_next_lock: k5,
            key_blackout: k6,
            cc_slider_morph: s1,
        })
    }
}

fn find_port(midi_in: &MidiInput, filter: &str) -> Option<(MidiInputPort, String)> {
    let filter = normalize_port_name(filter);
    midi_in
        .ports()
        .into_iter()
        .filter_map(|p| midi_in.port_name(&p).ok().map(|n| (p, n)))
        .find(|(_, name)| {
            let normalized = normalize_port_name(name);
            if filter.is_empty() {
                !VIRTUAL_PORT_HINTS.iter().any(|h| normalized.contains(h))
            } else {
                normalized.contains(&filter)
            }
        })
}

/// Strips OS-specific decorations so names match across replugs/reboots:
/// ALSA's trailing client:port ("Pad MIDI 1 24:0") and WinMM's duplicate prefix ("2- Pad").
fn normalize_display_name(name: &str) -> String {
    let mut s = name.trim();

    if let Some((head, tail)) = s.rsplit_once(' ') {
        if let Some((a, b)) = tail.split_once(':') {
            if !a.is_empty() && !b.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit()) {
                s = head.trim_end();
            }
        }
    }
    if let Some((prefix, rest)) = s.split_once("- ") {
        if !prefix.is_empty() && prefix.bytes().all(|c| c.is_ascii_digit()) {
            s = rest.trim_start();
        }
    }
    s.to_string()
}

fn normalize_port_name(name: &str) -> String {
    normalize_display_name(name).to_lowercase()
}

fn parse_midi_bytes(bytes: &[u8]) -> Option<MidiInputMessage> {
    if bytes.len() < 3 {
        return None;
    }
    let status = bytes[0] & 0xF0;
    let channel = bytes[0] & 0x0F;
    let data1 = bytes[1];
    let data2 = bytes[2];

    match status {
        0x90 => {
            if data2 == 0 {
                Some(MidiInputMessage::NoteOff {
                    channel,
                    note: data1,
                })
            } else {
                Some(MidiInputMessage::NoteOn {
                    channel,
                    note: data1,
                    velocity: data2,
                })
            }
        }
        0x80 => Some(MidiInputMessage::NoteOff {
            channel,
            note: data1,
        }),
        0xB0 => Some(MidiInputMessage::ControlChange {
            channel,
            controller: data1,
            value: data2,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_os_port_names() {
        assert_eq!(normalize_port_name("Launchpad Mini:Launchpad Mini MIDI 1 24:0"), "launchpad mini:launchpad mini midi 1");
        assert_eq!(normalize_port_name("2- Launchpad Mini"), "launchpad mini");
        assert_eq!(normalize_port_name("  RP2040 Pad "), "rp2040 pad");
        assert_eq!(normalize_port_name("Pad 1:2 thing"), "pad 1:2 thing");
    }

    #[test]
    fn parses_messages() {
        assert!(matches!(parse_midi_bytes(&[0x91, 36, 0]), Some(MidiInputMessage::NoteOff { channel: 1, note: 36 })));
        assert!(matches!(parse_midi_bytes(&[0xB0, 1, 64]), Some(MidiInputMessage::ControlChange { value: 64, .. })));
        assert!(parse_midi_bytes(&[0xF8]).is_none());
        assert!(parse_midi_bytes(&[0x90, 36]).is_none());
    }
}
