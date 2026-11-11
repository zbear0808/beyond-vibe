use crate::config::CueCell;
use rosc::{encoder, OscMessage, OscPacket, OscType};
use std::cell::{Cell, RefCell};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Duration;

/// Sends commands to BEYOND's OSC-to-PangoScript gateway (`/beyond/general/<Command>`).
pub struct BeyondClient {
    socket: UdpSocket,
    target: SocketAddr,
    grid_columns: u8,
    send_errors: Cell<u64>,
    last_error: RefCell<Option<String>>,
}

impl BeyondClient {
    pub fn new(ip: &str, port: u16, grid_columns: u8) -> Result<Self, String> {
        let target = (ip, port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .ok_or_else(|| format!("'{}:{}' is not a valid BEYOND address", ip, port))?;
        let bind_addr = if target.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
        let socket = UdpSocket::bind(bind_addr).map_err(|e| format!("Could not open UDP socket: {}", e))?;
        // Blocking with a short timeout: a full send buffer waits briefly instead of silently dropping a StopCue.
        let _ = socket.set_write_timeout(Some(Duration::from_millis(5)));
        if target.is_ipv4() {
            let _ = socket.set_broadcast(true);
        }
        Ok(Self {
            socket,
            target,
            grid_columns,
            send_errors: Cell::new(0),
            last_error: RefCell::new(None),
        })
    }

    pub fn target(&self) -> SocketAddr {
        self.target
    }

    pub fn send_errors(&self) -> u64 {
        self.send_errors.get()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.borrow().clone()
    }

    pub fn send(&self, addr: &str, args: Vec<OscType>) {
        let packet = OscPacket::Message(OscMessage {
            addr: addr.to_string(),
            args,
        });
        let result = encoder::encode(&packet)
            .map_err(|e| e.to_string())
            .and_then(|buf| self.socket.send_to(&buf, self.target).map(|_| ()).map_err(|e| e.to_string()));
        if let Err(e) = result {
            self.send_errors.set(self.send_errors.get() + 1);
            *self.last_error.borrow_mut() = Some(format!("{}: {}", addr, e));
        }
    }

    /// Runs one or more PangoScript statements (separated by ';' or newlines) through the OSC gateway.
    pub fn run_script(&self, script: &str) {
        for statement in split_statements(script) {
            if let Some((addr, args)) = parse_statement(&statement) {
                self.send(&addr, args);
            }
        }
    }

    pub fn start_cue(&self, cell: CueCell) {
        self.send("/beyond/general/StartCue", self.cue_args(cell));
    }

    pub fn stop_cue(&self, cell: CueCell) {
        self.send("/beyond/general/StopCueNow", self.cue_args(cell));
    }

    pub fn stop_all_now(&self) {
        self.send("/beyond/general/StopAllNow", vec![]);
    }

    pub fn blackout(&self) {
        self.send("/beyond/general/BlackOut", vec![]);
    }

    /// Maps a 0..127 MIDI CC onto `min..max` and sends it as a float.
    pub fn set_slider(&self, target: &str, min: f32, max: f32, midi_val: u8) {
        let target = target.trim();
        if target.is_empty() {
            return;
        }
        let value = min + (max - min) * (midi_val.min(127) as f32 / 127.0);
        let addr = if target.starts_with('/') {
            target.to_string()
        } else {
            format!("/beyond/general/{}", target)
        };
        self.send(&addr, vec![OscType::Float(value)]);
    }

    fn cue_args(&self, cell: CueCell) -> Vec<OscType> {
        vec![
            OscType::Int(cell.page as i32),
            OscType::Int(cell.cue_index(self.grid_columns) as i32),
        ]
    }
}

/// Splits on ';' and newlines that are not inside quotes, dropping blanks and `//` comments.
fn split_statements(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in script.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            ';' | '\n' | '\r' if !in_quotes => out.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    out.push(current);
    out.into_iter()
        .map(|s| strip_comment(&s).trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn strip_comment(s: &str) -> &str {
    let mut in_quotes = false;
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        match bytes[i] {
            b'"' => in_quotes = !in_quotes,
            b'/' if !in_quotes && bytes.get(i + 1) == Some(&b'/') => return &s[..i],
            _ => {}
        }
    }
    s
}

/// `Command a, b, c` -> (`/beyond/general/Command`, [a, b, c]); `/raw/address a, b` is passed through.
fn parse_statement(statement: &str) -> Option<(String, Vec<OscType>)> {
    let statement = statement.trim();
    let (head, rest) = match statement.find(char::is_whitespace) {
        Some(i) => (&statement[..i], statement[i..].trim()),
        None => (statement, ""),
    };
    if head.is_empty() {
        return None;
    }
    let addr = if head.starts_with('/') {
        head.to_string()
    } else {
        format!("/beyond/general/{}", head)
    };
    let args = split_args(rest).iter().map(|a| parse_arg(a)).collect();
    Some((addr, args))
}

fn split_args(rest: &str) -> Vec<String> {
    if rest.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in rest.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            ',' if !in_quotes => out.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    out.push(current);
    out.into_iter().map(|s| s.trim().to_string()).collect()
}

fn parse_arg(raw: &str) -> OscType {
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        return OscType::String(raw[1..raw.len() - 1].to_string());
    }
    let lower = raw.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        if let Ok(v) = i64::from_str_radix(hex, 16) {
            return OscType::Int(v as i32);
        }
    }
    if let Ok(v) = raw.parse::<i32>() {
        return OscType::Int(v);
    }
    if raw.contains(['.', 'e', 'E']) {
        if let Ok(v) = raw.parse::<f32>() {
            return OscType::Float(v);
        }
    }
    OscType::String(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typed_arguments() {
        let (addr, args) = parse_statement("RGBA 255.0, 0, 0x0000FF, \"a, b\"").unwrap();
        assert_eq!(addr, "/beyond/general/RGBA");
        assert_eq!(
            args,
            vec![
                OscType::Float(255.0),
                OscType::Int(0),
                OscType::Int(255),
                OscType::String("a, b".into())
            ]
        );
    }

    #[test]
    fn raw_osc_addresses_pass_through() {
        let (addr, args) = parse_statement("/beyond/master/livecontrol/sizey 50.0").unwrap();
        assert_eq!(addr, "/beyond/master/livecontrol/sizey");
        assert_eq!(args, vec![OscType::Float(50.0)]);
    }

    #[test]
    fn splits_multiple_statements_and_strips_comments() {
        let s = split_statements("ColorOn 1.0; BlackOut // reset\n\nMasterPause Toggle");
        assert_eq!(s, vec!["ColorOn 1.0", "BlackOut", "MasterPause Toggle"]);
    }

    #[test]
    fn no_arg_command() {
        let (addr, args) = parse_statement("StopAllNow").unwrap();
        assert_eq!(addr, "/beyond/general/StopAllNow");
        assert!(args.is_empty());
    }
}
