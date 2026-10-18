Building this middleware in Rust gives you a native 4–8 MB single executable with sub-millisecond timing accuracy, zero runtime dependencies, and direct access to Windows (`winmm`), macOS (`CoreMIDI`), and Linux (`ALSA`) hardware ports.

---

## 1. Crate Stack (`Cargo.toml`)

Every crate in this stack is chosen specifically so that `cargo build --release` produces a standalone binary with no external `.dll`, `.dylib`, or `.so` files required at runtime.

```toml
[package]
name = "beyond-vibe"
version = "0.1.0"
edition = "2021"

[dependencies]
# Cross-platform native MIDI I/O (WinMM, CoreMIDI, ALSA)
midir = "0.10"

# Open Sound Control (OSC) packet encoding for Pangolin BEYOND
rosc = "0.10"

# Lock-free multi-producer single-consumer channels between MIDI callback and Engine
crossbeam-channel = "0.5"

# Human-friendly config parsing (TOML is much easier for laser ops to edit than JSON)
serde = { version = "1.0", features = ["derive"] }
toml = "0.8"

# Randomization for the non-repeating "Shuffle Bag" cue selector
rand = "0.8"

# Lightweight cross-platform Terminal UI (or swap for `eframe` if you want a GUI window)
ratatui = "0.28"
crossterm = "0.28"

```

---

## 2. Project Directory Architecture

Keep the hardware I/O, state machine logic, and BEYOND network protocol strictly separated so you can test the state machine without needing Pangolin BEYOND open:

```text
beyond-vibe/
├── Cargo.toml
├── vibes.toml                # Auto-generated portable config next to the binary
└── src/
    ├── main.rs               # Entry point, CLI/TUI bootstrap, thread spawning
    ├── config.rs             # Serde structs + auto-generation of default vibes.toml
    ├── midi_io.rs            # midir port connection, hot-plug polling, MIDI Learn
    ├── events.rs             # Unified internal Event enum (MidiEvent, Tick, UiCommand)
    ├── engine/
    │   ├── mod.rs            # Main 1ms tick loop & state coordinator
    │   ├── tap_tempo.rs      # Rolling BPM calculator & beat-fraction timer
    │   ├── shuffle_bag.rs    # Non-repeating cue selector + step modifier cycler
    │   └── janitor.rs        # Tracks active BEYOND cells to guarantee clean stops
    ├── beyond_osc.rs         # UDP socket + OSC/PangoScript packet builder
    └── ui.rs                 # Live status dashboard (BPM, Active Vibe, Armed Timer)

```

---

## 3. Concurrency & Real-Time Threading Model

`midir` reads hardware MIDI on a high-priority OS audio thread. You should **never** put mutex locks, timers, or network socket calls inside the `midir` callback. Instead, use a lock-free channel architecture across three threads:

1. **Thread 1: OS MIDI Callback (`midi_io.rs`)**
* Receives raw 3-byte MIDI messages from your 6-key board.
* Immediately stamps them with `std::time::Instant::now()` for microsecond-accurate tap tempo calculation.
* Pushes a lightweight `EngineEvent::MidiInput` into a `crossbeam_channel::Sender` and returns in under 5 microseconds.


2. **Thread 2: The Core State Engine (`engine/mod.rs`)**
* Runs a deterministic loop waking every `1ms` (using `channel.recv_timeout(Duration::from_millis(1))`).
* Processes incoming MIDI gestures **and** evaluates time-based decays (e.g., *"Has 1.5 beats elapsed since Key 1 was tapped? If so, tell the Janitor to stop the cue"* and *"Has 45 seconds elapsed in Group 1? If so, transition state to Armed"*).
* Fires UDP packets directly to BEYOND via standard non-blocking `std::net::UdpSocket`.


3. **Thread 3: Status UI (`ui.rs`)**
* Renders the terminal or native GUI at 30 FPS by reading a snapshot of the engine state, ensuring screen redraws never jitter your laser timing.



---

## 4. Data Structures & Portable Configuration (`config.rs`)

Using **TOML** instead of JSON allows you and other operators to leave comments (`#`) right inside the configuration file next to the executable.

### Rust Data Model

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub network: NetworkConfig,
    pub controller: ControllerMapping,
    pub timing: TimingConfig,
    pub groups: Vec<VibeGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub beyond_ip: String,      // Default: "127.0.0.1"
    pub beyond_port: u16,       // Default: 8000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerMapping {
    pub midi_device_name: String, // Substring match, e.g., "Launchpad" or "RP2040"
    pub key_rhythm_main: u8,      // MIDI Note for Key 1
    pub key_rhythm_sub: u8,       // MIDI Note for Key 2
    pub key_flash: u8,            // MIDI Note for Key 3
    pub key_hold_latch: u8,       // MIDI Note for Key 4
    pub key_vibe_next_lock: u8,   // MIDI Note for Key 5
    pub key_blackout: u8,         // MIDI Note for Key 6
    pub cc_slider_morph: u8,      // MIDI CC number for Slider 1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimingConfig {
    pub default_bpm: f64,               // Fallback BPM before taps are registered (e.g., 128.0)
    pub group_duration_secs: u64,       // Time before group enters ARMED state (e.g., 45)
    pub pause_beats_to_switch: f64,     // Beats of silence required to execute armed switch (e.g., 2.0)
    pub quick_tap_threshold_ms: u64,    // Under 150ms = quantized decay; over 150ms = manual gate hold
    pub default_decay_beats: f64,       // How many beats a quick tap sustains (e.g., 1.0 or 2.0)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct CueCell {
    pub page: u8, // 1-based Page index in BEYOND
    pub row: u8,  // 1-based Row index
    pub col: u8,  // 1-based Column index
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SelectionMode {
    RoundRobin,
    ShuffleBag, // Random without immediate repeat
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepModifier {
    None,
    CycleColorPalette(Vec<u8>),       // Cycles BEYOND Live Control Color indices on each tap
    CyclePangoScript(Vec<String>),    // Runs custom PangoScript per tap (e.g., X-invert toggle)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FlashBehavior {
    TriggerSoloCue(CueCell),          // Mutes rhythm cue while held, fires high-energy cue
    OverlayPangoScript {              // Keeps rhythm cue alive, injects FX while held
        on_press: String,
        on_release: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VibeGroup {
    pub name: String,
    pub selection_mode: SelectionMode,
    pub rhythm_cues: Vec<CueCell>,
    pub step_modifier: StepModifier,
    pub flash_behavior: FlashBehavior,
    pub slider_pangoscript_target: String, // e.g., "SizeY" or "LaserOutput"
}

```

---

## 5. Core Engine Algorithms (`engine/`)

### A. Rolling Tap-Tempo & Quantized Beat Decay (`tap_tempo.rs`)

You want cues to last 1–2 beats when tapped, or cut short at half a beat if you tap early or release a hold.

1. **Tap History Buffer:** Store a `VecDeque` of the last 4 `Instant` timestamps from `key_rhythm_main`.
2. **Reset on Pause:** If the gap between the current tap and the previous tap exceeds `2.5 seconds` (less than 24 BPM), clear the buffer so a pause between songs doesn't ruin the average.
3. **Beat Duration Calculation:** Average the intervals between the stored timestamps to compute `beat_duration_ms`.
4. **Cut-Short & Auto-Decay Logic:**
* When `key_rhythm_main` is pressed (`NoteOn`), immediately stop the previously active rhythm cue, trigger the new cue from the pool, and set `active_cue_Started_at = Instant::now()`.
* Set an initial auto-off deadline: `auto_off_at = now + (beat_duration * default_decay_beats)`.
* When `NoteOff` arrives:
* Calculate `held_ms = now - active_cue_started_at`.
* If `held_ms` is **less than** `quick_tap_threshold_ms` (e.g., a fast 80ms tap), let the cue keep playing until either `auto_off_at` is reached **or** the next tap arrives at half a beat (which immediately cuts it off and fires the next cue).
* If `held_ms` is **greater than** `quick_tap_threshold_ms` (you intentionally held the key down for half a beat and lifted your finger), stop the cue immediately on `NoteOff`.





### B. The Non-Repeating "Shuffle Bag" (`shuffle_bag.rs`)

Pure random selection feels broken in live performance because a 4-cue group has a 25% chance of picking the exact same cue twice in a row—making your button press look like it didn't register.

* Maintain a `bag: Vec< usize >` of cue indices `[0, 1, 2, 3]` and `last_picked: Option< usize >`.
* Shuffle the bag. When the bag empties and reshuffles for the next cycle, check if the first item of the *new* shuffle equals `last_picked`. If it does, swap it with the last item in the bag. Every single tap is guaranteed to change the visual state.

### C. The 3-State Vibe Rotation Timer

```rust
pub enum VibeTransitionState {
    Active { entered_at: Instant },
    Armed, // Timer expired; waiting for a musical gap or Flash hit to switch
    Locked, // User toggled Key 5 to stay in this group indefinitely
}

```

* On every 1ms tick, if the state is `Active` and `now.duration_since(entered_at)` exceeds `group_duration_secs`, transition to `Armed`.
* While `Armed`, the engine monitors two triggers:
1. **Phrase Gap Trigger:** No rhythm keys are currently held down, AND `now.duration_since(last_rhythm_activity)` exceeds `beat_duration * pause_beats_to_switch` (e.g., 2 beats of rest).
2. **Flash Mask Trigger:** The user releases `key_flash` (switching to the new group right as the blinding rainbow flash ends is visually seamless).



### D. The Active-State Janitor (`janitor.rs`)

BEYOND tracks cues by grid cell. If your software switches from `VibeGroup 0` to `VibeGroup 1` while a cue in `VibeGroup 0` is still active, a naive controller forgets what was playing.

* `Janitor` holds a `HashSet` of currently active `CueCell`s and active FX overlays.
* Calling `janitor.trigger_exclusive_rhythm(new_cell, &osc_client)` automatically iterates over any currently playing rhythm cells, sends BEYOND's stop command for them, removes them from the set, starts `new_cell`, and inserts `new_cell`.

---

## 6. Pangolin BEYOND OSC & PangoScript Commands (`beyond_osc.rs`)

Pangolin BEYOND listens for UDP OSC messages on port `8000` (configurable inside BEYOND under **Settings -> OSC -> Enable OSC Input**).

The cleanest way to control BEYOND via OSC without needing custom setup inside BEYOND is sending PangoScript commands inside the `/beyond/general/pangoscript` OSC address, or using native BEYOND OSC paths:

```rust
use rosc::{encoder, OscMessage, OscPacket, OscType};
use std::net::UdpSocket;

pub struct BeyondClient {
    socket: UdpSocket,
    target_addr: String,
}

impl BeyondClient {
    pub fn new(ip: &str, port: u16) -> std::io::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            target_addr: format!("{}:{}", ip, port),
        })
    }

    pub fn send_pangoscript(&self, script: &str) {
        let msg = OscPacket::Message(OscMessage {
            addr: "/beyond/general/pangoscript".to_string(),
            args: vec![OscType::String(script.to_string())],
        });
        if let Ok(buf) = encoder::encode(&msg) {
            let _ = self.socket.send_to(&buf, &self.target_addr);
        }
    }

    pub fn start_cue(&self, page: u8, row: u8, col: u8) {
        // PangoScript command to trigger a specific cell
        self.send_pangoscript(&format!("StartCue {}, {}, {}", page, col, row));
    }

    pub fn stop_cue(&self, page: u8, row: u8, col: u8) {
        self.send_pangoscript(&format!("StopCue {}, {}, {}", page, col, row));
    }

    pub fn blackout(&self) {
        self.send_pangoscript("Blackout");
    }

    pub fn set_master_slider(&self, target_property: &str, midi_val: u8) {
        // Maps 0..127 MIDI CC to 0..100% inside BEYOND Live Control
        let pct = (midi_val as f32 / 127.0) * 100.0;
        self.send_pangoscript(&format!("{} {:.1}", target_property, pct));
    }
}

```

### Why `/beyond/general/pangoscript` Is Your Secret Weapon

Because PangoScript is a scripting language built into BEYOND, putting PangoScript strings directly inside your `vibes.toml` file means **you never have to recompile your Rust executable to add new BEYOND features**. If you discover a new BEYOND effect next month—like changing scan angles, triggering a timeline, or strobing a specific projector zone—you just type that PangoScript line into `vibes.toml`.

---

## 7. Cross-Platform Single-Executable Build Pipeline

To produce native single-file executables for **Windows (`.exe`)**, **macOS**, and **Linux** automatically without needing three physical computers:

1. **Linux ALSA Note:** On Linux, `midir` links against `libasound2-dev` at build time.
2. **GitHub Actions Matrix (`.github/workflows/release.yml`):** Create this file in your repo so every push builds all three OS binaries in parallel:

```yaml
name: Build Single Executables

on:
  push:
    tags:
      - 'v*'
  workflow_dispatch:

jobs:
  build:
    strategy:
      matrix:
        include:
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            artifact_name: beyond-vibe.exe
            binary_name: beyond-vibe.exe
          - os: macos-latest
            target: aarch64-apple-darwin
            artifact_name: beyond-vibe-macos
            binary_name: beyond-vibe
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            artifact_name: beyond-vibe-linux
            binary_name: beyond-vibe

    runs-on: ${{ matrix.os }}

    steps:
      - uses: actions/checkout@v4

      - name: Install ALSA headers on Linux
        if: matrix.os == 'ubuntu-latest'
        run: sudo apt-get update && sudo apt-get install -y libasound2-dev pkg-config

      - name: Install Rust Toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Build Release Binary
        run: cargo build --release --target ${{ matrix.target }}

      - name: Upload Single Executable
        uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.artifact_name }}
          path: target/${{ matrix.target }}/release/${{ matrix.binary_name }}

```

---

## 8. Step-by-Step Implementation Roadmap

1. **Milestone 1: Portable Config & MIDI Monitor (Day 1)**
* Write `config.rs` so running the binary generates a default `vibes.toml` in the current working directory if missing.
* Implement `midi_io.rs` with an interactive "MIDI Learn" wizard on first launch that prompts you to press your 6 keys and move your slider, saving the note/CC numbers straight to `vibes.toml`.


2. **Milestone 2: The Core State Machine & Shuffle Bag (Day 2)**
* Implement the 1ms engine loop, `ShuffleBag`, and `Janitor`.
* Verify in the terminal that tapping Key 1 cycles through cues without repeats, holding Key 3 activates the Flash state, and waiting 45 seconds transitions the group from `Active` to `Armed` and switches on a 2-beat pause.


3. **Milestone 3: BEYOND OSC Integration & Live Testing (Day 3)**
* Enable OSC input in BEYOND (`Settings -> OSC`), point `beyond_osc.rs` to `127.0.0.1:8000`, and fine-tune your `quick_tap_threshold_ms` (usually 120–160ms feels most natural on mechanical or MIDI keys).


4. **Milestone 4: TUI / GUI Status View (Day 4)**
* Add a minimal `ratatui` terminal dashboard (or `eframe` window) showing a large color-coded banner for the current Vibe Group, live tap BPM, and a blinking indicator when the next Vibe Group is `ARMED`.