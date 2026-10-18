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
        // PangoScript: StartCue Page, Col, Row (1-based)
        self.send_pangoscript(&format!("StartCue {}, {}, {}", page, col, row));
    }

    pub fn stop_cue(&self, page: u8, row: u8, col: u8) {
        // PangoScript: StopCue Page, Col, Row (1-based)
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

