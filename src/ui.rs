use crate::engine::tap_tempo::{MAX_BPM, MIN_BPM};
use crate::engine::{EngineSnapshot, VibeTransitionState};
use crate::events::{EngineEvent, MidiStatus};
use crossbeam_channel::Sender;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io::{stdout, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const QUIT_CONFIRM_WINDOW: Duration = Duration::from_secs(2);
const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 18;

/// True while the dashboard owns the terminal (used by the panic hook in main).
pub static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Restores the terminal even if the render loop returns early with an error.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

pub fn restore_terminal() {
    if TUI_ACTIVE.swap(false, Ordering::SeqCst) {
        ratatui::restore();
    }
}

pub struct TerminalUi;

impl TerminalUi {
    pub fn run(
        snapshot_handle: Arc<Mutex<EngineSnapshot>>,
        engine_error: Arc<Mutex<Option<String>>>,
        command_tx: Sender<EngineEvent>,
    ) -> std::io::Result<()> {
        TUI_ACTIVE.store(true, Ordering::SeqCst);
        let _guard = TerminalGuard;
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
        Self::render_loop(&mut terminal, snapshot_handle, engine_error, command_tx)
    }

    fn render_loop(
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        snapshot_handle: Arc<Mutex<EngineSnapshot>>,
        engine_error: Arc<Mutex<Option<String>>>,
        command_tx: Sender<EngineEvent>,
    ) -> std::io::Result<()> {
        let mut quit_requested_at: Option<Instant> = None;

        loop {
            if event::poll(Duration::from_millis(30))? {
                if let Event::Key(key) = event::read()? {
                    // Windows reports both Press and Release; only act once per keystroke
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                        return Ok(());
                    }
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                            // Require a second press so a stray key doesn't end the show
                            if quit_requested_at.is_some_and(|t| t.elapsed() < QUIT_CONFIRM_WINDOW) {
                                return Ok(());
                            }
                            quit_requested_at = Some(Instant::now());
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Right => {
                            let _ = command_tx.send(EngineEvent::ForceNextVibe);
                        }
                        KeyCode::Char('l') | KeyCode::Char('L') => {
                            let _ = command_tx.send(EngineEvent::ToggleVibeLock);
                        }
                        KeyCode::Char('b') | KeyCode::Char('B') => {
                            let _ = command_tx.send(EngineEvent::BlackoutToggle);
                        }
                        _ => {}
                    }
                }
            }

            let snap = snapshot_handle.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let error = engine_error.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let confirming_quit = quit_requested_at.is_some_and(|t| t.elapsed() < QUIT_CONFIRM_WINDOW);

            terminal.draw(|f| draw(f, &snap, error.as_deref(), confirming_quit))?;
        }
    }
}

fn draw(f: &mut Frame, snap: &EngineSnapshot, engine_error: Option<&str>, confirming_quit: bool) {
    let area = f.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let msg = Paragraph::new(format!(
            "Terminal too small ({}x{}). Please enlarge to at least {}x{}.\nPress Q twice to quit.",
            area.width, area.height, MIN_WIDTH, MIN_HEIGHT
        ))
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(Color::Yellow));
        f.render_widget(msg, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Alert banner (blackout / engine error / quit confirm)
            Constraint::Length(4), // Vibe Group Status
            Constraint::Length(3), // Transition State Indicator
            Constraint::Length(3), // Tap Tempo BPM readout
            Constraint::Length(4), // Connections
            Constraint::Min(4),    // Keybinds & help
        ])
        .split(area);

    draw_banner(f, chunks[0], snap, engine_error, confirming_quit);

    // Active Vibe Group
    let group_lines = vec![
        Line::from(format!(
            " Group [{}/{}]: {}",
            snap.current_group_index + 1,
            snap.total_groups,
            snap.current_group_name
        )),
        Line::from(format!(
            " Active cues: {}   Latch: {}",
            snap.active_cues_count,
            if snap.is_latched { "ON" } else { "off" }
        )),
    ];
    let group_block = Paragraph::new(group_lines)
        .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL).title("Active Vibe"));
    f.render_widget(group_block, chunks[1]);

    // Transition State
    let (state_text, state_color) = match snap.transition_state {
        VibeTransitionState::Active { .. } => ("ACTIVE (timer running)", Color::Yellow),
        VibeTransitionState::Armed => ("ARMED - switching on next gap/flash", Color::Magenta),
        VibeTransitionState::Locked => ("LOCKED (auto-rotation paused)", Color::Blue),
    };
    let state_para = Paragraph::new(format!(" {}", state_text))
        .style(Style::default().fg(state_color).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL).title("Rotation State"));
    f.render_widget(state_para, chunks[2]);

    // BPM Gauge / Readout
    let bpm_gauge = Gauge::default()
        .block(Block::default().title("Tap Tempo").borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::LightBlue))
        .ratio(((snap.bpm - MIN_BPM) / (MAX_BPM - MIN_BPM)).clamp(0.0, 1.0))
        .label(format!("{:.1} BPM", snap.bpm));
    f.render_widget(bpm_gauge, chunks[3]);

    // Connections
    let (midi_text, midi_color) = match &snap.midi_status {
        MidiStatus::Searching => ("searching for device...".to_string(), Color::Yellow),
        MidiStatus::Connected(name) => (format!("connected: {}", name), Color::Green),
        MidiStatus::Disconnected(name) => (format!("DISCONNECTED: {} (waiting to reconnect)", name), Color::Red),
        MidiStatus::Error(e) => (format!("ERROR: {}", e), Color::Red),
    };
    let osc_text = match (&snap.last_osc_error, snap.osc_errors) {
        (Some(e), n) if n > 0 => format!(" OSC  -> {}  ({} send errors, last: {})", snap.osc_target, n, e),
        _ => format!(" OSC  -> {}", snap.osc_target),
    };
    let conn_para = Paragraph::new(vec![
        Line::styled(format!(" MIDI: {}", midi_text), Style::default().fg(midi_color)),
        Line::styled(
            osc_text,
            Style::default().fg(if snap.osc_errors > 0 { Color::Red } else { Color::Gray }),
        ),
    ])
    .wrap(Wrap { trim: false })
    .block(Block::default().borders(Borders::ALL).title("Connections"));
    f.render_widget(conn_para, chunks[4]);

    // Help / Footer
    let help_para = Paragraph::new(vec![
        Line::from(" [N / Right] Next vibe    [L] Lock / unlock vibe"),
        Line::from(" [B] Blackout on / off    [Q / Esc twice] Quit"),
    ])
    .wrap(Wrap { trim: false })
    .style(Style::default().fg(Color::DarkGray))
    .block(Block::default().borders(Borders::ALL).title("Keyboard Shortcuts"));
    f.render_widget(help_para, chunks[5]);
}

fn draw_banner(f: &mut Frame, area: Rect, snap: &EngineSnapshot, engine_error: Option<&str>, confirming_quit: bool) {
    let (text, fg, bg) = if let Some(err) = engine_error {
        (format!(" ENGINE STOPPED: {} - restart beyond-vibe", err), Color::White, Color::Red)
    } else if confirming_quit {
        (" Press Q / Esc again to quit (all cues will stop)".to_string(), Color::Black, Color::Yellow)
    } else if snap.is_blackout {
        (" *** BLACKOUT ACTIVE *** (press Key 6 or B to release)".to_string(), Color::White, Color::Red)
    } else {
        (" BEYOND-VIBE: MIDI -> BEYOND OSC".to_string(), Color::Cyan, Color::Reset)
    };
    let banner = Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(banner, area);
}
