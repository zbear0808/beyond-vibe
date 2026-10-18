use crate::beyond_osc::BeyondClient;
use crate::config::CueCell;
use std::collections::HashSet;

#[derive(Default, Debug)]
pub struct Janitor {
    active_rhythm_cues: HashSet<CueCell>,
    active_flash_cue: Option<CueCell>,
    active_overlay_release_script: Option<String>,
}

impl Janitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn trigger_exclusive_rhythm(&mut self, new_cell: CueCell, client: &BeyondClient) {
        // Stop all existing rhythm cues
        for old_cell in self.active_rhythm_cues.drain() {
            if old_cell != new_cell {
                client.stop_cue(old_cell.page, old_cell.row, old_cell.col);
            }
        }

        // Start new cue
        client.start_cue(new_cell.page, new_cell.row, new_cell.col);
        self.active_rhythm_cues.insert(new_cell);
    }

    pub fn stop_all_rhythm(&mut self, client: &BeyondClient) {
        for cell in self.active_rhythm_cues.drain() {
            client.stop_cue(cell.page, cell.row, cell.col);
        }
    }

    pub fn trigger_flash_solo(&mut self, cell: CueCell, client: &BeyondClient) {
        // Stop rhythm cues temporarily while flash is active
        for r_cell in &self.active_rhythm_cues {
            client.stop_cue(r_cell.page, r_cell.row, r_cell.col);
        }
        client.start_cue(cell.page, cell.row, cell.col);
        self.active_flash_cue = Some(cell);
    }

    pub fn release_flash_solo(&mut self, client: &BeyondClient) {
        if let Some(cell) = self.active_flash_cue.take() {
            client.stop_cue(cell.page, cell.row, cell.col);
        }
        // Restore rhythm cues if any were active
        for r_cell in &self.active_rhythm_cues {
            client.start_cue(r_cell.page, r_cell.row, r_cell.col);
        }
    }

    pub fn trigger_flash_overlay(&mut self, on_press: &str, on_release: &str, client: &BeyondClient) {
        client.send_pangoscript(on_press);
        self.active_overlay_release_script = Some(on_release.to_string());
    }

    pub fn release_flash_overlay(&mut self, client: &BeyondClient) {
        if let Some(script) = self.active_overlay_release_script.take() {
            client.send_pangoscript(&script);
        }
    }

    pub fn active_cues_count(&self) -> usize {
        self.active_rhythm_cues.len() + if self.active_flash_cue.is_some() { 1 } else { 0 }
    }

    pub fn blackout_all(&mut self, client: &BeyondClient) {
        self.active_rhythm_cues.clear();
        self.active_flash_cue = None;
        self.active_overlay_release_script = None;
        client.blackout();
    }
}

