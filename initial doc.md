Requiring **Windows, macOS, and Linux compatibility** inside a **single zero-install executable** immediately settles your biggest architectural question: **you must use OSC/PangoScript over UDP rather than Virtual MIDI.**

Here is why that constraint changes everything, which language stack will actually give you a clean single file, and how to design the executable so other laser ops can drop it onto a USB stick and run it anywhere.

---

## 1. The Hidden OS Trap: Why Virtual MIDI Breaks "Single Executable" on Windows

To read your physical 6-key MIDI controller, all three operating systems have built-in native APIs (`winmm.dll` on Windows, `CoreMIDI` on macOS, and `ALSA` on Linux). Any compiled program can read a hardware USB MIDI controller with zero drivers installed.

**However, outputting MIDI to another program on the same computer (Virtual MIDI) is a different story:**

* **macOS (`CoreMIDI`) and Linux (`ALSA`)** allow your code to spin up a virtual MIDI output port natively in three lines of code.
* **Windows has no native virtual MIDI loopback API** across standard Windows 10/11 installations. For your software to appear as a MIDI input inside Pangolin BEYOND on Windows, the user would be forced to install a third-party kernel driver like **loopMIDI** or **teVirtualMIDI**—instantly breaking your "single portable executable" goal.

### Why UDP / OSC Solves Both Portability and Cross-Platform Use

1. **Zero Drivers:** Every OS natively supports sending UDP packets out of the box.
2. **Network Bonus for Mac/Linux Users:** Pangolin BEYOND itself only runs on Windows. Because OSC runs over standard network sockets, someone can run your executable on a **macOS laptop, Linux machine, Steam Deck, or Raspberry Pi** at the front-of-house booth, plug their 6-key MIDI board into it, and point your software at the Windows laser PC's IP address (`192.168.1.x:8000`) over Ethernet or Wi-Fi. If it's running on the same Windows PC as BEYOND, it just defaults to `127.0.0.1:8000`.

---

## 2. Choosing the Right Language for a True Single Binary

Not all "single executables" are equal. Some languages compile to native machine code, while others pack an entire language runtime into a self-extracting zip file that gets flagged by Windows Defender on other people's show laptops.

| Tech Stack | Binary Size | True Native Binary? | Hardware MIDI Library | The Verdict for Your Project |
| --- | --- | --- | --- | --- |
| **Rust** | **3 – 8 MB** | **Yes** | `midir` (Pure Rust wrapper for WinMM, CoreMIDI, ALSA) | **Best Overall.** Compiles to a tiny, rock-solid native executable with sub-millisecond timing jitter and zero external DLLs. |
| **Go (Golang)** | **8 – 15 MB** | **Yes** | `gomidi/v2` + `rtmidi` driver | **Easiest Concurrency.** `goroutines` make beat timers and armed state transitions effortless to write. Requires `cgo` at build time for MIDI. |
| **Tauri (Rust + Web UI)** | **10 – 15 MB** | **Yes** (uses OS WebView) | `midir` (in Rust backend) | **Best for a Polished GUI.** Gives you a modern HTML/CSS/React desktop app without the 180 MB bloat of Electron. |
| **Python + PyInstaller** | **35 – 60 MB** | **No** (Self-extracting archive) | `mido` + `python-rtmidi` | **Great for Prototyping, Poor for Distribution.** Unpacks to a `TEMP` folder on every launch (slow startup) and frequently triggers antivirus false positives when shared with other users. |

---

## 3. How to Handle the UI Inside a Single Executable

Even if your live performance only uses the 6 physical keys and 1 slider, users still need a way to **select their MIDI controller, see what Vibe Group is currently active, watch the transition timer, and edit their cue pools.**

You have three ways to include a UI without breaking the single-file rule:

### Option A: The "Embedded Localhost Web UI" (Works in Rust or Go)

* **How it works:** You compile a single-page HTML/JS dashboard *directly inside the executable binary* (using `rust-embed` in Rust or `//go:embed` in Go).
* **User experience:** When the user double-clicks your executable, it starts the high-speed MIDI/OSC engine in the background and automatically pops open `http://localhost:8080` in their default browser (or runs headless if they don't need the screen).
* **Why it's great:** Zero OS-specific graphics driver headaches, and a performer can even open that local URL on their **phone or iPad** propped up next to their 6-key controller as a live status display.

### Option B: Immediate-Mode Native Window (`egui` / `eframe` in Rust)

* **How it works:** Uses a lightweight Rust GUI library (`egui`) that draws a 60 FPS hardware-accelerated window directly using OpenGL/Metal/DirectX.
* **User experience:** Double-clicking the 6 MB executable immediately opens a dark-mode native utility window with a MIDI device dropdown, live BPM readout, and a progress bar showing when the Vibe Group is `ARMED_FOR_SWITCH`.

### Option C: Interactive Terminal UI (`ratatui` in Rust or `Bubble Tea` in Go)

* **How it works:** Renders a sleek dashboard with keyboard navigation, color-coded boxes, and timers directly inside the terminal/console window that opens when you launch the binary.
* **Why it's great:** Fastest to build, zero graphics dependencies, and uses virtually 0% CPU/GPU on a laptop already working hard to run BEYOND.

---

## 4. Architecture & Packaging Decisions for "USB-Stick Portability"

To make this genuinely frictionless for yourself and other laser operators, build these four behaviors into the executable from day one:

1. **Side-by-Side Portable Config (`vibes.json` or `vibes.toml`):**
* Never store the user's cue mappings inside hidden OS folders (`%APPDATA%` on Windows or `~/Library/Application Support` on macOS).
* Have the executable check **its own current directory first** for a `vibes.json` file, and automatically generate a clean, commented default `vibes.json` right next to the executable if none exists. That way, an operator can keep `laser-vibe.exe` and `my-show-vibes.json` in one folder on a thumb drive and move between venues effortlessly.


2. **Interactive "MIDI Learn" on First Launch:**
* Cheap 6-key macro pads and MIDI controllers all send different MIDI notes or CC numbers. Build a simple startup prompt (or button): *"Press Key 1 (Main Rhythm)... Press Key 2 (Sub Rhythm)... Move Slider 1..."* and save those bindings straight into the local config file.


3. **Auto-Reconnect Polling for Hardware MIDI:**
* In a live booth, USB cables get bumped. Many MIDI libraries silently stop receiving messages if a USB device disconnects and reconnects. Run a lightweight background check every 2 seconds that listens for the device name and automatically re-binds the port if unplugged and plugged back in.


4. **Automated Multi-OS Building via GitHub Actions:**
* Because Windows `winmm`, macOS `CoreMIDI`, and Linux `ALSA` require linking against each OS's native audio headers during compilation, you can't easily compile a macOS binary from a Windows PC locally.
* Instead, host your code on a GitHub repository (public or private) with a 40-line **GitHub Actions workflow**. Every time you push code, GitHub's cloud servers will spin up a Windows VM, a macOS VM, and an Ubuntu Linux VM simultaneously and hand you three ready-to-download executables (`vibe-win.exe`, `vibe-macos`, `vibe-linux`).