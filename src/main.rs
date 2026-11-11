mod beyond_osc;
mod config;
mod engine;
mod events;
mod midi_io;
mod ui;

use beyond_osc::BeyondClient;
use config::AppConfig;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use engine::{Engine, EngineSnapshot};
use events::EngineEvent;
use midi_io::MidiManager;
use std::io::IsTerminal;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use ui::TerminalUi;

const TICK: Duration = Duration::from_millis(1);
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(16);
const ENGINE_THREAD_NAME: &str = "engine";

const HELP: &str = "\
beyond-vibe - MIDI controller to Pangolin BEYOND OSC middleware

USAGE:
    beyond-vibe [OPTIONS]

OPTIONS:
    --config <path>   Use this config file instead of vibes.toml next to the program
    --learn           Run the MIDI learn wizard and save the key mapping to the config
    --headless        Run without the dashboard (stop with Ctrl+C)
    -h, --help        Show this help
    -V, --version     Show the version";

struct Args {
    learn: bool,
    headless: bool,
    config: Option<PathBuf>,
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut args = Args {
        learn: false,
        headless: false,
        config: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--learn" => args.learn = true,
            "--headless" => args.headless = true,
            "--config" => {
                let path = it.next().ok_or("--config needs a file path")?;
                args.config = Some(PathBuf::from(path));
            }
            "-h" | "--help" => {
                println!("{}", HELP);
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("beyond-vibe {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            other => return Err(format!("Unknown option '{}'.\n\n{}", other, HELP)),
        }
    }
    Ok(Some(args))
}

fn main() {
    if let Err(e) = run() {
        ui::restore_terminal();
        eprintln!("\nbeyond-vibe could not continue:\n  {}\n", e.replace('\n', "\n  "));
        // When double-clicked, the console window would otherwise close before the message can be read
        if std::io::stdin().is_terminal() {
            eprint!("Press Enter to close...");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let Some(args) = parse_args()? else {
        return Ok(());
    };

    // 1. Load (or create) and validate config before anything else, so a broken file is reported up front
    let (config, config_path, warnings) = AppConfig::load_or_create(args.config.as_deref())?;
    println!("Loaded config from: {}", config_path.display());
    for w in &warnings {
        println!("  warning: {}", w);
    }

    if args.learn {
        let mapping = MidiManager::run_midi_learn().map_err(|e| e.to_string())?;
        AppConfig::save_controller(&config_path, &mapping)?;
        println!("Updated config saved to: {}", config_path.display());
        return Ok(());
    }

    println!("Starting beyond-vibe...");

    // 2. Initialize Pangolin BEYOND OSC UDP client
    let beyond_client = BeyondClient::new(
        &config.network.beyond_ip,
        config.network.beyond_port,
        config.grid.columns,
    )?;
    println!("Pangolin BEYOND OSC target: {}", beyond_client.target());

    #[cfg(windows)]
    let _timer = WindowsTimerResolution::new();

    // 3. Channels for lock-free communication
    let (event_tx, event_rx) = crossbeam_channel::unbounded::<EngineEvent>();
    let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(1);

    // 4. Start MIDI input listener (connects/reconnects in the background)
    let _midi_manager = MidiManager::start_input_listener(&config.controller.midi_device_name, event_tx.clone());

    // 5. Shared state for UI rendering
    let now = Instant::now();
    let mut state_engine = Engine::new(config, beyond_client, now);
    state_engine.startup_clear();
    let snapshot_shared = Arc::new(Mutex::new(state_engine.snapshot()));
    let engine_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    install_panic_hook();

    // 6. Spawn Engine thread
    let engine_handle = {
        let snapshot = Arc::clone(&snapshot_shared);
        let engine_error = Arc::clone(&engine_error);
        thread::Builder::new()
            .name(ENGINE_THREAD_NAME.into())
            .spawn(move || {
                let result = panic::catch_unwind(AssertUnwindSafe(|| {
                    engine_loop(&mut state_engine, &event_rx, &snapshot);
                }));
                if let Err(payload) = result {
                    let msg = panic_message(payload.as_ref());
                    *engine_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg);
                    // Best effort: don't leave lasers running after a crash
                    let _ = panic::catch_unwind(AssertUnwindSafe(|| state_engine.shutdown()));
                }
                let _ = done_tx.send(());
            })
            .map_err(|e| format!("Could not start engine thread: {}", e))?
    };

    // 7. Ctrl+C / SIGTERM / console window close: stop cues before the process dies
    {
        let tx = event_tx.clone();
        let done_rx = done_rx.clone();
        let _ = ctrlc::set_handler(move || {
            let _ = tx.send(EngineEvent::Quit);
            let _ = done_rx.recv_timeout(Duration::from_secs(2));
            ui::restore_terminal();
            std::process::exit(0);
        });
    }

    // 8. Run Terminal UI on main thread (or headless if requested)
    let ui_result = if args.headless {
        println!("Running in headless mode. Press Ctrl+C to stop.");
        let _ = done_rx.recv();
        Ok(())
    } else {
        TerminalUi::run(Arc::clone(&snapshot_shared), Arc::clone(&engine_error), event_tx.clone())
    };

    // 9. Always stop cues and wait for the engine to finish before exiting
    let _ = event_tx.send(EngineEvent::Quit);
    if done_rx.recv_timeout(Duration::from_secs(2)).is_ok() {
        let _ = engine_handle.join();
    }

    if let Some(err) = engine_error.lock().unwrap_or_else(|e| e.into_inner()).take() {
        return Err(format!("The engine stopped unexpectedly: {}", err));
    }
    ui_result.map_err(|e| format!("Dashboard error: {}", e))
}

fn engine_loop(engine: &mut Engine, rx: &Receiver<EngineEvent>, snapshot: &Mutex<EngineSnapshot>) {
    let mut next_tick = Instant::now() + TICK;
    let mut last_publish = Instant::now();

    loop {
        // Wait until the next tick deadline; ticks still run on time under a constant stream of MIDI events
        match rx.recv_deadline(next_tick) {
            Ok(event) => {
                let is_quit = matches!(event, EngineEvent::Quit);
                engine.handle_event(event, Instant::now());
                if is_quit {
                    *snapshot.lock().unwrap_or_else(|e| e.into_inner()) = engine.snapshot();
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                engine.shutdown();
                return;
            }
        }

        let now = Instant::now();
        if now >= next_tick {
            engine.tick(now);
            next_tick = now + TICK;
        }

        if now.saturating_duration_since(last_publish) >= SNAPSHOT_INTERVAL {
            *snapshot.lock().unwrap_or_else(|e| e.into_inner()) = engine.snapshot();
            last_publish = now;
        }
    }
}

fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // Engine panics are caught and shown in the dashboard; printing would garble the screen
        if thread::current().name() == Some(ENGINE_THREAD_NAME) && ui::TUI_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        ui::restore_terminal();
        default_hook(info);
    }));
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown error".to_string())
}

/// Windows timers default to ~15.6 ms resolution; request 1 ms for accurate cue timing.
#[cfg(windows)]
struct WindowsTimerResolution;

#[cfg(windows)]
impl WindowsTimerResolution {
    fn new() -> Self {
        unsafe {
            windows_sys::Win32::Media::timeBeginPeriod(1);
        }
        Self
    }
}

#[cfg(windows)]
impl Drop for WindowsTimerResolution {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Media::timeEndPeriod(1);
        }
    }
}
