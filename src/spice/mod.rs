//! SPICE client core: channel lifecycle, common messages and the `SpiceSession`
//! facade used by the MCP tools.
//!
//! One SPICE channel is one connection; the main channel is opened first and
//! kept for the lifetime of the session, child channels are opened on demand
//! (one operation per connection, as the tools use a fresh session per call).

pub mod display;
pub mod inputs;
pub mod link;
pub mod proto;

use std::io::{ErrorKind, Read, Write};
use std::thread;
use std::time::{Duration, Instant};

use crate::libvirt::SpiceEndpoint;
use link::Stream;
use proto::Reader;

/// How long to wait for the TCP connection to a SPICE endpoint.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Read timeout for the link handshake and for the main channel during connect.
const HANDSHAKE_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for `SPICE_MSG_MAIN_INIT` after the main channel is linked.
const MAIN_INIT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for `SPICE_MSG_MAIN_MOUSE_MODE` after requesting client mode.
const MOUSE_MODE_CONFIRM_TIMEOUT: Duration = Duration::from_millis(200);
/// Hard cap on the whole screenshot capture.
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(5);
/// Read slice used while waiting for display updates.
const SCREENSHOT_POLL: Duration = Duration::from_millis(100);
/// Read slice used to consume pending server messages after sending input.
const DRAIN_POLL: Duration = Duration::from_millis(20);
/// Size of `SpiceMiniDataHeader` (type u16, size u32).
const MINI_HEADER_LEN: usize = 6;
/// Refuse absurd message sizes instead of allocating gigabytes.
const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Gap between the events of one click or key press.
const EVENT_GAP: Duration = Duration::from_millis(15);
/// Gap after positioning the pointer before pressing a button.
const CLICK_GAP: Duration = Duration::from_millis(20);
/// Gap between the two clicks of a double click.
const DOUBLE_CLICK_GAP: Duration = Duration::from_millis(80);
/// Gap between interpolated positions of a drag.
const DRAG_GAP: Duration = Duration::from_millis(15);
/// Number of interpolated positions sent between the drag endpoints.
const DRAG_STEPS: u32 = 8;

/// Error text returned by [`Channel::recv_msg`] when the read timed out.
/// A timeout is not fatal: the server simply had nothing to say.
pub const RECV_TIMEOUT_ERROR: &str = "SPICE channel read timed out";

/// Mouse buttons understood by the MCP tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// Wheel direction for [`SpiceSession::mouse_scroll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDir {
    Up,
    Down,
}

/// A captured screen as PNG bytes.
#[derive(Debug, Clone)]
pub struct PngImage {
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

/// One open SPICE channel: a stream plus the server's ack-window state.
///
/// `send_msg` wraps the body in a `SpiceMiniDataHeader`; `recv_msg` handles the
/// common messages (`SET_ACK`, `PING`, `NOTIFY`) internally and only returns
/// channel-specific messages.
pub struct Channel {
    stream: Stream,
    channel_type: u8,
    connection_id: u32,
    ack_window: u32,
    msgs_until_ack: u32,
    /// Bytes read from the socket that belong to unfinished messages.
    pending: Vec<u8>,
}

impl Channel {
    pub fn new(stream: Stream, channel_type: u8, connection_id: u32) -> Channel {
        Channel {
            stream,
            channel_type,
            connection_id,
            ack_window: 0,
            msgs_until_ack: 0,
            pending: Vec::new(),
        }
    }

    pub fn channel_type(&self) -> u8 {
        self.channel_type
    }

    pub fn connection_id(&self) -> u32 {
        self.connection_id
    }

    fn name(&self) -> &'static str {
        match self.channel_type {
            proto::CHANNEL_MAIN => "main",
            proto::CHANNEL_DISPLAY => "display",
            proto::CHANNEL_INPUTS => "inputs",
            proto::CHANNEL_CURSOR => "cursor",
            _ => "unknown",
        }
    }

    /// Send one message: `SpiceMiniDataHeader` (type u16, size u32) + body.
    pub fn send_msg(&mut self, msg_type: u16, payload: &[u8]) -> Result<(), String> {
        let mut header = [0u8; MINI_HEADER_LEN];
        header[0..2].copy_from_slice(&msg_type.to_le_bytes());
        header[2..6].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        self.stream
            .write_all(&header)
            .map_err(|e| format!("SPICE {} channel write failed: {}", self.name(), e))?;
        if !payload.is_empty() {
            self.stream
                .write_all(payload)
                .map_err(|e| format!("SPICE {} channel write failed: {}", self.name(), e))?;
        }
        self.stream
            .flush()
            .map_err(|e| format!("SPICE {} channel flush failed: {}", self.name(), e))
    }

    /// Receive the next channel-specific message, or a timeout error.
    pub fn recv_msg(&mut self, timeout: Duration) -> Result<(u16, Vec<u8>), String> {
        match self.recv_msg_opt(timeout)? {
            Some(msg) => Ok(msg),
            None => Err(RECV_TIMEOUT_ERROR.to_string()),
        }
    }

    /// Receive the next channel-specific message, or `None` if the server sent
    /// nothing within `timeout`. Common messages are handled here.
    pub fn recv_msg_opt(&mut self, timeout: Duration) -> Result<Option<(u16, Vec<u8>)>, String> {
        loop {
            while self.pending.len() < MINI_HEADER_LEN {
                if self.fill(timeout)? == 0 {
                    return Ok(None);
                }
            }
            let msg_type = u16::from_le_bytes([self.pending[0], self.pending[1]]);
            let size =
                u32::from_le_bytes([self.pending[2], self.pending[3], self.pending[4], self.pending[5]])
                    as usize;
            if size > MAX_MESSAGE_SIZE {
                return Err(format!(
                    "SPICE {} channel message type {} claims {} bytes, refusing",
                    self.name(),
                    msg_type,
                    size
                ));
            }
            while self.pending.len() < MINI_HEADER_LEN + size {
                if self.fill(timeout)? == 0 {
                    return Err(format!(
                        "SPICE {} channel stalled: message type {} announced {} bytes, {} arrived",
                        self.name(),
                        msg_type,
                        size,
                        self.pending.len() - MINI_HEADER_LEN
                    ));
                }
            }
            let body = self.pending[MINI_HEADER_LEN..MINI_HEADER_LEN + size].to_vec();
            self.pending.drain(..MINI_HEADER_LEN + size);

            let mut channel_message = None;
            match msg_type {
                proto::MSG_SET_ACK => {
                    let mut r = Reader::new(&body);
                    let generation = r.u32();
                    let window = r.u32();
                    self.ack_window = window;
                    self.msgs_until_ack = window;
                    let mut ack = proto::Writer::new();
                    ack.u32(generation);
                    self.send_msg(proto::MSGC_ACK_SYNC, ack.as_slice())?;
                }
                proto::MSG_PING => {
                    // {id u32, timestamp u64, data...}: echo id and timestamp.
                    let n = body.len().min(12);
                    self.send_msg(proto::MSGC_PONG, &body[..n])?;
                }
                proto::MSG_NOTIFY => {
                    let mut r = Reader::new(&body);
                    let _time_stamp = r.u64();
                    let severity = r.u32();
                    let _visibility = r.u32();
                    let _what = r.u32();
                    let message_len = r.u32() as usize;
                    let text = String::from_utf8_lossy(r.bytes(message_len)).into_owned();
                    eprintln!("SPICE notify (severity {}): {}", severity, text);
                }
                proto::MSG_DISCONNECTING => {
                    let mut r = Reader::new(&body);
                    let _time_stamp = r.u64();
                    let reason = r.u32();
                    return Err(format!(
                        "SPICE server disconnected the {} channel (reason {})",
                        self.name(),
                        reason
                    ));
                }
                _ => channel_message = Some((msg_type, body)),
            }

            // Every received message counts against the ack window.
            if self.ack_window > 0 {
                if self.msgs_until_ack > 0 {
                    self.msgs_until_ack -= 1;
                }
                if self.msgs_until_ack == 0 {
                    self.msgs_until_ack = self.ack_window;
                    self.send_msg(proto::MSGC_ACK, &[])?;
                }
            }

            if let Some(msg) = channel_message {
                return Ok(Some(msg));
            }
        }
    }

    /// Read once from the socket; returns the number of bytes read, or 0 on
    /// timeout (an idle server is not an error).
    fn fill(&mut self, timeout: Duration) -> Result<usize, String> {
        self.stream.set_read_timeout(timeout)?;
        let mut chunk = [0u8; 8192];
        match self.stream.read(&mut chunk) {
            Ok(0) => Err(format!(
                "SPICE {} channel closed by the server",
                self.name()
            )),
            Ok(n) => {
                self.pending.extend_from_slice(&chunk[..n]);
                Ok(n)
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                Ok(0)
            }
            Err(e) => Err(format!("SPICE {} channel read failed: {}", self.name(), e)),
        }
    }
}

/// Owned copy of the SPICE endpoint, so a session can open child channels
/// without borrowing the caller's `SpiceEndpoint`.
enum SessionEndpoint {
    Tcp { host: String, port: u16 },
    Unix { path: String },
}

impl SessionEndpoint {
    fn from_endpoint(ep: &SpiceEndpoint) -> SessionEndpoint {
        match ep {
            SpiceEndpoint::Tcp { host, port } => SessionEndpoint::Tcp {
                host: host.clone(),
                port: *port,
            },
            SpiceEndpoint::Unix { path } => SessionEndpoint::Unix { path: path.clone() },
        }
    }

    fn connect(&self) -> Result<Stream, String> {
        match self {
            SessionEndpoint::Tcp { host, port } => {
                Stream::connect_tcp(host, *port, CONNECT_TIMEOUT, HANDSHAKE_READ_TIMEOUT)
            }
            SessionEndpoint::Unix { path } => {
                Stream::connect_unix(path, HANDSHAKE_READ_TIMEOUT, CONNECT_TIMEOUT)
            }
        }
    }

    fn describe(&self) -> String {
        match self {
            SessionEndpoint::Tcp { host, port } => format!("{}:{}", host, port),
            SessionEndpoint::Unix { path } => path.clone(),
        }
    }
}

/// A connected SPICE session: the main channel stays open for the whole
/// lifetime, child channels are opened per operation.
pub struct SpiceSession {
    endpoint: SessionEndpoint,
    password: String,
    main: Channel,
    session_id: u32,
    /// Current mouse mode as negotiated with the server (`SPICE_MOUSE_MODE_*`).
    mouse_mode: u32,
    /// Last pointer position we sent, used to compute deltas in server mode.
    pointer_x: u32,
    pointer_y: u32,
}

impl SpiceSession {
    /// Connect the main channel, read `SPICE_MSG_MAIN_INIT` and ask for client
    /// mouse mode when the server offers it.
    pub fn connect(ep: &SpiceEndpoint, password: &str) -> Result<SpiceSession, String> {
        let endpoint = SessionEndpoint::from_endpoint(ep);
        let mut stream = endpoint
            .connect()
            .map_err(|e| format!("SPICE {}: {}", endpoint.describe(), e))?;
        link::link_connect(&mut stream, 0, proto::CHANNEL_MAIN, &[], password)?;
        let mut main = Channel::new(stream, proto::CHANNEL_MAIN, 0);

        let deadline = Instant::now() + MAIN_INIT_TIMEOUT;
        let init = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "SPICE main channel: no MAIN_INIT from {} within {}s",
                    endpoint.describe(),
                    MAIN_INIT_TIMEOUT.as_secs()
                ));
            }
            match main.recv_msg_opt(remaining)? {
                Some((proto::MSG_MAIN_INIT, body)) => break body,
                // Anything else (mouse mode, agent tokens, ...) can arrive
                // before MAIN_INIT and does not matter yet.
                Some(_) => continue,
                None => {
                    return Err(format!(
                        "SPICE main channel: no MAIN_INIT from {} within {}s",
                        endpoint.describe(),
                        MAIN_INIT_TIMEOUT.as_secs()
                    ))
                }
            }
        };

        if init.len() < proto::MAIN_INIT_LEN {
            return Err(format!(
                "SPICE MAIN_INIT is {} bytes, expected {}",
                init.len(),
                proto::MAIN_INIT_LEN
            ));
        }
        let mut r = Reader::new(&init);
        let session_id = r.u32();
        let _display_channels_hint = r.u32();
        let supported_mouse_modes = r.u32();
        let current_mouse_mode = r.u32();
        let _agent_connected = r.u32();
        let _agent_tokens = r.u32();
        let _multi_media_time = r.u32();
        let _ram_hint = r.u32();

        let mut mouse_mode = current_mouse_mode & proto::MOUSE_MODE_MASK;
        if supported_mouse_modes & proto::MOUSE_MODE_CLIENT != 0
            && mouse_mode != proto::MOUSE_MODE_CLIENT
        {
            // flags16 mouse_mode: the server only reads the low 16 bits.
            let mut w = proto::Writer::new();
            w.u16(proto::MOUSE_MODE_CLIENT as u16);
            main.send_msg(proto::MSGC_MAIN_MOUSE_MODE_REQUEST, w.as_slice())?;
            // The server confirms with SPICE_MSG_MAIN_MOUSE_MODE when it grants
            // the switch; if it stays silent we keep the server mouse mode.
            let deadline = Instant::now() + MOUSE_MODE_CONFIRM_TIMEOUT;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match main.recv_msg_opt(remaining)? {
                    Some((proto::MSG_MAIN_MOUSE_MODE, body)) => {
                        let mut mr = Reader::new(&body);
                        let _supported = mr.u16();
                        let current = mr.u16();
                        mouse_mode = u32::from(current) & proto::MOUSE_MODE_MASK;
                        break;
                    }
                    Some(_) => continue,
                    None => break,
                }
            }
        }

        Ok(SpiceSession {
            endpoint,
            password: password.to_string(),
            main,
            session_id,
            mouse_mode,
            pointer_x: 0,
            pointer_y: 0,
        })
    }

    /// Capture the primary surface as a PNG.
    ///
    /// Opens a display channel, asks for uncompressed bitmaps, then pumps
    /// messages until a complete frame is available and the channel has been
    /// quiet for `wait_ms`. Gives up after 5 seconds.
    pub fn screenshot(&mut self, wait_ms: u32) -> Result<PngImage, String> {
        let mut ch = self.open_channel(proto::CHANNEL_DISPLAY)?;
        ch.send_msg(
            proto::MSGC_DISPLAY_INIT,
            &display::display_init_payload(1),
        )?;
        ch.send_msg(
            proto::MSGC_DISPLAY_PREFERRED_COMPRESSION,
            &display::preferred_compression_payload(proto::IMAGE_COMPRESSION_OFF),
        )?;

        let mut capture = display::DisplayCapture::new();
        let quiet = Duration::from_millis(u64::from(wait_ms.max(1)));
        let deadline = Instant::now() + SCREENSHOT_TIMEOUT;
        let mut last_message = Instant::now();
        let mut frame: Option<PngImage> = None;

        loop {
            if Instant::now() >= deadline {
                break;
            }
            match ch.recv_msg_opt(SCREENSHOT_POLL)? {
                Some((msg_type, body)) => {
                    last_message = Instant::now();
                    let mut r = Reader::new(&body);
                    capture.handle_message(msg_type, &body, &mut r)?;
                }
                None => {
                    if last_message.elapsed() >= quiet {
                        if let Some(png) = capture.png() {
                            frame = Some(png);
                            break;
                        }
                    }
                }
            }
        }

        frame.or_else(|| capture.png()).ok_or_else(|| {
            format!(
                "SPICE display channel produced no complete frame within {}s",
                SCREENSHOT_TIMEOUT.as_secs()
            )
        })
    }

    /// Type text using the US keyboard layout. `interval_ms` is the delay
    /// between keystrokes.
    pub fn type_text(&mut self, text: &str, interval_ms: u64) -> Result<(), String> {
        let events = inputs::type_text_events(text, interval_ms);
        if events.is_empty() {
            return Ok(());
        }
        let mut ch = self.open_channel(proto::CHANNEL_INPUTS)?;
        self.send_events(&mut ch, &events)
    }

    /// Press a key combination such as `"ctrl+alt+t"`.
    pub fn key_press(&mut self, combo: &str) -> Result<(), String> {
        let events = inputs::key_combo_events(combo)?;
        if events.is_empty() {
            return Ok(());
        }
        let mut ch = self.open_channel(proto::CHANNEL_INPUTS)?;
        self.send_events(&mut ch, &events)
    }

    /// Move the pointer to `(x, y)`: absolute in client mouse mode, relative to
    /// the previous position in server mouse mode.
    pub fn mouse_move(&mut self, x: u32, y: u32) -> Result<(), String> {
        let mut ch = self.open_channel(proto::CHANNEL_INPUTS)?;
        self.send_pointer(&mut ch, x, y, 0)?;
        self.drain(&mut ch)
    }

    /// Click a button, optionally moving the pointer first. `double` sends two
    /// clicks close enough together for the guest to see a double click.
    pub fn mouse_click(
        &mut self,
        button: Button,
        x: Option<u32>,
        y: Option<u32>,
        double: bool,
    ) -> Result<(), String> {
        let position = match (x, y) {
            (Some(x), Some(y)) => Some((x, y)),
            (None, None) => None,
            _ => return Err("mouse_click needs both x and y, or neither".to_string()),
        };

        let mut ch = self.open_channel(proto::CHANNEL_INPUTS)?;
        if let Some((x, y)) = position {
            self.send_pointer(&mut ch, x, y, 0)?;
            thread::sleep(CLICK_GAP);
        }

        let clicks = if double { 2 } else { 1 };
        for i in 0..clicks {
            if i > 0 {
                thread::sleep(DOUBLE_CLICK_GAP);
            }
            let mask = button_mask(button);
            ch.send_msg(
                proto::MSGC_INPUTS_MOUSE_PRESS,
                &inputs::mouse_press_payload(button, mask),
            )?;
            thread::sleep(EVENT_GAP);
            ch.send_msg(
                proto::MSGC_INPUTS_MOUSE_RELEASE,
                &inputs::mouse_release_payload(button, 0),
            )?;
        }
        self.drain(&mut ch)
    }

    /// Turn the wheel `clicks` times. Wheel buttons are not part of
    /// `buttons_state`, so their payloads are built here.
    pub fn mouse_scroll(&mut self, dir: ScrollDir, clicks: u32) -> Result<(), String> {
        if clicks == 0 {
            return Ok(());
        }
        let button = match dir {
            ScrollDir::Up => proto::MOUSE_BUTTON_UP,
            ScrollDir::Down => proto::MOUSE_BUTTON_DOWN,
        };
        let mut ch = self.open_channel(proto::CHANNEL_INPUTS)?;
        for _ in 0..clicks {
            ch.send_msg(
                proto::MSGC_INPUTS_MOUSE_PRESS,
                &mouse_button_payload(button, 0),
            )?;
            thread::sleep(EVENT_GAP);
            ch.send_msg(
                proto::MSGC_INPUTS_MOUSE_RELEASE,
                &mouse_button_payload(button, 0),
            )?;
            thread::sleep(EVENT_GAP);
        }
        self.drain(&mut ch)
    }

    /// Press the left button at `from`, move to `to` in steps, release.
    pub fn mouse_drag(&mut self, from: (u32, u32), to: (u32, u32)) -> Result<(), String> {
        let mut ch = self.open_channel(proto::CHANNEL_INPUTS)?;
        let mask = button_mask(Button::Left);

        self.send_pointer(&mut ch, from.0, from.1, 0)?;
        thread::sleep(CLICK_GAP);
        ch.send_msg(
            proto::MSGC_INPUTS_MOUSE_PRESS,
            &inputs::mouse_press_payload(Button::Left, mask),
        )?;
        thread::sleep(EVENT_GAP);

        for step in 1..=DRAG_STEPS {
            let x = interpolate(from.0, to.0, step, DRAG_STEPS);
            let y = interpolate(from.1, to.1, step, DRAG_STEPS);
            self.send_pointer(&mut ch, x, y, mask)?;
            thread::sleep(DRAG_GAP);
        }
        self.send_pointer(&mut ch, to.0, to.1, mask)?;
        thread::sleep(EVENT_GAP);

        ch.send_msg(
            proto::MSGC_INPUTS_MOUSE_RELEASE,
            &inputs::mouse_release_payload(Button::Left, 0),
        )?;
        self.drain(&mut ch)
    }

    /// Send an already-built event sequence (delay_ms, msg_type, payload).
    fn send_events(
        &mut self,
        ch: &mut Channel,
        events: &[(u16, u16, Vec<u8>)],
    ) -> Result<(), String> {
        for (delay_ms, msg_type, payload) in events {
            if *delay_ms > 0 {
                thread::sleep(Duration::from_millis(u64::from(*delay_ms)));
            }
            ch.send_msg(*msg_type, payload)?;
        }
        self.drain(ch)
    }

    /// Send one pointer message in the form the negotiated mouse mode expects.
    fn send_pointer(
        &mut self,
        ch: &mut Channel,
        x: u32,
        y: u32,
        buttons: u16,
    ) -> Result<(), String> {
        if self.mouse_mode == proto::MOUSE_MODE_CLIENT {
            let payload = inputs::mouse_position_payload(x, y, buttons);
            ch.send_msg(proto::MSGC_INPUTS_MOUSE_POSITION, &payload)?;
        } else {
            let dx = clamp_i32(x as i64 - self.pointer_x as i64);
            let dy = clamp_i32(y as i64 - self.pointer_y as i64);
            let payload = inputs::mouse_motion_payload(dx, dy, buttons);
            ch.send_msg(proto::MSGC_INPUTS_MOUSE_MOTION, &payload)?;
        }
        self.pointer_x = x;
        self.pointer_y = y;
        Ok(())
    }

    /// Consume messages the server already sent (acks, pings, inputs
    /// notifications) so the ack window does not stall the channel.
    fn drain(&mut self, ch: &mut Channel) -> Result<(), String> {
        while ch.recv_msg_opt(DRAIN_POLL)?.is_some() {}
        Ok(())
    }

    /// Open a child channel: fresh connection, link handshake with the main
    /// channel's session id.
    fn open_channel(&self, channel_type: u8) -> Result<Channel, String> {
        let mut stream = self.endpoint.connect().map_err(|e| {
            format!("SPICE {} ({}): {}", channel_name(channel_type), channel_type, e)
        })?;
        link::link_connect(
            &mut stream,
            self.session_id,
            channel_type,
            &[],
            &self.password,
        )?;
        Ok(Channel::new(stream, channel_type, self.session_id))
    }
}

fn channel_name(channel_type: u8) -> &'static str {
    match channel_type {
        proto::CHANNEL_MAIN => "main",
        proto::CHANNEL_DISPLAY => "display",
        proto::CHANNEL_INPUTS => "inputs",
        proto::CHANNEL_CURSOR => "cursor",
        _ => "unknown",
    }
}

/// `SpiceMouseButtonMask` for the tools' three buttons.
fn button_mask(button: Button) -> u16 {
    match button {
        Button::Left => proto::MOUSE_BUTTON_MASK_LEFT,
        Button::Middle => proto::MOUSE_BUTTON_MASK_MIDDLE,
        Button::Right => proto::MOUSE_BUTTON_MASK_RIGHT,
    }
}

/// Wire body of `SPICE_MSGC_INPUTS_MOUSE_PRESS` / `_RELEASE`.
fn mouse_button_payload(button: u8, buttons_state: u16) -> Vec<u8> {
    let mut w = proto::Writer::new();
    w.u8(button);
    w.u16(buttons_state);
    w.into_vec()
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// Linear interpolation between two coordinates (inclusive of `step == steps`).
fn interpolate(from: u32, to: u32, step: u32, steps: u32) -> u32 {
    let delta = to as i64 - from as i64;
    (from as i64 + delta * step as i64 / steps as i64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream as RawUnixStream;

    fn write_msg(stream: &RawUnixStream, msg_type: u16, body: &[u8]) {
        let mut w = &*stream;
        let mut header = [0u8; MINI_HEADER_LEN];
        header[0..2].copy_from_slice(&msg_type.to_le_bytes());
        header[2..6].copy_from_slice(&(body.len() as u32).to_le_bytes());
        w.write_all(&header).expect("write header");
        w.write_all(body).expect("write body");
    }

    fn read_msg(stream: &RawUnixStream) -> (u16, Vec<u8>) {
        let mut r = &*stream;
        let mut header = [0u8; MINI_HEADER_LEN];
        r.read_exact(&mut header).expect("read header");
        let msg_type = u16::from_le_bytes([header[0], header[1]]);
        let size = u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as usize;
        let mut body = vec![0u8; size];
        r.read_exact(&mut body).expect("read body");
        (msg_type, body)
    }

    #[test]
    fn channel_replies_to_set_ack_and_ping() {
        let (client, server) = RawUnixStream::pair().expect("socketpair");
        server
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("read timeout");
        let mut ch = Channel::new(Stream::Unix(client), proto::CHANNEL_MAIN, 0);

        // SPICE_MSG_SET_ACK {generation 5, window 2}
        let mut ack = proto::Writer::new();
        ack.u32(5);
        ack.u32(2);
        write_msg(&server, proto::MSG_SET_ACK, ack.as_slice());
        // SPICE_MSG_PING {id 9, timestamp 1234, extra 4 bytes}
        let mut ping = proto::Writer::new();
        ping.u32(9);
        ping.u64(1234);
        ping.u32(0xdead_beef);
        write_msg(&server, proto::MSG_PING, ping.as_slice());
        // A channel message: MAIN_INIT (contents do not matter here).
        write_msg(&server, proto::MSG_MAIN_INIT, &[0u8; proto::MAIN_INIT_LEN]);

        let (msg_type, body) = ch
            .recv_msg(Duration::from_secs(1))
            .expect("channel message");
        assert_eq!(msg_type, proto::MSG_MAIN_INIT);
        assert_eq!(body.len(), proto::MAIN_INIT_LEN);

        // The client must have answered ACK_SYNC(5), PONG(id 9, timestamp 1234)
        // and, because the window is 2, a bare ACK.
        let (t, b) = read_msg(&server);
        assert_eq!(t, proto::MSGC_ACK_SYNC);
        assert_eq!(b, 5u32.to_le_bytes().to_vec());
        let (t, b) = read_msg(&server);
        assert_eq!(t, proto::MSGC_PONG);
        assert_eq!(b.len(), 12);
        assert_eq!(b, ping.as_slice()[..12].to_vec());
        let (t, b) = read_msg(&server);
        assert_eq!(t, proto::MSGC_ACK);
        assert!(b.is_empty());
    }

    #[test]
    fn channel_times_out_without_traffic() {
        let (client, _server) = RawUnixStream::pair().expect("socketpair");
        let mut ch = Channel::new(Stream::Unix(client), proto::CHANNEL_INPUTS, 0);
        assert!(ch
            .recv_msg_opt(Duration::from_millis(20))
            .expect("no error")
            .is_none());
        assert_eq!(
            ch.recv_msg(Duration::from_millis(20)),
            Err(RECV_TIMEOUT_ERROR.to_string())
        );
    }

    #[test]
    fn channel_reports_disconnecting() {
        let (client, server) = RawUnixStream::pair().expect("socketpair");
        let mut ch = Channel::new(Stream::Unix(client), proto::CHANNEL_DISPLAY, 0);
        let mut body = proto::Writer::new();
        body.u64(1_234_567);
        body.u32(3);
        write_msg(&server, proto::MSG_DISCONNECTING, body.as_slice());
        let err = ch.recv_msg(Duration::from_secs(1)).expect_err("must fail");
        assert!(err.contains("disconnected"), "got: {}", err);
        assert!(err.contains("reason 3"), "got: {}", err);
    }

    #[test]
    fn channel_send_msg_wraps_mini_header() {
        let (client, server) = RawUnixStream::pair().expect("socketpair");
        let mut ch = Channel::new(Stream::Unix(client), proto::CHANNEL_INPUTS, 0);
        ch.send_msg(proto::MSGC_INPUTS_KEY_DOWN, &[0x1e, 0x00, 0x00, 0x00])
            .expect("send");
        let (t, b) = read_msg(&server);
        assert_eq!(t, proto::MSGC_INPUTS_KEY_DOWN);
        assert_eq!(b, vec![0x1e, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn button_masks_match_protocol_header() {
        assert_eq!(button_mask(Button::Left), 1);
        assert_eq!(button_mask(Button::Middle), 2);
        assert_eq!(button_mask(Button::Right), 4);
    }

    #[test]
    fn mouse_button_payload_is_three_bytes() {
        assert_eq!(
            mouse_button_payload(proto::MOUSE_BUTTON_UP, 0),
            vec![4, 0x00, 0x00]
        );
        assert_eq!(
            mouse_button_payload(proto::MOUSE_BUTTON_LEFT, 1),
            vec![1, 0x01, 0x00]
        );
    }

    #[test]
    fn interpolation_reaches_both_endpoints() {
        assert_eq!(interpolate(0, 100, 0, 8), 0);
        assert_eq!(interpolate(0, 100, 8, 8), 100);
        assert_eq!(interpolate(100, 0, 4, 8), 50);
        assert_eq!(interpolate(10, 20, 1, 10), 11);
        assert_eq!(interpolate(u32::MAX, 0, 1, 2), (u32::MAX / 2) as u32);
    }

    #[test]
    fn clamp_i32_saturates() {
        assert_eq!(clamp_i32(0), 0);
        assert_eq!(clamp_i32(-5), -5);
        assert_eq!(clamp_i32(i64::MIN), i32::MIN);
        assert_eq!(clamp_i32(i64::MAX), i32::MAX);
    }
}
