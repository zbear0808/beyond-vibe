mod beyond_osc;
mod config;
mod engine;
mod events;
mod midi_io;
mod ui;

use beyond_osc::BeyondClient;
use config::AppConfig;
use engine::Engine;
use events::EngineEvent;
use midi_io::MidiManager;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use ui::TerminalUi;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    // Check for --learn flag
    if args.iter().any(|arg| arg == "--learn") {
        let mapping = MidiManager::run_midi_learn()?;
        let (mut cfg, path) = AppConfig::load_or_create()?;
        cfg.controller = mapping;
        cfg.save(&path)?;
        println!("Updated config saved to: {}", path.display());
        return Ok(());
    }

    println!("Starting beyond-vibe...");

    // 1. Load or auto-generate config
    let (config, config_path) = AppConfig::load_or_create()?;
    println!("Loaded config from: {}", config_path.display());

    // 2. Initialize Pangolin BEYOND OSC UDP client
    let beyond_client = BeyondClient::new(&config.network.beyond_ip, config.network.beyond_port)?;
    println!(
        "Pangolin BEYOND OSC target: {}:{}",
        config.network.beyond_ip, config.network.beyond_port
    );

    // 3. Channels for lock-free communication
    let (event_tx, event_rx) = crossbeam_channel::unbounded::<EngineEvent>();

    // 4. Start MIDI input listener
    let _midi_manager = MidiManager::start_input_listener(
        &config.controller.midi_device_name,
        event_tx.clone(),
    )?;

    // 5. Shared snapshot for UI rendering
    let mut state_engine = Engine::new(config, beyond_client);
    let snapshot_shared = Arc::new(Mutex::new(state_engine.snapshot()));
    let snapshot_ui_handle = Arc::clone(&snapshot_shared);

    // 6. Spawn Engine tick thread (1ms loop)
    let engine_tx = event_tx.clone();
    thread::spawn(move || {
        loop {
            // Receive events with 1ms timeout to keep accurate 1ms ticks
            match event_rx.recv_timeout(Duration::from_millis(1)) {
                Ok(event) => {
                    let is_quit = matches!(event, EngineEvent::Quit);
                    state_engine.handle_event(event);
                    if is_quit {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    state_engine.handle_event(EngineEvent::Tick(Instant::now()));
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }

            // Update shared snapshot for UI
            if let Ok(mut snap) = snapshot_shared.try_lock() {
                *snap = state_engine.snapshot();
            }
        }
    });

    // 7. Run Terminal UI on main thread (or headless if requested)
    if args.iter().any(|arg| arg == "--headless") {
        println!("Running in headless mode. Press Ctrl+C to terminate.");
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    } else {
        TerminalUi::run(snapshot_ui_handle, engine_tx)?;
    }

    Ok(())
}

