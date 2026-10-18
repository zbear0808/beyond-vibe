use crate::engine::{EngineSnapshot, VibeTransitionState};
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Gauge, Paragraph},
    Terminal,
};
use std::io::stdout;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct TerminalUi;

impl TerminalUi {
    pub fn run(
        snapshot_handle: Arc<Mutex<EngineSnapshot>>,
        command_tx: crossbeam_channel::Sender<crate::events::EngineEvent>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let res = Self::render_loop(&mut terminal, snapshot_handle, command_tx);

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        res
    }

    fn render_loop<B: ratatui::backend::Backend>(
        terminal: &mut Terminal<B>,
        snapshot_handle: Arc<Mutex<EngineSnapshot>>,
        command_tx: crossbeam_channel::Sender<crate::events::EngineEvent>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            // Check keyboard input for manual UI overrides
            if event::poll(Duration::from_millis(30))? {
                if let Event::Key(key) = event::read()? {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            let _ = command_tx.send(crate::events::EngineEvent::Quit);
                            break;
                        }
                        KeyCode::Char('n') | KeyCode::Right => {
                            let _ = command_tx.send(crate::events::EngineEvent::ForceNextVibe);
                        }
                        KeyCode::Char('l') => {
                            let _ = command_tx.send(crate::events::EngineEvent::ToggleVibeLock);
                        }
                        KeyCode::Char('b') => {
                            let _ = command_tx.send(crate::events::EngineEvent::BlackoutToggle);
                        }
                        _ => {}
                    }
                }
            }

            let snap = {
                let guard = snapshot_handle.lock().unwrap();
                guard.clone()
            };

            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints(
                        [
                            Constraint::Length(3), // Header banner
                            Constraint::Length(4), // Vibe Group Status
                            Constraint::Length(3), // Transition State Indicator
                            Constraint::Length(3), // Tap Tempo BPM readout
                            Constraint::Min(3),    // Keybinds & help
                        ]
                        .as_ref(),
                    )
                    .split(f.area());

                // 1. Header
                let title = Paragraph::new("⚡ BEYOND-VIBE: Zero-Install MIDI -> OSC Middleware ⚡")
                    .style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                    .block(Block::default().borders(Borders::ALL).title("Status"));
                f.render_widget(title, chunks[0]);

                // 2. Active Vibe Group
                let group_info = format!(
                    " Current Group [{}/{}]: {} | Active Cues: {} {}",
                    snap.current_group_index + 1,
                    snap.total_groups,
                    snap.current_group_name,
                    snap.active_cues_count,
                    if snap.is_blackout { " [*** BLACKOUT ACTIVE ***]" } else { "" }
                );
                let group_block = Paragraph::new(group_info)
                    .style(
                        Style::default()
                            .fg(if snap.is_blackout { Color::Red } else { Color::Green })
                            .add_modifier(Modifier::BOLD),
                    )
                    .block(Block::default().borders(Borders::ALL).title("Active Vibe"));
                f.render_widget(group_block, chunks[1]);

                // 3. Transition State
                let (state_text, state_color) = match snap.transition_state {
                    VibeTransitionState::Active { .. } => ("State: ACTIVE (Timer running)", Color::Yellow),
                    VibeTransitionState::Armed => ("State: *** ARMED FOR SWITCH (Waiting for gap/flash) ***", Color::Magenta),
                    VibeTransitionState::Locked => ("State: [LOCKED] (Auto-rotation paused)", Color::Blue),
                };
                let state_para = Paragraph::new(format!(" {}", state_text))
                    .style(Style::default().fg(state_color).add_modifier(Modifier::BOLD))
                    .block(Block::default().borders(Borders::ALL).title("Rotation State"));
                f.render_widget(state_para, chunks[2]);

                // 4. BPM Gauge / Readout
                let bpm_label = format!("{:.1} BPM", snap.bpm);
                let bpm_gauge = Gauge::default()
                    .block(Block::default().title("Tap Tempo").borders(Borders::ALL))
                    .gauge_style(Style::default().fg(Color::LightBlue))
                    .ratio(((snap.bpm - 60.0).max(0.0) / 140.0).clamp(0.0, 1.0))
                    .label(bpm_label);
                f.render_widget(bpm_gauge, chunks[3]);

                // 5. Help / Footer
                let help_text = "Controls:\n[N / Right] Force Next Vibe   [L] Lock/Unlock Current Vibe   [B] Blackout   [Q / Esc] Quit";
                let help_para = Paragraph::new(help_text)
                    .style(Style::default().fg(Color::DarkGray))
                    .block(Block::default().borders(Borders::ALL).title("Keyboard Shortcuts"));
                f.render_widget(help_para, chunks[4]);
            })?;
        }

        Ok(())
    }
}

