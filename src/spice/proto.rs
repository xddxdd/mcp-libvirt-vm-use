//! SPICE wire protocol constants and byte cursor helpers.
//!
//! All integers on the SPICE wire are little-endian. Constant values come from
//! `docs/enums.h` (the generated `spice/enums.h` from spice-protocol) and are
//! cross-checked against the working minimal client in `docs/spice-html5/enums.js`.
//!
//! The tables below are deliberately complete, as PLAN.md requires: a value that
//! only a sibling module or the unit tests read is not dead code, so the
//! `dead_code` lint is off for this protocol reference table.
#![allow(dead_code)]

// ---------------------------------------------------------------------------
// Link handshake
// ---------------------------------------------------------------------------

/// `SPICE_MAGIC_CONST("REDQ")`: the 4 ASCII bytes 'R','E','D','Q' packed
/// little-endian (see `SPICE_MAGIC_CONST` in `docs/macros.h`).
pub const SPICE_MAGIC: u32 = 0x5144_4552;
pub const SPICE_VERSION_MAJOR: u32 = 2;
pub const SPICE_VERSION_MINOR: u32 = 2;

/// `SPICE_TICKET_KEY_PAIR_LENGTH / 8`: RSA modulus size of the server ticket key.
pub const SPICE_TICKET_KEY_BYTES: usize = 128;
/// `SPICE_TICKET_PUBKEY_BYTES` = 1024/8 + 34: fixed size of the `pub_key` field.
pub const SPICE_TICKET_PUBKEY_BYTES: usize = 162;
/// `SPICE_MAX_PASSWORD_LENGTH`: plaintext ticket is the password plus one NUL.
pub const SPICE_MAX_PASSWORD_LENGTH: usize = 60;
/// Offset of the capability words inside `SpiceLinkMess` / `SpiceLinkReply`
/// (i.e. `sizeof` of the fixed part of those structs).
pub const LINK_CAPS_OFFSET: u32 = 18;

/// `SPICE_COMMON_CAP_AUTH_SPICE`: the `auth_mechanism` value we select.
pub const AUTH_MECHANISM_SPICE: u32 = 1;

pub const LINK_ERR_OK: u32 = 0;
pub const LINK_ERR_ERROR: u32 = 1;
pub const LINK_ERR_INVALID_MAGIC: u32 = 2;
pub const LINK_ERR_INVALID_DATA: u32 = 3;
pub const LINK_ERR_VERSION_MISMATCH: u32 = 4;
pub const LINK_ERR_NEED_SECURED: u32 = 5;
pub const LINK_ERR_NEED_UNSECURED: u32 = 6;
pub const LINK_ERR_PERMISSION_DENIED: u32 = 7;
pub const LINK_ERR_BAD_CONNECTION_ID: u32 = 8;
pub const LINK_ERR_CHANNEL_NOT_AVAILABLE: u32 = 9;

// ---------------------------------------------------------------------------
// Capability bit indices
// ---------------------------------------------------------------------------

pub const COMMON_CAP_PROTOCOL_AUTH_SELECTION: u32 = 0;
pub const COMMON_CAP_AUTH_SPICE: u32 = 1;
pub const COMMON_CAP_AUTH_SASL: u32 = 2;
pub const COMMON_CAP_MINI_HEADER: u32 = 3;

pub const MAIN_CAP_SEMI_SEAMLESS_MIGRATE: u32 = 0;
pub const MAIN_CAP_NAME_AND_UUID: u32 = 1;
pub const MAIN_CAP_AGENT_CONNECTED_TOKENS: u32 = 2;
pub const MAIN_CAP_SEAMLESS_MIGRATE: u32 = 3;

pub const DISPLAY_CAP_SIZED_STREAM: u32 = 0;
pub const DISPLAY_CAP_MONITORS_CONFIG: u32 = 1;
pub const DISPLAY_CAP_COMPOSITE: u32 = 2;
pub const DISPLAY_CAP_A8_SURFACE: u32 = 3;
pub const DISPLAY_CAP_STREAM_REPORT: u32 = 4;
pub const DISPLAY_CAP_LZ4_COMPRESSION: u32 = 5;
pub const DISPLAY_CAP_PREF_COMPRESSION: u32 = 6;
pub const DISPLAY_CAP_GL_SCANOUT: u32 = 7;
pub const DISPLAY_CAP_MULTI_CODEC: u32 = 8;
pub const DISPLAY_CAP_CODEC_MJPEG: u32 = 9;
pub const DISPLAY_CAP_CODEC_VP8: u32 = 10;
pub const DISPLAY_CAP_CODEC_H264: u32 = 11;
pub const DISPLAY_CAP_PREF_VIDEO_CODEC_TYPE: u32 = 12;

pub const INPUTS_CAP_KEY_SCANCODE: u32 = 0;

// ---------------------------------------------------------------------------
// Channel types (`SpiceChannel`)
// ---------------------------------------------------------------------------

pub const CHANNEL_MAIN: u8 = 1;
pub const CHANNEL_DISPLAY: u8 = 2;
pub const CHANNEL_INPUTS: u8 = 3;
pub const CHANNEL_CURSOR: u8 = 4;
pub const CHANNEL_PLAYBACK: u8 = 5;
pub const CHANNEL_RECORD: u8 = 6;
pub const CHANNEL_TUNNEL: u8 = 7;
pub const CHANNEL_SMARTCARD: u8 = 8;
pub const CHANNEL_USBREDIR: u8 = 9;
pub const CHANNEL_PORT: u8 = 10;
pub const CHANNEL_WEBDAV: u8 = 11;

// ---------------------------------------------------------------------------
// Common messages (every channel)
// ---------------------------------------------------------------------------

pub const MSG_MIGRATE: u16 = 1;
pub const MSG_MIGRATE_DATA: u16 = 2;
pub const MSG_SET_ACK: u16 = 3;
pub const MSG_PING: u16 = 4;
pub const MSG_WAIT_FOR_CHANNELS: u16 = 5;
pub const MSG_DISCONNECTING: u16 = 6;
pub const MSG_NOTIFY: u16 = 7;
pub const MSG_LIST: u16 = 8;

pub const MSGC_ACK_SYNC: u16 = 1;
pub const MSGC_ACK: u16 = 2;
pub const MSGC_PONG: u16 = 3;

// ---------------------------------------------------------------------------
// Main channel
// ---------------------------------------------------------------------------

pub const MSG_MAIN_MIGRATE_BEGIN: u16 = 101;
pub const MSG_MAIN_MIGRATE_CANCEL: u16 = 102;
pub const MSG_MAIN_INIT: u16 = 103;
pub const MSG_MAIN_CHANNELS_LIST: u16 = 104;
pub const MSG_MAIN_MOUSE_MODE: u16 = 105;
pub const MSG_MAIN_MULTI_MEDIA_TIME: u16 = 106;
pub const MSG_MAIN_AGENT_CONNECTED: u16 = 107;
pub const MSG_MAIN_AGENT_DISCONNECTED: u16 = 108;
pub const MSG_MAIN_AGENT_DATA: u16 = 109;
pub const MSG_MAIN_AGENT_TOKEN: u16 = 110;
pub const MSG_MAIN_MIGRATE_SWITCH_HOST: u16 = 111;
pub const MSG_MAIN_MIGRATE_END: u16 = 112;
pub const MSG_MAIN_NAME: u16 = 113;
pub const MSG_MAIN_UUID: u16 = 114;
pub const MSG_MAIN_AGENT_CONNECTED_TOKENS: u16 = 115;

pub const MSGC_MAIN_CLIENT_INFO: u16 = 101;
pub const MSGC_MAIN_MIGRATE_CONNECTED: u16 = 102;
pub const MSGC_MAIN_MIGRATE_CONNECT_ERROR: u16 = 103;
pub const MSGC_MAIN_ATTACH_CHANNELS: u16 = 104;
pub const MSGC_MAIN_MOUSE_MODE_REQUEST: u16 = 105;
pub const MSGC_MAIN_AGENT_START: u16 = 106;
pub const MSGC_MAIN_AGENT_DATA: u16 = 107;
pub const MSGC_MAIN_AGENT_TOKEN: u16 = 108;
pub const MSGC_MAIN_MIGRATE_END: u16 = 109;

/// Body size of `SpiceMsgcMainMouseModeRequest`: a `flags16 mouse_mode`.
pub const MAIN_MOUSE_MODE_REQUEST_LEN: usize = 2;
/// Body size of `SpiceMsgMainInit`: eight u32 fields.
pub const MAIN_INIT_LEN: usize = 32;

// ---------------------------------------------------------------------------
// Display channel
// ---------------------------------------------------------------------------

pub const MSG_DISPLAY_MODE: u16 = 101;
pub const MSG_DISPLAY_MARK: u16 = 102;
pub const MSG_DISPLAY_RESET: u16 = 103;
pub const MSG_DISPLAY_COPY_BITS: u16 = 104;
pub const MSG_DISPLAY_INVAL_LIST: u16 = 105;
pub const MSG_DISPLAY_INVAL_ALL_PIXMAPS: u16 = 106;
pub const MSG_DISPLAY_INVAL_PALETTE: u16 = 107;
pub const MSG_DISPLAY_INVAL_ALL_PALETTES: u16 = 108;

pub const MSG_DISPLAY_STREAM_CREATE: u16 = 122;
pub const MSG_DISPLAY_STREAM_DATA: u16 = 123;
pub const MSG_DISPLAY_STREAM_CLIP: u16 = 124;
pub const MSG_DISPLAY_STREAM_DESTROY: u16 = 125;
pub const MSG_DISPLAY_STREAM_DESTROY_ALL: u16 = 126;

pub const MSG_DISPLAY_DRAW_FILL: u16 = 302;
pub const MSG_DISPLAY_DRAW_OPAQUE: u16 = 303;
pub const MSG_DISPLAY_DRAW_COPY: u16 = 304;
pub const MSG_DISPLAY_DRAW_BLEND: u16 = 305;
pub const MSG_DISPLAY_DRAW_BLACKNESS: u16 = 306;
pub const MSG_DISPLAY_DRAW_WHITENESS: u16 = 307;
pub const MSG_DISPLAY_DRAW_INVERS: u16 = 308;
pub const MSG_DISPLAY_DRAW_ROP3: u16 = 309;
pub const MSG_DISPLAY_DRAW_STROKE: u16 = 310;
pub const MSG_DISPLAY_DRAW_TEXT: u16 = 311;
pub const MSG_DISPLAY_DRAW_TRANSPARENT: u16 = 312;
pub const MSG_DISPLAY_DRAW_ALPHA_BLEND: u16 = 313;
pub const MSG_DISPLAY_SURFACE_CREATE: u16 = 314;
pub const MSG_DISPLAY_SURFACE_DESTROY: u16 = 315;
pub const MSG_DISPLAY_STREAM_DATA_SIZED: u16 = 316;
pub const MSG_DISPLAY_MONITORS_CONFIG: u16 = 317;
pub const MSG_DISPLAY_DRAW_COMPOSITE: u16 = 318;
pub const MSG_DISPLAY_STREAM_ACTIVATE_REPORT: u16 = 319;
pub const MSG_DISPLAY_GL_SCANOUT_UNIX: u16 = 320;
pub const MSG_DISPLAY_GL_DRAW: u16 = 321;
pub const MSG_DISPLAY_QUALITY_INDICATOR: u16 = 322;
pub const MSG_DISPLAY_GL_SCANOUT2_UNIX: u16 = 323;

pub const MSGC_DISPLAY_INIT: u16 = 101;
pub const MSGC_DISPLAY_STREAM_REPORT: u16 = 102;
pub const MSGC_DISPLAY_PREFERRED_COMPRESSION: u16 = 103;
pub const MSGC_DISPLAY_GL_DRAW_DONE: u16 = 104;
pub const MSGC_DISPLAY_PREFERRED_VIDEO_CODEC_TYPE: u16 = 105;

/// Body size of `SpiceMsgcDisplayInit`: u8, i64, u8, i32.
pub const DISPLAY_INIT_LEN: usize = 14;

// ---------------------------------------------------------------------------
// Inputs channel
// ---------------------------------------------------------------------------

pub const MSG_INPUTS_INIT: u16 = 101;
pub const MSG_INPUTS_KEY_MODIFIERS: u16 = 102;
pub const MSG_INPUTS_MOUSE_MOTION_ACK: u16 = 111;

pub const MSGC_INPUTS_KEY_DOWN: u16 = 101;
pub const MSGC_INPUTS_KEY_UP: u16 = 102;
pub const MSGC_INPUTS_KEY_MODIFIERS: u16 = 103;
pub const MSGC_INPUTS_KEY_SCANCODE: u16 = 104;
pub const MSGC_INPUTS_MOUSE_MOTION: u16 = 111;
pub const MSGC_INPUTS_MOUSE_POSITION: u16 = 112;
pub const MSGC_INPUTS_MOUSE_PRESS: u16 = 113;
pub const MSGC_INPUTS_MOUSE_RELEASE: u16 = 114;

pub const INPUT_MOTION_ACK_BUNCH: u32 = 4;

// ---------------------------------------------------------------------------
// Mouse
// ---------------------------------------------------------------------------

pub const MOUSE_MODE_SERVER: u32 = 1 << 0;
pub const MOUSE_MODE_CLIENT: u32 = 1 << 1;
pub const MOUSE_MODE_MASK: u32 = 0x3;

pub const MOUSE_BUTTON_INVALID: u8 = 0;
pub const MOUSE_BUTTON_LEFT: u8 = 1;
pub const MOUSE_BUTTON_MIDDLE: u8 = 2;
pub const MOUSE_BUTTON_RIGHT: u8 = 3;
pub const MOUSE_BUTTON_UP: u8 = 4;
pub const MOUSE_BUTTON_DOWN: u8 = 5;

pub const MOUSE_BUTTON_MASK_LEFT: u16 = 1 << 0;
pub const MOUSE_BUTTON_MASK_MIDDLE: u16 = 1 << 1;
pub const MOUSE_BUTTON_MASK_RIGHT: u16 = 1 << 2;
pub const MOUSE_BUTTON_MASK_UP: u16 = 1 << 3;
pub const MOUSE_BUTTON_MASK_DOWN: u16 = 1 << 4;

// ---------------------------------------------------------------------------
// Images / bitmaps
// ---------------------------------------------------------------------------

pub const IMAGE_TYPE_BITMAP: u8 = 0;
pub const IMAGE_TYPE_QUIC: u8 = 1;
pub const IMAGE_TYPE_RESERVED: u8 = 2;
pub const IMAGE_TYPE_LZ_PLT: u8 = 100;
pub const IMAGE_TYPE_LZ_RGB: u8 = 101;
pub const IMAGE_TYPE_GLZ_RGB: u8 = 102;
pub const IMAGE_TYPE_FROM_CACHE: u8 = 103;
pub const IMAGE_TYPE_SURFACE: u8 = 104;
pub const IMAGE_TYPE_JPEG: u8 = 105;
pub const IMAGE_TYPE_FROM_CACHE_LOSSLESS: u8 = 106;
pub const IMAGE_TYPE_ZLIB_GLZ_RGB: u8 = 107;
pub const IMAGE_TYPE_JPEG_ALPHA: u8 = 108;
pub const IMAGE_TYPE_LZ4: u8 = 109;

pub const IMAGE_FLAGS_CACHE_ME: u8 = 1 << 0;
pub const IMAGE_FLAGS_HIGH_BITS_SET: u8 = 1 << 1;
pub const IMAGE_FLAGS_CACHE_REPLACE_ME: u8 = 1 << 2;

pub const IMAGE_COMPRESSION_INVALID: u8 = 0;
pub const IMAGE_COMPRESSION_OFF: u8 = 1;
pub const IMAGE_COMPRESSION_AUTO_GLZ: u8 = 2;
pub const IMAGE_COMPRESSION_AUTO_LZ: u8 = 3;
pub const IMAGE_COMPRESSION_QUIC: u8 = 4;
pub const IMAGE_COMPRESSION_GLZ: u8 = 5;
pub const IMAGE_COMPRESSION_LZ: u8 = 6;
pub const IMAGE_COMPRESSION_LZ4: u8 = 7;

pub const BITMAP_FMT_INVALID: u8 = 0;
pub const BITMAP_FMT_1BIT_LE: u8 = 1;
pub const BITMAP_FMT_1BIT_BE: u8 = 2;
pub const BITMAP_FMT_4BIT_LE: u8 = 3;
pub const BITMAP_FMT_4BIT_BE: u8 = 4;
pub const BITMAP_FMT_8BIT: u8 = 5;
pub const BITMAP_FMT_16BIT: u8 = 6;
pub const BITMAP_FMT_24BIT: u8 = 7;
pub const BITMAP_FMT_32BIT: u8 = 8;
pub const BITMAP_FMT_RGBA: u8 = 9;
pub const BITMAP_FMT_8BIT_A: u8 = 10;

pub const BITMAP_FLAGS_PAL_CACHE_ME: u8 = 1 << 0;
pub const BITMAP_FLAGS_PAL_FROM_CACHE: u8 = 1 << 1;
pub const BITMAP_FLAGS_TOP_DOWN: u8 = 1 << 2;

pub const SURFACE_FLAGS_PRIMARY: u32 = 1 << 0;
pub const SURFACE_FLAGS_STREAMING_MODE: u32 = 1 << 1;

pub const SURFACE_FMT_INVALID: u32 = 0;
pub const SURFACE_FMT_1_A: u32 = 1;
pub const SURFACE_FMT_8_A: u32 = 8;
pub const SURFACE_FMT_16_555: u32 = 16;
pub const SURFACE_FMT_32_XRGB: u32 = 32;
pub const SURFACE_FMT_16_565: u32 = 80;
pub const SURFACE_FMT_32_ARGB: u32 = 96;

pub const CLIP_TYPE_NONE: u8 = 0;
pub const CLIP_TYPE_RECTS: u8 = 1;

pub const BRUSH_TYPE_NONE: u8 = 0;
pub const BRUSH_TYPE_SOLID: u8 = 1;
pub const BRUSH_TYPE_PATTERN: u8 = 2;

pub const MASK_FLAGS_INVERS: u8 = 1 << 0;

pub const ROPD_INVERS_SRC: u16 = 1 << 0;
pub const ROPD_INVERS_BRUSH: u16 = 1 << 1;
pub const ROPD_INVERS_DEST: u16 = 1 << 2;
pub const ROPD_OP_PUT: u16 = 1 << 3;
pub const ROPD_OP_OR: u16 = 1 << 4;
pub const ROPD_OP_AND: u16 = 1 << 5;
pub const ROPD_OP_XOR: u16 = 1 << 6;
pub const ROPD_OP_BLACKNESS: u16 = 1 << 7;
pub const ROPD_OP_WHITENESS: u16 = 1 << 8;
pub const ROPD_OP_INVERS: u16 = 1 << 9;
pub const ROPD_INVERS_RES: u16 = 1 << 10;

pub const IMAGE_SCALE_MODE_INTERPOLATE: u8 = 0;
pub const IMAGE_SCALE_MODE_NEAREST: u8 = 1;

// ---------------------------------------------------------------------------
// Notify
// ---------------------------------------------------------------------------

pub const NOTIFY_SEVERITY_INFO: u32 = 0;
pub const NOTIFY_SEVERITY_WARN: u32 = 1;
pub const NOTIFY_SEVERITY_ERROR: u32 = 2;

/// Human-readable text for a `SpiceLinkErr` / auth code.
pub fn link_error_text(code: u32) -> String {
    let name = match code {
        LINK_ERR_OK => "OK",
        LINK_ERR_ERROR => "ERROR",
        LINK_ERR_INVALID_MAGIC => "INVALID_MAGIC",
        LINK_ERR_INVALID_DATA => "INVALID_DATA",
        LINK_ERR_VERSION_MISMATCH => "VERSION_MISMATCH",
        LINK_ERR_NEED_SECURED => "NEED_SECURED",
        LINK_ERR_NEED_UNSECURED => "NEED_UNSECURED",
        LINK_ERR_PERMISSION_DENIED => "PERMISSION_DENIED",
        LINK_ERR_BAD_CONNECTION_ID => "BAD_CONNECTION_ID",
        LINK_ERR_CHANNEL_NOT_AVAILABLE => "CHANNEL_NOT_AVAILABLE",
        _ => return format!("unknown link error {}", code),
    };
    format!("{} ({})", name, code)
}

// ---------------------------------------------------------------------------
// Reader: read-only little-endian cursor over a message body
// ---------------------------------------------------------------------------

/// A cursor over a borrowed SPICE message body.
///
/// Reads are saturating: reading past the end yields zero bytes and leaves the
/// cursor at the end of the buffer instead of panicking. Callers that care
/// about truncation should check [`Reader::remaining`] / [`Reader::eof`].
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    /// Advance by `n` bytes and return the bytes that were available.
    fn take(&mut self, n: usize) -> &'a [u8] {
        let start = self.pos.min(self.buf.len());
        let end = self.pos.saturating_add(n).min(self.buf.len());
        self.pos = end;
        &self.buf[start..end]
    }

    pub fn u8(&mut self) -> u8 {
        let bytes = self.take(1);
        bytes.first().copied().unwrap_or(0)
    }

    pub fn u16(&mut self) -> u16 {
        let bytes = self.take(2);
        let mut v: u16 = 0;
        for (i, &b) in bytes.iter().enumerate() {
            v |= (b as u16) << (8 * i);
        }
        v
    }

    pub fn u32(&mut self) -> u32 {
        let bytes = self.take(4);
        let mut v: u32 = 0;
        for (i, &b) in bytes.iter().enumerate() {
            v |= (b as u32) << (8 * i);
        }
        v
    }

    pub fn u64(&mut self) -> u64 {
        let bytes = self.take(8);
        let mut v: u64 = 0;
        for (i, &b) in bytes.iter().enumerate() {
            v |= (b as u64) << (8 * i);
        }
        v
    }

    pub fn i32(&mut self) -> i32 {
        self.u32() as i32
    }

    pub fn i64(&mut self) -> i64 {
        self.u64() as i64
    }

    pub fn bytes(&mut self, n: usize) -> &'a [u8] {
        self.take(n)
    }

    /// Bytes left after the cursor.
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    pub fn eof(&self) -> bool {
        self.pos >= self.buf.len()
    }

    /// Absolute offset within the message buffer.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Move the cursor to an absolute offset (used for palette offset pointers).
    /// Offsets past the end are clamped to the end.
    pub fn seek_abs(&mut self, abs: usize) {
        self.pos = abs.min(self.buf.len());
    }

    pub fn buf(&self) -> &'a [u8] {
        self.buf
    }
}

// ---------------------------------------------------------------------------
// Writer: build a little-endian message body
// ---------------------------------------------------------------------------

/// Minimal little-endian payload builder.
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_reads_little_endian() {
        // 0x01, 0x0203, 0x04050607, 0x08090a0b0c0d0e0f, -2, "abc"
        let buf: Vec<u8> = vec![
            0x01, //
            0x03, 0x02, //
            0x07, 0x06, 0x05, 0x04, //
            0x0f, 0x0e, 0x0d, 0x0c, 0x0b, 0x0a, 0x09, 0x08, //
            0xfe, 0xff, 0xff, 0xff, // i32 -2
            0x61, 0x62, 0x63,
        ];
        let mut r = Reader::new(&buf);
        assert_eq!(r.u8(), 0x01);
        assert_eq!(r.u16(), 0x0203);
        assert_eq!(r.u32(), 0x0405_0607);
        assert_eq!(r.u64(), 0x0809_0a0b_0c0d_0e0f);
        assert_eq!(r.i32(), -2);
        assert_eq!(r.pos(), 19);
        assert_eq!(r.bytes(3), b"abc");
        assert_eq!(r.remaining(), 0);
        assert!(r.eof());
        assert_eq!(r.buf().len(), 22);
    }

    #[test]
    fn reader_seek_abs_and_truncated_reads_do_not_panic() {
        let buf: Vec<u8> = vec![0xaa, 0xbb, 0xcc, 0xdd];
        let mut r = Reader::new(&buf);
        assert_eq!(r.u16(), 0xbbaa);
        r.seek_abs(2);
        assert_eq!(r.pos(), 2);
        assert_eq!(r.u16(), 0xddcc);
        r.seek_abs(1000);
        assert_eq!(r.pos(), 4);
        assert!(r.eof());
        assert_eq!(r.u32(), 0);
        assert_eq!(r.u64(), 0);
        assert_eq!(r.i64(), 0);
        assert_eq!(r.bytes(4).len(), 0);

        // A short read yields the bytes that exist.
        let mut r = Reader::new(&buf[..3]);
        assert_eq!(r.u32(), 0x00cc_bbaa);
    }

    #[test]
    fn writer_emits_little_endian() {
        let mut w = Writer::new();
        w.u8(0x01);
        w.u16(0x0203);
        w.u32(0x0405_0607);
        w.u64(0x0809_0a0b_0c0d_0e0f);
        w.i32(-2);
        w.bytes(b"ab");
        assert_eq!(
            w.as_slice(),
            &[
                0x01, 0x03, 0x02, 0x07, 0x06, 0x05, 0x04, 0x0f, 0x0e, 0x0d, 0x0c, 0x0b, 0x0a, 0x09,
                0x08, 0xfe, 0xff, 0xff, 0xff, 0x61, 0x62,
            ]
        );
        assert_eq!(w.into_vec().len(), 21);
    }

    #[test]
    fn channel_type_values_match_protocol_header() {
        // docs/enums.h: MAIN=1, DISPLAY=2, INPUTS=3.
        assert_eq!(CHANNEL_MAIN, 1);
        assert_eq!(CHANNEL_DISPLAY, 2);
        assert_eq!(CHANNEL_INPUTS, 3);
    }

    #[test]
    fn display_message_ids_match_protocol_header() {
        assert_eq!(MSG_DISPLAY_MARK, 102);
        assert_eq!(MSG_DISPLAY_SURFACE_CREATE, 314);
        assert_eq!(MSG_DISPLAY_SURFACE_DESTROY, 315);
        assert_eq!(MSG_DISPLAY_MONITORS_CONFIG, 317);
        assert_eq!(MSG_DISPLAY_DRAW_COMPOSITE, 318);
    }

    #[test]
    fn magic_is_redq() {
        assert_eq!(SPICE_MAGIC.to_le_bytes(), *b"REDQ");
    }
}
