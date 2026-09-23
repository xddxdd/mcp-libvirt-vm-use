//! SPICE client adapter: an async facade over the ryll/shakenfist renderer.
//!
//! One [`SpiceSession`] owns one `shakenfist_spice_renderer::run_connection`
//! task. The renderer connects every channel the server advertises (main,
//! display, inputs, cursor), pushes `ChannelEvent`s into an mpsc and consumes
//! `InputEvent`s from another; a drain task applies each event to an owned
//! [`SurfaceMirror`], which is where screenshots come from.
//!
//! Sessions are short-lived: every MCP tool call opens one, performs its
//! operation and drops it, which raises the renderer's cancel flag so all
//! channel tasks exit.

pub mod inputs;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use image::{ImageBuffer, ImageFormat, Rgb};
use shakenfist_spice_protocol::{mouse_buttons, ChannelType, ConnectionConfig, MOUSE_MODE_CLIENT};
use shakenfist_spice_renderer::channels::VolumeControl;
use shakenfist_spice_renderer::session::{EVENT_CHANNEL_SIZE, INPUT_CHANNEL_SIZE};
use shakenfist_spice_renderer::{
    run_connection, ByteCounter, ChannelEvent, ChannelSnapshots, DisplaySurface, InputEvent,
    LogConfig, SurfaceMirror, TrafficSink,
};
use tokio::sync::{mpsc, Notify};

use crate::libvirt::SpiceEndpoint;

/// How long [`SpiceSession::connect`] waits for the SPICE session to initialize.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Poll interval while waiting for session events.
const CONNECT_POLL: Duration = Duration::from_millis(10);
/// Extra grace to learn the server's mouse mode once the session is up. The
/// main channel emits `MouseMode` right after `SessionInitialized`.
const MOUSE_MODE_GRACE: Duration = Duration::from_millis(500);

/// Hard cap on the whole screenshot capture.
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(5);
/// Poll interval while waiting for a quiet display.
const SCREENSHOT_POLL: Duration = Duration::from_millis(25);

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

/// Client-side pixmap cache size. The display channel advertises 20 MiB to the
/// server (`SPICE_MSGC_DISPLAY_INIT`), so the cache must be comfortably larger.
const IMAGE_CACHE_CAP_BYTES: usize = 64 * 1024 * 1024;
/// Client-side GLZ dictionary size, well above the 3 MiB window the display
/// channel advertises to the server.
const GLZ_DICTIONARY_CAP_BYTES: usize = 8 * 1024 * 1024;

/// Mouse buttons understood by the MCP tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    /// The `InputEvent::MouseDown` button value: the SPICE button *mask*
    /// (the protocol crate converts it to the wire button id, see
    /// `shakenfist_spice_protocol::messages::MouseButton::mask_to_id`).
    fn mask(self) -> u32 {
        match self {
            Button::Left => mouse_buttons::LEFT,
            Button::Middle => mouse_buttons::MIDDLE,
            Button::Right => mouse_buttons::RIGHT,
        }
    }
}

/// Wheel direction for [`SpiceSession::mouse_scroll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDir {
    Up,
    Down,
}

impl ScrollDir {
    /// The wheel button mask (`UP` = 4, `DOWN` = 5 on the wire).
    fn mask(self) -> u32 {
        match self {
            ScrollDir::Up => mouse_buttons::UP,
            ScrollDir::Down => mouse_buttons::DOWN,
        }
    }
}

/// A captured screen as PNG bytes.
#[derive(Debug, Clone)]
pub struct PngImage {
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

/// A connected SPICE session: one renderer task plus the state a drain task
/// keeps up to date from its events.
pub struct SpiceSession {
    /// Endpoint in `host:port` form, for error messages.
    endpoint: String,
    /// Queue into the renderer's inputs channel.
    input_tx: mpsc::Sender<InputEvent>,
    /// Pixel store and session bookkeeping, written by the drain task.
    state: Arc<Mutex<SessionState>>,
    /// Renderer cancel flag; raising it aborts every channel task.
    cancel: Arc<AtomicBool>,
    /// Last pointer position we sent, used for relative motion and for the
    /// position the renderer attaches to its button messages.
    pointer_x: u32,
    pointer_y: u32,
}

/// Session bookkeeping shared between the drain task and the session methods.
struct SessionState {
    /// Mirrors every draw the display channel sends.
    mirror: SurfaceMirror,
    /// Session id reported by the main channel.
    session_id: Option<u32>,
    /// Channels the server advertised; `None` until `ChannelsAvailable`.
    channels: Option<Vec<ChannelType>>,
    /// Current mouse mode (`SPICE_MOUSE_MODE_SERVER` = 1, `_CLIENT` = 2).
    mouse_mode: Option<u32>,
    /// Number of draws applied to the mirror.
    paints: u64,
    /// When the display channel last sent anything, for the screenshot quiet period.
    last_display_at: Instant,
    /// Error reported by the main or display channel.
    error: Option<String>,
    /// True once every channel task has exited and the event stream ended.
    closed: bool,
}

impl SessionState {
    fn new() -> SessionState {
        SessionState {
            mirror: SurfaceMirror::new(),
            session_id: None,
            channels: None,
            mouse_mode: None,
            paints: 0,
            last_display_at: Instant::now(),
            error: None,
            closed: false,
        }
    }

    /// Apply one renderer event, updating bookkeeping and then the pixel store.
    fn apply(&mut self, event: &ChannelEvent) {
        match event {
            ChannelEvent::SessionInitialized(session_id) => self.session_id = Some(*session_id),
            ChannelEvent::ChannelsAvailable(channels) => {
                self.channels = Some(channels.iter().map(|(kind, _)| *kind).collect());
            }
            ChannelEvent::MouseMode(mode) => self.mouse_mode = Some(*mode),
            ChannelEvent::Error { channel, message } => {
                if is_fatal_channel(*channel) {
                    self.error.get_or_insert_with(|| message.clone());
                }
            }
            ChannelEvent::Disconnected(channel) => {
                if is_fatal_channel(*channel) && self.error.is_none() {
                    self.error = Some(format!("the SPICE {} channel disconnected", channel.name()));
                }
            }
            _ => {}
        }
        if is_draw(event) {
            self.paints += 1;
        }
        if is_display_activity(event) {
            self.last_display_at = Instant::now();
        }
        self.mirror.apply_event(event);
    }
}

/// Channels whose loss ends the usefulness of the session.
fn is_fatal_channel(channel: ChannelType) -> bool {
    matches!(channel, ChannelType::Main | ChannelType::Display)
}

/// Display events that put pixels into the mirror. A surface is created black,
/// so `SurfaceCreated` alone is not a frame.
fn is_draw(event: &ChannelEvent) -> bool {
    matches!(
        event,
        ChannelEvent::ImageReady { .. }
            | ChannelEvent::ImageReadyChroma { .. }
            | ChannelEvent::ImageReadyAlpha { .. }
            | ChannelEvent::FillRect { .. }
            | ChannelEvent::CopyBits { .. }
            | ChannelEvent::Invert { .. }
    )
}

/// Every display event, including the frame bookkeeping around the draws.
/// These restart the screenshot quiet period.
fn is_display_activity(event: &ChannelEvent) -> bool {
    is_draw(event)
        || matches!(
            event,
            ChannelEvent::SurfaceCreated { .. }
                | ChannelEvent::SurfaceDestroyed { .. }
                | ChannelEvent::DisplayMark { .. }
        )
}

/// Protocol-traffic sink for the renderer. This server keeps no bug-report ring
/// buffers, so every message is dropped.
struct NoopTrafficSink {
    start: Instant,
}

impl NoopTrafficSink {
    fn new() -> NoopTrafficSink {
        NoopTrafficSink {
            start: Instant::now(),
        }
    }
}

impl TrafficSink for NoopTrafficSink {
    fn record_sent(
        &self,
        _channel: &'static str,
        _msg_type: u16,
        _msg_name: &'static str,
        _raw: &[u8],
    ) {
    }

    fn record_received(
        &self,
        _channel: &'static str,
        _msg_type: u16,
        _msg_name: &'static str,
        _raw: &[u8],
    ) {
    }

    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Lock the shared session state, ignoring poisoning: the drain task only runs
/// short, panic-free updates under this lock.
fn lock(state: &Mutex<SessionState>) -> MutexGuard<'_, SessionState> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl SpiceSession {
    /// Start a SPICE connection to `ep` and wait for its session to initialize.
    ///
    /// The ryll-based client speaks TCP only; unix-socket endpoints are rejected.
    pub async fn connect(ep: &SpiceEndpoint, password: &str) -> Result<SpiceSession, String> {
        let (host, port) = match ep {
            SpiceEndpoint::Tcp { host, port } => (host.clone(), *port),
            SpiceEndpoint::Unix { path } => {
                return Err(format!(
                    "unix-socket SPICE endpoints are not supported by the ryll-based client \
                     (domain listens on {})",
                    path
                ))
            }
        };
        let endpoint = format!("{}:{}", host, port);
        let config = ConnectionConfig {
            host,
            port,
            // An empty password is "no password": the protocol crate encrypts a
            // single NUL either way.
            password: if password.is_empty() {
                None
            } else {
                Some(password.to_string())
            },
            ..Default::default()
        };

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_SIZE);
        let (input_tx, input_rx) = mpsc::channel(INPUT_CHANNEL_SIZE);
        // USB, WebDAV and resize queues are unused; dropping the senders leaves
        // those select arms idle inside the channel tasks.
        let (_, usb_rx) = mpsc::channel(1);
        let (_, webdav_rx) = mpsc::channel(1);
        let (_, resize_rx) = mpsc::channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let connection_cancel = Arc::clone(&cancel);

        let connection = tokio::spawn(async move {
            run_connection(
                config,
                event_tx,
                Arc::new(Notify::new()),
                input_rx,
                usb_rx,
                webdav_rx,
                Vec::new(),
                None,
                None,
                Arc::new(ByteCounter::new()),
                Arc::new(NoopTrafficSink::new()),
                ChannelSnapshots::new(),
                1,
                resize_rx,
                VolumeControl::new(),
                false,
                LogConfig::default(),
                connection_cancel,
                None,
                None,
                IMAGE_CACHE_CAP_BYTES,
                GLZ_DICTIONARY_CAP_BYTES,
            )
            .await
            .map_err(|error| format!("{:#}", error))
        });

        let state = Arc::new(Mutex::new(SessionState::new()));
        {
            let state = Arc::clone(&state);
            let mut events = event_rx;
            tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    lock(&state).apply(&event);
                }
                lock(&state).closed = true;
            });
        }

        let deadline = Instant::now() + CONNECT_TIMEOUT;
        while lock(&state).session_id.is_none() {
            let reported = lock(&state).error.clone();
            if let Some(error) = reported {
                cancel.store(true, Ordering::Relaxed);
                connection.abort();
                return Err(format!("SPICE {}: {}", endpoint, error));
            }
            if connection.is_finished() {
                cancel.store(true, Ordering::Relaxed);
                let message = match connection.await {
                    Ok(Err(error)) => error,
                    Ok(Ok(())) => "the connection ended before the session initialized".to_string(),
                    Err(error) => format!("connection task failed: {}", error),
                };
                return Err(format!("SPICE {}: {}", endpoint, message));
            }
            if Instant::now() >= deadline {
                cancel.store(true, Ordering::Relaxed);
                connection.abort();
                return Err(format!(
                    "SPICE {}: no session initialization within {}s",
                    endpoint,
                    CONNECT_TIMEOUT.as_secs()
                ));
            }
            tokio::time::sleep(CONNECT_POLL).await;
        }

        // The main channel emits MouseMode immediately after SessionInitialized;
        // if it never arrives, the absolute client mouse mode is assumed, which
        // is also the mode the renderer requests.
        let mode_deadline = Instant::now() + MOUSE_MODE_GRACE;
        while lock(&state).mouse_mode.is_none() && Instant::now() < mode_deadline {
            tokio::time::sleep(CONNECT_POLL).await;
        }

        Ok(SpiceSession {
            endpoint,
            input_tx,
            state,
            cancel,
            pointer_x: 0,
            pointer_y: 0,
        })
    }

    /// Capture the primary surface as a PNG.
    ///
    /// Waits until the display channel has painted at least one frame and has
    /// then been quiet for `wait_ms`; gives up after 5 seconds, falling back to
    /// whatever the last complete frame holds.
    pub async fn screenshot(&mut self, wait_ms: u32) -> Result<PngImage, String> {
        let quiet = Duration::from_millis(u64::from(wait_ms.max(1)));
        let deadline = Instant::now() + SCREENSHOT_TIMEOUT;
        loop {
            if let Some(image) = capture(&lock(&self.state), Some(quiet))? {
                return Ok(image);
            }
            let reported = lock(&self.state).error.clone();
            if let Some(error) = reported {
                return Err(format!("SPICE {}: {}", self.endpoint, error));
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(SCREENSHOT_POLL).await;
        }

        let relaxed = capture(&lock(&self.state), None)?;
        relaxed.ok_or_else(|| {
            if lock(&self.state).closed {
                format!(
                    "SPICE {}: the session ended before the display channel drew a frame",
                    self.endpoint
                )
            } else {
                format!(
                    "SPICE display channel produced no complete frame within {}s",
                    SCREENSHOT_TIMEOUT.as_secs()
                )
            }
        })
    }

    /// Type text using the US keyboard layout. `interval_ms` is the delay
    /// between keystrokes. Text the US layout cannot produce is rejected.
    pub async fn type_text(&mut self, text: &str, interval_ms: u64) -> Result<(), String> {
        let steps = inputs::type_text_events(text, interval_ms)?;
        self.stream_keys(&steps).await
    }

    /// Press a key combination such as `"ctrl+alt+t"`.
    pub async fn key_press(&mut self, combo: &str) -> Result<(), String> {
        let steps = inputs::key_combo_events(combo)?;
        self.stream_keys(&steps).await
    }

    /// Move the pointer to `(x, y)`: absolute in client mouse mode, relative to
    /// the previous position in server mouse mode.
    pub async fn mouse_move(&mut self, x: u32, y: u32) -> Result<(), String> {
        self.move_pointer(x, y).await
    }

    /// Click a button, optionally moving the pointer first. `double` sends two
    /// clicks close enough together for the guest to see a double click.
    pub async fn mouse_click(
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

        if let Some((x, y)) = position {
            self.move_pointer(x, y).await?;
            tokio::time::sleep(CLICK_GAP).await;
        }

        let clicks = if double { 2 } else { 1 };
        for i in 0..clicks {
            if i > 0 {
                tokio::time::sleep(DOUBLE_CLICK_GAP).await;
            }
            self.press(button.mask()).await?;
            tokio::time::sleep(EVENT_GAP).await;
            self.release(button.mask()).await?;
        }
        Ok(())
    }

    /// Turn the wheel `clicks` times.
    pub async fn mouse_scroll(&mut self, dir: ScrollDir, clicks: u32) -> Result<(), String> {
        for _ in 0..clicks {
            self.press(dir.mask()).await?;
            tokio::time::sleep(EVENT_GAP).await;
            self.release(dir.mask()).await?;
            tokio::time::sleep(EVENT_GAP).await;
        }
        Ok(())
    }

    /// Press the left button at `from`, move to `to` in steps, release.
    pub async fn mouse_drag(&mut self, from: (u32, u32), to: (u32, u32)) -> Result<(), String> {
        let mask = Button::Left.mask();
        self.move_pointer(from.0, from.1).await?;
        tokio::time::sleep(CLICK_GAP).await;
        self.press(mask).await?;
        tokio::time::sleep(EVENT_GAP).await;

        for step in 1..=DRAG_STEPS {
            let x = interpolate(from.0, to.0, step, DRAG_STEPS);
            let y = interpolate(from.1, to.1, step, DRAG_STEPS);
            self.move_pointer(x, y).await?;
            tokio::time::sleep(DRAG_GAP).await;
        }
        self.move_pointer(to.0, to.1).await?;
        tokio::time::sleep(EVENT_GAP).await;

        self.release(mask).await
    }

    /// Send a key sequence, waiting each step's delay first.
    async fn stream_keys(&mut self, steps: &[inputs::KeyStep]) -> Result<(), String> {
        for step in steps {
            if step.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(u64::from(step.delay_ms))).await;
            }
            self.send(step.event.clone()).await?;
        }
        Ok(())
    }

    /// Send one pointer message in the form the negotiated mouse mode expects.
    async fn move_pointer(&mut self, x: u32, y: u32) -> Result<(), String> {
        let event = if self.mouse_mode() == MOUSE_MODE_CLIENT {
            InputEvent::MouseMove { x, y }
        } else {
            let dx = clamp_i32(x as i64 - self.pointer_x as i64);
            let dy = clamp_i32(y as i64 - self.pointer_y as i64);
            InputEvent::MouseMotion { dx, dy }
        };
        self.send(event).await?;
        self.pointer_x = x;
        self.pointer_y = y;
        Ok(())
    }

    /// Press a button mask at the current pointer position.
    async fn press(&mut self, mask: u32) -> Result<(), String> {
        let (x, y) = (self.pointer_x, self.pointer_y);
        self.send(InputEvent::MouseDown { button: mask, x, y })
            .await
    }

    /// Release a button mask at the current pointer position.
    async fn release(&mut self, mask: u32) -> Result<(), String> {
        let (x, y) = (self.pointer_x, self.pointer_y);
        self.send(InputEvent::MouseUp { button: mask, x, y }).await
    }

    /// Queue one input event for the renderer's inputs channel.
    async fn send(&mut self, event: InputEvent) -> Result<(), String> {
        check_inputs_channel(&lock(&self.state), &self.endpoint)?;
        self.input_tx.send(event).await.map_err(|_| {
            format!(
                "SPICE {}: the inputs channel is no longer available",
                self.endpoint
            )
        })
    }

    /// The server's current mouse mode, defaulting to absolute client mode
    /// (the mode the renderer requests) when it is still unknown.
    fn mouse_mode(&self) -> u32 {
        lock(&self.state).mouse_mode.unwrap_or(MOUSE_MODE_CLIENT)
    }
}

impl Drop for SpiceSession {
    fn drop(&mut self) {
        // `run_connection`'s cancel watcher polls this flag and aborts every
        // channel task of this session.
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Refuse input events when the server did not advertise an inputs channel.
/// Sending them would queue into a channel task that does not exist, turning a
/// failed operation into a silent no-op.
fn check_inputs_channel(state: &SessionState, endpoint: &str) -> Result<(), String> {
    match &state.channels {
        Some(channels) if !channels.contains(&ChannelType::Inputs) => Err(format!(
            "SPICE {}: the server offers no inputs channel",
            endpoint
        )),
        _ => Ok(()),
    }
}

/// Encode the primary surface as an 8-bit RGB PNG, if it has been painted and
/// (when `quiet` is set) the display channel has been idle for that long.
fn capture(state: &SessionState, quiet: Option<Duration>) -> Result<Option<PngImage>, String> {
    if state.paints == 0 {
        return Ok(None);
    }
    if let Some(quiet) = quiet {
        if state.last_display_at.elapsed() < quiet {
            return Ok(None);
        }
    }
    match state.mirror.primary_surface() {
        Some(surface) => encode_png(surface).map(Some),
        None => Ok(None),
    }
}

/// Encode a surface's RGBA pixels as an 8-bit RGB PNG (the alpha channel is
/// dropped, as the MCP tools have always returned RGB).
fn encode_png(surface: &DisplaySurface) -> Result<PngImage, String> {
    let (width, height) = surface.size();
    let rgba = surface.pixels();
    let expected = width as usize * height as usize * 4;
    if rgba.len() < expected {
        return Err(format!(
            "SPICE surface {} holds {} bytes, expected {} for {}x{}",
            surface.id,
            rgba.len(),
            expected,
            width,
            height
        ));
    }

    let mut rgb = Vec::with_capacity(expected / 4 * 3);
    for pixel in rgba[..expected].chunks_exact(4) {
        rgb.extend_from_slice(&pixel[..3]);
    }
    let image = ImageBuffer::<Rgb<u8>, Vec<u8>>::from_raw(width, height, rgb)
        .ok_or_else(|| format!("cannot build a {}x{} RGB image", width, height))?;

    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| format!("cannot encode the screenshot as PNG: {}", e))?;
    Ok(PngImage {
        width,
        height,
        png,
    })
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

    fn state_with_surface() -> SessionState {
        let mut state = SessionState::new();
        state.apply(&ChannelEvent::SurfaceCreated {
            display_channel_id: 0,
            surface_id: 0,
            width: 2,
            height: 2,
        });
        state
    }

    fn image_event(left: u32, top: u32, pixels: Vec<u8>) -> ChannelEvent {
        ChannelEvent::ImageReady {
            display_channel_id: 0,
            surface_id: 0,
            left,
            top,
            width: 2,
            height: 2,
            pixels,
            image_id: 0,
            produced_at_secs: 0.0,
        }
    }

    #[test]
    fn session_state_records_paints_and_session_id() {
        let mut state = state_with_surface();
        assert!(state.mirror.primary_surface().is_some());
        assert_eq!(state.paints, 0, "a created surface holds no frame yet");

        let pixels: Vec<u8> = (0..4).flat_map(|_| [7u8, 8, 9, 255]).collect();
        state.apply(&image_event(0, 0, pixels));
        assert_eq!(state.paints, 1);

        state.apply(&ChannelEvent::SessionInitialized(7));
        assert_eq!(state.session_id, Some(7));

        state.apply(&ChannelEvent::MouseMode(1));
        assert_eq!(state.mouse_mode, Some(1));
    }

    #[test]
    fn input_events_need_an_inputs_channel() {
        let mut state = SessionState::new();
        // Nothing advertised yet: events may still be queued.
        assert!(check_inputs_channel(&state, "host:5900").is_ok());

        state.apply(&ChannelEvent::ChannelsAvailable(vec![
            (ChannelType::Main, 0),
            (ChannelType::Display, 0),
        ]));
        let error = check_inputs_channel(&state, "host:5900").expect_err("no inputs channel");
        assert!(error.contains("no inputs channel"), "got: {}", error);

        state.apply(&ChannelEvent::ChannelsAvailable(vec![
            (ChannelType::Main, 0),
            (ChannelType::Display, 0),
            (ChannelType::Inputs, 0),
        ]));
        assert!(check_inputs_channel(&state, "host:5900").is_ok());
    }

    #[test]
    fn only_main_and_display_failures_are_fatal() {
        let mut state = SessionState::new();
        state.apply(&ChannelEvent::Error {
            channel: ChannelType::Inputs,
            message: "inputs hiccup".to_string(),
        });
        assert!(state.error.is_none(), "inputs errors are not fatal");

        state.apply(&ChannelEvent::Disconnected(ChannelType::Display));
        assert!(state
            .error
            .as_deref()
            .expect("display disconnect is fatal")
            .contains("display"));

        // The first error wins, so a later main-channel error cannot hide it.
        state.apply(&ChannelEvent::Error {
            channel: ChannelType::Main,
            message: "main died".to_string(),
        });
        assert!(state.error.as_deref().expect("error").contains("display"));
    }

    #[test]
    fn capture_requires_a_painted_frame_and_a_quiet_display() {
        // Nothing displayed yet.
        let state = SessionState::new();
        assert!(capture(&state, None).expect("no error").is_none());

        // A created surface alone is not a frame: it starts out black.
        let state = state_with_surface();
        assert!(capture(&state, None).expect("no error").is_none());

        let mut state = state_with_surface();
        let pixels: Vec<u8> = (0..4).flat_map(|_| [255u8, 0, 0, 255]).collect();
        state.apply(&image_event(0, 0, pixels));
        // Not quiet yet.
        assert!(capture(&state, Some(Duration::from_secs(30)))
            .expect("no error")
            .is_none());
        // Without a quiet requirement the frame is returned.
        let image = capture(&state, None)
            .expect("no error")
            .expect("painted frame");
        assert_eq!((image.width, image.height), (2, 2));
    }

    #[test]
    fn png_encode_produces_an_rgb_png() {
        let pixels: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 9, 9, 9, 255];
        let mirror = SurfaceMirror::with_test_surface(0, 0, 2, 2, &pixels);
        let surface = mirror.primary_surface().expect("surface");
        let image = encode_png(surface).expect("encode");

        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(&image.png[..8], b"\x89PNG\r\n\x1a\n");

        let decoded = image::load_from_memory(&image.png)
            .expect("decode")
            .to_rgb8();
        assert_eq!(decoded.dimensions(), (2, 2));
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(decoded.get_pixel(1, 0).0, [0, 255, 0]);
        assert_eq!(decoded.get_pixel(0, 1).0, [0, 0, 255]);
        assert_eq!(decoded.get_pixel(1, 1).0, [9, 9, 9]);
    }

    #[test]
    fn button_and_wheel_masks_match_the_protocol_table() {
        // `MouseButton::mask_to_id` maps these masks to the wire ids 1..5.
        assert_eq!(Button::Left.mask(), 1);
        assert_eq!(Button::Middle.mask(), 2);
        assert_eq!(Button::Right.mask(), 4);
        assert_eq!(ScrollDir::Up.mask(), 8);
        assert_eq!(ScrollDir::Down.mask(), 16);
    }

    #[tokio::test]
    async fn unix_endpoints_are_rejected() {
        let result = SpiceSession::connect(
            &SpiceEndpoint::Unix {
                path: "/run/libvirt/qemu/spice.sock".to_string(),
            },
            "",
        )
        .await;
        let error = match result {
            Ok(_) => panic!("unix sockets are unsupported"),
            Err(error) => error,
        };
        assert!(error.contains("unix"), "got: {}", error);
        assert!(error.contains("spice.sock"), "got: {}", error);
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_is_reported() {
        // Port 1 has no listener (and needs root to bind), so the renderer's
        // main-channel connect fails before any session event arrives.
        let result = SpiceSession::connect(
            &SpiceEndpoint::Tcp {
                host: "127.0.0.1".to_string(),
                port: 1,
            },
            "",
        )
        .await;
        let error = match result {
            Ok(_) => panic!("nothing listens on 127.0.0.1:1"),
            Err(error) => error,
        };
        assert!(error.contains("127.0.0.1:1"), "got: {}", error);
    }

    #[test]
    fn interpolation_reaches_both_endpoints() {
        assert_eq!(interpolate(0, 100, 0, 8), 0);
        assert_eq!(interpolate(0, 100, 8, 8), 100);
        assert_eq!(interpolate(100, 0, 4, 8), 50);
        assert_eq!(interpolate(10, 20, 1, 10), 11);
        assert_eq!(interpolate(u32::MAX, 0, 1, 2), 2_147_483_648);
    }

    #[test]
    fn clamp_i32_saturates() {
        assert_eq!(clamp_i32(0), 0);
        assert_eq!(clamp_i32(-5), -5);
        assert_eq!(clamp_i32(i64::MIN), i32::MIN);
        assert_eq!(clamp_i32(i64::MAX), i32::MAX);
    }
}
