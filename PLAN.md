# PLAN: mcp-libvirt — MCP server for libvirt VMs over SPICE

Rust MCP server (stdio) exposing libvirt VM control: list domains, take screenshots,
and send keyboard/mouse input to VMs via the SPICE protocol.

## Environment facts (verified)

- NixOS host. `cargo`/`rustc` 1.97 available. **No libvirt dev headers** (`pkg-config libvirt` fails)
  → we talk to libvirt through the `virsh` CLI, not the `virt` FFI crate.
- `virsh` (12.7.0) works against `qemu:///system`; several VMs exist (all shut off at planning time).
- QEMU with SPICE (`qemu-system-x86_64` present).
- Local libvirt SPICE servers usually have **no password** (empty ticket) and listen on
  `127.0.0.1:590x` (autoport) or a unix socket.

## Architecture

Single binary crate `mcp-libvirt`. The MCP layer is async (`rmcp` + tokio, stdio transport);
the SPICE client is a short-lived synchronous TCP/unix client per operation (wrapped in spawn_blocking).

```
src/
├── main.rs          [A] tokio main: build LibvirtTools, serve over rmcp stdio
├── mcp.rs           [A] rmcp glue: tool router, param structs, CallToolResult helpers
├── libvirt.rs       [A] libvirt bindings via `virt` crate: list domains, states, find SPICE endpoint
├── tools.rs         [A] MCP tool schemas + dispatch: wires libvirt.rs + spice::SpiceSession
└── spice/
    ├── mod.rs       [B] SpiceSession facade + channel lifecycle + ack/ping/common messages
    ├── proto.rs     [B] wire constants, byte reader/writer helpers
    ├── link.rs      [B] link handshake, DER pubkey parse, RSA-OAEP ticket auth, Stream (tcp/unix)
    ├── inputs.rs    [C] keyboard scancodes + key/mouse event senders
    └── display.rs   [C] display channel capture: draw commands → RGB framebuffer → PNG
```

File ownership is disjoint: **A** owns main.rs, mcp.rs, libvirt.rs, tools.rs, Cargo.toml;
**B** owns spice/mod.rs, spice/proto.rs, spice/link.rs;
**C** owns spice/inputs.rs, spice/display.rs.

## Public API contracts (MUST match exactly across workers)

```rust
// libvirt.rs
pub struct DomainInfo { pub name: String, pub id: Option<u32>, pub state: String, pub spice: Option<SpiceEndpoint> }
pub enum SpiceEndpoint { Tcp { host: String, port: u16 }, Unix { path: String } }
pub struct Libvirt { pub uri: String }   // e.g. "qemu:///system"
impl Libvirt {
    pub fn new() -> Self;                       // uri from env LIBVIRT_DEFAULT_URI or "qemu:///system"
    pub fn list_domains(&self) -> Result<Vec<DomainInfo>, String>;
    pub fn domain(&self, name: &str) -> Result<DomainInfo, String>;  // single domain or error
}

// spice/mod.rs
pub struct SpiceSession { /* fields owned by B */ }
impl SpiceSession {
    pub fn connect(ep: &SpiceEndpoint, password: &str) -> Result<SpiceSession, String>;
    pub fn screenshot(&mut self, wait_ms: u32) -> Result<PngImage, String>;   // PngImage { width: u32, height: u32, png: Vec<u8> }
    pub fn type_text(&mut self, text: &str, interval_ms: u64) -> Result<(), String>;
    pub fn key_press(&mut self, combo: &str) -> Result<(), String>;           // "ctrl+alt+t" style; names lowercased
    pub fn mouse_move(&mut self, x: u32, y: u32) -> Result<(), String>;
    pub fn mouse_click(&mut self, button: Button, x: Option<u32>, y: Option<u32>, double: bool) -> Result<(), String>;
    pub fn mouse_scroll(&mut self, dir: ScrollDir, clicks: u32) -> Result<(), String>;
    pub fn mouse_drag(&mut self, from: (u32, u32), to: (u32, u32)) -> Result<(), String>;
}
pub enum Button { Left, Middle, Right }
pub enum ScrollDir { Up, Down }
pub struct PngImage { pub width: u32, pub height: u32, pub png: Vec<u8> }

// spice/proto.rs (B) — consumed by C via these shapes:
pub struct Reader<'a> { /* cursor over &'a [u8] */ }
impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self;
    pub fn u8(&mut self) -> u8; pub fn u16(&mut self) -> u16; pub fn u32(&mut self) -> u32;
    pub fn u64(&mut self) -> u64; pub fn i64(&mut self) -> i64; pub fn i32(&mut self) -> i32;
    pub fn bytes(&mut self, n: usize) -> &'a [u8];
    pub fn remaining(&self) -> usize; pub fn eof(&self) -> bool;
    pub fn pos(&self) -> usize;           // absolute offset within the message buffer
    pub fn seek_abs(&mut self, abs: usize); // for palette offset pointers
    pub fn buf(&self) -> &'a [u8];
}
pub struct Channel { /* one open SPICE channel: stream + ack window state (B-owned) */ }
impl Channel {
    pub fn send_msg(&mut self, msg_type: u16, payload: &[u8]) -> Result<(), String>;  // wraps in mini header, counts ack
    pub fn recv_msg(&mut self, timeout: Duration) -> Result<(u16, Vec<u8>), String>;   // handles SET_ACK/PING internally
}
// channel types:
pub const CHANNEL_MAIN: u8 = 1; pub const CHANNEL_DISPLAY: u8 = 3; pub const CHANNEL_INPUTS: u8 = 2;

// spice/inputs.rs (C) — functions called by B's mod.rs:
pub fn try_type_text_events(text: &str, interval_ms: u64) -> Result<Vec<(u16 /*delay_ms*/, u16 /*msg_type*/, Vec<u8>)>, String>;  // rejects non-ASCII; B calls THIS from SpiceSession::type_text
pub fn type_text_events(text: &str, interval_ms: u64) -> Vec<(u16, u16, Vec<u8>)>;  // infallible variant
pub fn key_combo_events(combo: &str) -> Result<Vec<(u16, u16, Vec<u8>)>, String>;
pub fn mouse_position_payload(x: u32, y: u32, buttons: u16) -> Vec<u8>;
pub fn mouse_press_payload(button: Button, buttons: u16) -> Vec<u8>;
pub fn mouse_release_payload(button: Button, buttons: u16) -> Vec<u8>;
pub fn mouse_motion_payload(dx: i32, dy: i32, buttons: u16) -> Vec<u8>;
// (u16 msg_type values are the SPICE_MSGC_INPUTS_* constants; payload is the exact body bytes)
// C also provides the char/key name → scancode tables + unit tests.

// spice/display.rs (C):
pub struct DisplayCapture { /* framebuffer + image cache (C-owned) */ }
impl DisplayCapture {
    pub fn new() -> Self;
    /// Process one display-channel server message body; returns Ok(()) or Err on fatal.
    pub fn handle_message(&mut self, msg_type: u16, body: &[u8], r: &mut Reader…) -> Result<(), String>;
    pub fn png(&self) -> Option<PngImage>;   // Some once surface 0 has been fully drawn at least once
}
pub fn display_init_payload(cache_id: u8) -> Vec<u8>;      // SPICE_MSGC_DISPLAY_INIT body
pub fn preferred_compression_payload(comp: u8) -> Vec<u8>; // SPICE_MSGC_DISPLAY_PREFERRED_COMPRESSION body
```

Error handling: plain `Result<_, String>` with human-readable messages (this is an MCP tool
server; errors go back to the LLM as text). No `thiserror`/`anyhow` needed.

## SPICE protocol — verified facts (do NOT guess; reference files under `docs/`)

Authoritative references (fetched into this repo):
- `docs/spice-protocol/protocol.h`, `docs/spice-protocol/enums.h` — wire structs, constants, message IDs
- `docs/spice.proto` — message body layouts (spice-common definition file)
- `docs/spice-html5/*.js` — complete working minimal client (link/auth in `spiceconn.js`+`ticket.js`,
  wire structs in `spicemsg.js`/`spicetype.js`, keyboard in `inputs.js`+`code_to_scancode.js`+`utils.js`,
  display in `display.js`+`bitmap.js`) — **port logic from here**
- `docs/spice-server/dcc.cpp` — server behavior (preferred compression handling)

All integers are **little-endian** on the wire.

### Link handshake (each channel = one TCP/unix connection)

1. Client → server `SpiceLinkHeader` (16B): magic u32 `0x51444552` ("REDQ"), major u32 = 2,
   minor u32 = 2, size u32 = sizeof(LinkMess incl. caps).
2. Then `SpiceLinkMess` (18B + caps): connection_id u32 (0 for MAIN, session id for children),
   channel_type u8, channel_id u8 = 0, num_common_caps u32, num_channel_caps u32,
   caps_offset u32 (= 18, offset from LinkMess start), then cap words.
   Common caps: `(1<<SPICE_COMMON_CAP_PROTOCOL_AUTH_SELECTION) | (1<<SPICE_COMMON_CAP_MINI_HEADER)`.
   Channel caps: MAIN → none required; DISPLAY → none required (we want raw/lossless); INPUTS → none.
3. Server → client `SpiceLinkHeader` + `SpiceLinkReply`: error u32, pub_key[162], num_common_caps u32,
   num_channel_caps u32, caps_offset u32 (offset from LinkReply start), then cap words.
   `pub_key` is a DER SubjectPublicKeyInfo: sequence(sequence(alg)) bitstring(sequence(INTEGER n, INTEGER e)).
   See `docs/spice-html5/ticket.js::create_rsa_from_mb` for a working parse. RSA is 1024-bit (n = 128 bytes).
4. Client → server: `SpiceLinkAuthTicket`: auth_mechanism u32 = 1 (`SPICE_COMMON_CAP_AUTH_SPICE`),
   then encrypted_data[128]. Ticket plaintext = password bytes + one trailing NUL (≤ 60 bytes; empty
   password → single NUL byte), RSA-OAEP(SHA-1) encrypted → exactly 128 bytes.
   Use the `rsa` crate: `RsaPublicKey::new(BigUint, BigUint)` + `rsa::oaep::Oaep::new::<Sha1>()`.
   (spice-html5 `ticket.js` confirms OAEP + password+"\0" plaintext; this is NOT PKCS1 v1.5.)
5. Server → client: u32 auth_code. 0 = OK (`SPICE_LINK_ERR_OK`); 7 = PERMISSION_DENIED (bad password).
6. On success the channel is "ready": all further messages use `SpiceMiniDataHeader`
   (type u16, size u32 = 6 bytes) + body. (We advertised MINI_HEADER.)

### Channel lifecycle / common messages (apply to every channel)

- Server `SPICE_MSG_SET_ACK` (3): {generation u32, window u32} → immediately reply
  `SPICE_MSGC_ACK_SYNC` (1): {generation u32}; then after every `window` messages received send
  `SPICE_MSGC_ACK` (2, empty body).
- Server `SPICE_MSG_PING` (4): {id u32, timestamp u64, data…} → reply `SPICE_MSGC_PONG` (3) with
  {id, timestamp} copied.
- `SPICE_MSG_DISCONNECTING` (6): {time_stamp u64, reason u32} → connection over, report error.
- `SPICE_MSG_NOTIFY` (7): {severity, visibility, what, message_len u32, message bytes} → log, ignore.

### Main channel (SPICE_CHANNEL_MAIN = 1)

After auth OK, server sends `SPICE_MSG_MAIN_INIT` (103):
{session_id u32, display_channels_hint u32, supported_mouse_modes u16, current_mouse_mode u16,
agent_connected u32, agent_tokens u32, multi_media_time u32, ram_hint u32}.
**session_id is the connection_id** child channels must pass in their LinkMess.
If `supported_mouse_modes & SPICE_MOUSE_MODE_CLIENT (0x2)` and current mode ≠ CLIENT, send
`SPICE_MSGC_MAIN_MOUSE_MODE_REQUEST` (105): {mode u32 = 2}. Keep main channel open while
child channels run (respond to ping/ack).

### Inputs channel (SPICE_CHANNEL_INPUTS = 2)

Client messages (mini header + body):
- `SPICE_MSGC_INPUTS_KEY_DOWN` (101): {code u32}
- `SPICE_MSGC_INPUTS_KEY_UP` (102): {code u32}
- `SPICE_MSGC_INPUTS_MOUSE_MOTION` (111): {dx i32, dy i32, buttons_state u16}
- `SPICE_MSGC_INPUTS_MOUSE_POSITION` (112): {x u32, y u32, buttons_state u16, display_id u8 = 0}
- `SPICE_MSGC_INPUTS_MOUSE_PRESS` (113): {button u8, buttons_state u16}
- `SPICE_MSGC_INPUTS_MOUSE_RELEASE` (114): {button u8, buttons_state u16}

**Scancode encoding** (port `code_to_scancode.js` + `utils.js`):
- Normal key: code = AT set-1 make code (e.g. "a" = 0x1E). Key up = code | 0x80.
- Extended (E0-prefixed) key: code = 0xE0 | (make << 8) (e.g. ArrowUp = 0x48E0). Key up = code | 0x8000.
- Buttons: LEFT=1, MIDDLE=2, RIGHT=3, UP(wheel)=4, DOWN(wheel)=5; masks 1<<0..1<<4.
- Click = position(112) with buttons_state=0 → press(113) → release(114) (each with buttons_state
  updated); wheel = press+release of button 4/5; drag = position → press → position(s, interpolated)
  → release. Small delays (10–20 ms) between events.
- Typing text: for each char, produce [down, up] events with 10–50 ms between; US layout mapping
  char → scancode + optional shift (e.g. 'A' → shift + 0x1E, '!' → shift + 0x02). Reject non-ASCII
  with a clear error.
- Key combos "ctrl+alt+t": press all downs (left→right), then releases in reverse; aliases:
  ctrl, alt, shift, meta/super/win, esc, enter/return, tab, space, backspace, delete, insert,
  home, end, pgup, pageup, pgdn, pagedown, up/down/left/right, f1..f12, capslock, numlock,
  printscreen, pause, a–z, 0–9, and symbols via US-layout mapping.

### Display channel (SPICE_CHANNEL_DISPLAY = 3)

Immediately after auth OK:
1. Send `SPICE_MSGC_DISPLAY_INIT` (101): {pixmap_cache_id u8 = 1, pixmap_cache_size i64 = 10*1024*1024,
   glz_dictionary_id u8 = 0, glz_dictionary_window_size i32 = 0}.
2. Send `SPICE_MSGC_DISPLAY_PREFERRED_COMPRESSION` (103): {image_compression u8 = 1 (OFF)}.
   **Verified** in `docs/spice-server/dcc.cpp::dcc_handle_preferred_compression` that the server
   accepts OFF and then sends raw bitmaps (`get_compression_for_bitmap` returns OFF), so we never
   need QUIC/GLZ/LZ decoders.

Server messages to handle (see `docs/spice-html5/display.js` for working logic):
- `SPICE_MSG_SURFACE_CREATE` (318): {surface_id u32, width u32, height u32, format u32, flags u32}
  → allocate RGB framebuffer (surface 0 = primary).
- `SPICE_MSG_SURFACE_DESTROY` (319) → drop surface.
- `SPICE_MSG_DISPLAY_MARK` (102) → mark "need full redraw"; server follows with full-screen draw commands.
- `SPICE_MSG_DISPLAY_MODE` (101), `SPICE_MSG_DISPLAY_RESET` (103) → reset state.
- `SPICE_MSG_DISPLAY_COPY_BITS` (104): {DisplayBase, src_pos Point} — move pixels within framebuffer.
- `SPICE_MSG_DISPLAY_DRAW_FILL` (302): {DisplayBase, brush(type u8, color u32 if SOLID), rop_descriptor u16, mask}
  → fill rect with color. DisplayBase = {surface_id u32, box Rect(top,left,bottom,right), clip type u8 + rects}.
  Mask on the wire: {flags u8, pos Point(x,y), then an Image descriptor (id u64, type u8, flags u8,
  width u32, height u32) — a null mask is a descriptor with id 0/type 0; then no image body}.
- `SPICE_MSG_DISPLAY_DRAW_COPY` (304) / `SPICE_MSG_DISPLAY_DRAW_BLEND` (306) /
  `DRAW_TRANSPARENT` (313) / `DRAW_ALPHA_BLEND` (314): {DisplayBase, src Image, src_area Rect,
  [extras], mask} → blit image region into framebuffer box.
- `SPICE_MSG_DISPLAY_DRAW_OPAQUE` (303): {DisplayBase, src Image, src_area Rect, brush, rop_descriptor u16,
  scale_mode u8, mask} → treat like copy using src image.
- Image (wire): descriptor {id u64, type u8, flags u8, width u32, height u32}. For
  `SPICE_IMAGE_TYPE_BITMAP` (0): bitmap = {format u8, flags u8, x u32, y u32, stride u32,
  palette (u32 offset: 0 = none, else absolute offset in message → {unique u64, num_ents u16, ents u32[]}),
  then raw pixel bytes, length = stride × y}. **No chunk header precedes the pixel data**
  (the `@chunk` annotation is virtual; verified in spice-common codegen: the demarshaller wraps the
  remaining bytes as a single synthetic chunk).
- Bitmap formats to support (convert to 24-bit RGB framebuffer): 32BIT (xrgb LE), RGBA (argb LE),
  24BIT (bgr 3 bytes), 16BIT (555 LE), 8BIT (palette indexed), 1BIT_LE/1BIT_BE (bit per pixel,
  default palette black/white when absent). Respect `SPICE_BITMAP_FLAGS_TOP_DOWN` (0x4):
  absent → rows are bottom-up.
- Image cache: if descriptor flags & CACHE_ME (0x1), store decoded image by descriptor id;
  type `FROM_CACHE` (4) / `FROM_CACHE_LOSSLESS` (9) → reuse cached; `SPICE_MSG_DISPLAY_INVAL_ALL_PIXMAPS`
  (106) → clear cache; `SPICE_MSG_DISPLAY_INVAL_LIST` (105) → drop listed ids.
- Ignore (log + continue): STREAM_* (122–125), MONITORS_CONFIG (320), DRAW_COMPOSITE (321),
  DRAW_STROKE/DRAW_TEXT/DRAW_ROP3/DRAW_BLACKNESS/WHITENESS/INVERS (log warn; if a full-frame screenshot
  turns out incomplete that's acceptable — report `png()` when surface 0 has data).
- Unsupported image types (QUIC etc.) → error string "unsupported image type N" (should not happen
  with preferred compression OFF).

Screenshot flow in `SpiceSession::screenshot`: open display channel, send init + preferred
compression OFF, then pump messages until either (a) quiet period of `wait_ms` (default 500 ms
after MARK + at least one full-frame pass), then `DisplayCapture::png()` → `PngImage`.
Cap total wait at ~5 s. Encode with the `png` crate (RGB 8-bit).

## MCP layer (official `rmcp` crate, stdio transport)

We reuse the official Rust MCP SDK (`rmcp` 3.x, features `transport-io` + `schemars`) instead of
hand-rolling JSON-RPC. Worker A rewrites src/mcp.rs + src/main.rs + src/tools.rs to this model
(API verified against the rmcp README):

```rust
use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock},
           schemars, tool, tool_router, tool_handler, ServerHandler, ServiceExt, transport::stdio};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ScreenshotParams { domain: String, wait_ms: Option<u32> }

#[derive(Clone)]
struct LibvirtTools;   // holds the crate::libvirt::Libvirt handle

#[tool_router]
impl LibvirtTools {
    #[tool(description = "Take a screenshot of a VM's SPICE display")]
    async fn screenshot(&self, Parameters(ScreenshotParams { domain, wait_ms }): Parameters<ScreenshotParams>)
        -> Result<CallToolResult, rmcp::Error> { ... }
}
#[tool_handler(name = "mcp-libvirt", version = "0.1.0")]
impl ServerHandler for LibvirtTools {}

#[tokio::main]
async fn main() {
    let service = LibvirtTools.serve(stdio()).await?;   // then service.waiting().await
}
```

- Tool results: `Ok(CallToolResult::success(vec![ContentBlock::text(...), ContentBlock::image(base64_png, "image/png")]))`;
  operational errors -> `Ok(CallToolResult::error(vec![ContentBlock::text("<error>")]))` (isError content),
  protocol errors -> Err(rmcp::Error). Param structs derive `serde::Deserialize` + `schemars::JsonSchema`
  (field docs = tool descriptions). Screenshot returns one image block + a text block with width/height.
- Long SPICE work is synchronous: run it via `tokio::task::spawn_blocking` inside each handler.
- src/mcp.rs may shrink to glue/re-exports or be deleted - main.rs owns the tokio main + serve call.
  Check the vendored rmcp 3.x source in ~/.cargo/registry for exact signatures when unsure
  (`cargo fetch` first); the macro API above is from the current SDK README.

### Tools

1. `list_domains` () -> text table: name, id, state, spice endpoint.
2. `screenshot` ({domain: string, wait_ms?: number}) -> image content (+ text with width/height).
3. `type_text` ({domain, text: string, interval_ms?: number}) -> text ok.
4. `key_press` ({domain, keys: string /* "ctrl+alt+t" */}) -> text ok.
5. `mouse_move` ({domain, x: int, y: int}) -> text ok.
6. `mouse_click` ({domain, button: "left"|"middle"|"right", x?: int, y?: int, double_click?: bool}) -> text ok.
7. `mouse_scroll` ({domain, direction: "up"|"down", clicks?: int}) -> text ok.
8. `mouse_drag` ({domain, from_x, from_y, to_x, to_y}) -> text ok.

Every input/screenshot call: resolve domain -> SPICE endpoint (fresh connection per call),
connect main channel (+ mouse mode request), open the needed child channel, perform operation,
drop connection. rmcp derives the inputSchema JSON from the param structs.

## libvirt layer (official `virt` crate - NO virsh subprocess)

The public Rust contract (Libvirt/DomainInfo/SpiceEndpoint) stays EXACTLY as in 'Public API
contracts'; only the implementation changes: use the `virt` crate (0.4, FFI to libvirt, linked
via the Nix devShell - see 'Build environment'). API verified against virt 0.4.3 source:

```rust
use virt::connect::Connect;
let conn = Connect::open(Some(uri))?;                 // "qemu:///system"
let domains = conn.list_all_domains(0)?;              // Vec<virt::domain::Domain>
for d in domains {
    let name = d.get_name()?;
    let id = d.get_id();                              // Option<u32>: None when inactive
    let (state, _reason) = d.get_state()?;            // numeric virDomainState
    let xml = d.get_xml_desc(0)?;                     // LIVE xml: port populated when running
}
```

- State mapping to human strings: 0 no state, 1 running, 2 blocked, 3 paused, 4 shutdown,
  5 shut off, 6 crashed, 7 pmsuspended.
- SPICE endpoint: parse the LIVE XML from `get_xml_desc(0)`: `<graphics type="spice" port="5900"
listen="...">` (port present only while the VM runs; autoport VMs get their assigned port there)
and `<listen type="socket" path="..."/>` -> `SpiceEndpoint::Unix`. If graphics type is not
spice -> `spice: None`; tools report a clear error. Keep Worker A's existing XML-parsing helpers
and unit tests - only swap the transport (virsh CLI -> virt crate).
- Password: env `MCP_LIBVIRT_SPICE_PASSWORD` (optional, default empty).
- Errors: `virt::error::Error` -> human string (include .to_string()).

## Build environment (NixOS devShell - parent-managed)

The `virt` crate needs libvirt C headers/libs at build time -> everything builds inside a
flake-parts devShell: `nix develop -c cargo build`, `nix develop -c cargo test`,
`nix develop -c cargo run`. flake.nix at the repo root provides rustc, cargo, pkg-config, libvirt.
If a bare `cargo build` fails with header/linker errors, that is expected - use the devShell.
(Host note: the rustup ld shim may need `-C link-arg=-fuse-ld=bfd`; nixpkgs rustc inside the
devShell should not.)

## Cargo.toml

```toml
[package]
name = "mcp-libvirt"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
base64 = "0.22"
png = "0.17"
rsa = { version = "0.9", features = ["sha1"] }
sha1 = "0.10"
rand = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "io-std"] }
rmcp = { version = "3", features = ["transport-io", "schemars"] }
virt = "0.4"
```

This Cargo.toml is **created and managed by the parent** (it already exists in the repo root).
Workers A/B/C must NOT modify it. If you believe a dependency or feature is missing,
say so in your report instead of editing it.


## Task decomposition (delegated implementation)

- **Worker A — scaffold + MCP (rmcp) + libvirt (virt crate) + tools** (files: src/main.rs,
  src/mcp.rs, src/libvirt.rs, src/tools.rs). Follows contracts above; calls `crate::spice::SpiceSession`.
- **Worker B — SPICE core** (files: src/spice/mod.rs, src/spice/proto.rs, src/spice/link.rs).
  Link/auth/ack/ping/session/channel plumbing; implements `SpiceSession` methods by calling
  into `crate::spice::inputs` / `crate::spice::display` functions (contract above).
- **Worker C — inputs + display capture** (files: src/spice/inputs.rs, src/spice/display.rs).
  Port scancode tables + display composition from docs/spice-html5.
- **Integration worker** — after A+B+C: `nix develop -c cargo build` + `nix develop -c cargo test`
  green, fix all mismatches against this PLAN, no redesign.
  against this PLAN, no redesign.

Validation per contract: pure-logic unit tests (link message bytes, scancode table, text typing
events, bitmap→RGB conversion, PNG roundtrip, JSON-RPC dispatch) + final smoke test against a
real VM (parent-run).