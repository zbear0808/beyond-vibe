use crate::config::ControllerMapping;
use crate::events::{EngineEvent, MidiInputMessage};
use crossbeam_channel::Sender;
use midir::{Ignore, MidiInput, MidiInputConnection};
use std::io::{stdin, stdout, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub struct MidiManager {
    _conn: Arc<Mutex<Option<MidiInputConnection<()>>>>,
}

impl MidiManager {
    pub fn start_input_listener(
        device_name_filter: &str,
        sender: Sender<EngineEvent>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let conn_holder = Arc::new(Mutex::new(None));
        let filter = device_name_filter.to_string();

        // Initial connect attempt
        let _ = Self::try_connect(&filter, &sender, &conn_holder);

        // Spawn reconnect poller thread (checks every 2 seconds if device dropped/reconnected)
        let thread_filter = filter.clone();
        let thread_sender = sender.clone();
        let thread_holder = Arc::clone(&conn_holder);
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(2));
                let guard = thread_holder.lock().unwrap();
                if guard.is_none() {
                    drop(guard);
                    let _ = Self::try_connect(&thread_filter, &thread_sender, &thread_holder);
                }
            }
        });

        Ok(Self {
            _conn: conn_holder,
        })
    }

    fn try_connect(
        filter: &str,
        sender: &Sender<EngineEvent>,
        holder: &Arc<Mutex<Option<MidiInputConnection<()>>>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut midi_in = MidiInput::new("beyond-vibe-listener")?;
        midi_in.ignore(Ignore::None);

        let in_ports = midi_in.ports();
        if in_ports.is_empty() {
            return Ok(());
        }

        let mut target_port = None;
        for port in &in_ports {
            if let Ok(name) = midi_in.port_name(port) {
                if filter.is_empty() || name.to_lowercase().contains(&filter.to_lowercase()) {
                    target_port = Some(port.clone());
                    println!("[MIDI] Connected to: {}", name);
                    break;
                }
            }
        }

        if let Some(port) = target_port {
            let tx = sender.clone();
            let conn = midi_in.connect(
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
            );

            match conn {
                Ok(c) => {
                    let mut guard = holder.lock().unwrap();
                    *guard = Some(c);
                }
                Err(e) => {
                    eprintln!("[MIDI] Failed to connect: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Run an interactive CLI MIDI Learn session to map the 6 keys and slider
    pub fn run_midi_learn() -> Result<ControllerMapping, Box<dyn std::error::Error>> {
        println!("\n=== BEYOND-VIBE MIDI LEARN WIZARD ===");
        let mut midi_in = MidiInput::new("beyond-vibe-learn")?;
        midi_in.ignore(Ignore::None);

        let ports = midi_in.ports();
        if ports.is_empty() {
            return Err("No MIDI input devices detected for learn wizard.".into());
        }

        println!("Available MIDI Ports:");
        for (i, p) in ports.iter().enumerate() {
            println!(" [{}] {}", i, midi_in.port_name(p).unwrap_or_default());
        }

        print!("Select port [0]: ");
        stdout().flush()?;
        let mut input = String::new();
        stdin().read_line(&mut input)?;
        let choice: usize = input.trim().parse().unwrap_or(0);
        let port = ports.get(choice).unwrap_or(&ports[0]);
        let dev_name = midi_in.port_name(port)?;

        let (tx, rx) = crossbeam_channel::bounded::<(u8, bool, u8)>(100);

        let conn = midi_in.connect(
            port,
            "beyond-vibe-learn-conn",
            move |_, bytes, _| {
                if bytes.len() >= 3 {
                    let status = bytes[0] & 0xF0;
                    let d1 = bytes[1];
                    let d2 = bytes[2];
                    if status == 0x90 && d2 > 0 {
                        let _ = tx.send((d1, true, d2)); // Note On
                    } else if status == 0xB0 {
                        let _ = tx.send((d1, false, d2)); // CC
                    }
                }
            },
            (),
        )?;

        let prompt_key = |name: &str| -> Result<u8, Box<dyn std::error::Error>> {
            println!("\n>> Press: {}", name);
            loop {
                let (val, is_note, _) = rx.recv()?;
                if is_note {
                    println!("   Captured Note: {}", val);
                    return Ok(val);
                }
            }
        };

        let prompt_cc = |name: &str| -> Result<u8, Box<dyn std::error::Error>> {
            println!("\n>> Move: {}", name);
            loop {
                let (val, is_note, _) = rx.recv()?;
                if !is_note {
                    println!("   Captured CC: {}", val);
                    return Ok(val);
                }
            }
        };

        let k1 = prompt_key("Key 1: Main Rhythm (Tap Tempo / Cue trigger)")?;
        let k2 = prompt_key("Key 2: Sub Rhythm")?;
        let k3 = prompt_key("Key 3: Flash / Solo Cue")?;
        let k4 = prompt_key("Key 4: Hold / Latch Toggle")?;
        let k5 = prompt_key("Key 5: Vibe Next / Lock")?;
        let k6 = prompt_key("Key 6: Blackout")?;
        let s1 = prompt_cc("Slider 1: Parameter / Morph Fader")?;

        drop(conn);

        println!("\nMIDI Learn complete!");

        Ok(ControllerMapping {
            midi_device_name: dev_name,
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

