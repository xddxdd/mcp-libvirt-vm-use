//! Display channel capture: draw commands composed into a 24-bit RGB framebuffer,
//! and that framebuffer encoded as a PNG.
//!
//! Ported from the reference client in `docs/spice-html5` (`display.js`,
//! `bitmap.js`, `spicetype.js`, `spicemsg.js`) with the wire layouts from
//! `docs/spice.proto` and the message values from `docs/spice-protocol/enums.h`.
//!
//! **Message values**: PLAN.md listed a few `SPICE_MSG_DISPLAY_*` values that do not
//! match `docs/spice-protocol/enums.h` (they were shifted by up to four, and the
//! `FROM_CACHE` image types were given bitmap-format numbers). The supervisor
//! approved using the verified `enums.h` values everywhere in this file, so
//! `SURFACE_CREATE` is 314, `DRAW_BLEND` is 305, `IMAGE_TYPE_FROM_CACHE` is 103.
//!
//! **Ignored on purpose**: video streams (`SPICE_MSG_DISPLAY_STREAM_*`), monitor
//! configuration and the raster ops we do not approximate (stroke, text, rop3,
//! blackness, whiteness, invers, composite). They leave whatever the framebuffer
//! already holds, so a surface with a live video area comes out with that area
//! blank. An error is reserved for things we cannot parse or decode.
//!
//! Only surfaces are tracked: the primary surface (id 0) is what [`DisplayCapture::png`]
//! returns. Every draw is converted to RGB on the way in, so the guest's surface
//! format never has to be a specific one.

use std::collections::HashMap;

use crate::spice::proto::Reader;
use crate::spice::PngImage;

/// Client message types (`SPICE_MSGC_DISPLAY_*`).
pub const SPICE_MSGC_DISPLAY_INIT: u16 = 101;
pub const SPICE_MSGC_DISPLAY_PREFERRED_COMPRESSION: u16 = 103;

/// Server message types (`SPICE_MSG_DISPLAY_*`), `docs/spice-protocol/enums.h`.
pub const SPICE_MSG_DISPLAY_MODE: u16 = 101;
pub const SPICE_MSG_DISPLAY_MARK: u16 = 102;
pub const SPICE_MSG_DISPLAY_RESET: u16 = 103;
pub const SPICE_MSG_DISPLAY_COPY_BITS: u16 = 104;
pub const SPICE_MSG_DISPLAY_INVAL_LIST: u16 = 105;
pub const SPICE_MSG_DISPLAY_INVAL_ALL_PIXMAPS: u16 = 106;
pub const SPICE_MSG_DISPLAY_INVAL_PALETTE: u16 = 107;
pub const SPICE_MSG_DISPLAY_INVAL_ALL_PALETTES: u16 = 108;
pub const SPICE_MSG_DISPLAY_STREAM_CREATE: u16 = 122;
pub const SPICE_MSG_DISPLAY_STREAM_DATA: u16 = 123;
pub const SPICE_MSG_DISPLAY_STREAM_CLIP: u16 = 124;
pub const SPICE_MSG_DISPLAY_STREAM_DESTROY: u16 = 125;
pub const SPICE_MSG_DISPLAY_STREAM_DESTROY_ALL: u16 = 126;
pub const SPICE_MSG_DISPLAY_DRAW_FILL: u16 = 302;
pub const SPICE_MSG_DISPLAY_DRAW_OPAQUE: u16 = 303;
pub const SPICE_MSG_DISPLAY_DRAW_COPY: u16 = 304;
pub const SPICE_MSG_DISPLAY_DRAW_BLEND: u16 = 305;
pub const SPICE_MSG_DISPLAY_DRAW_BLACKNESS: u16 = 306;
pub const SPICE_MSG_DISPLAY_DRAW_WHITENESS: u16 = 307;
pub const SPICE_MSG_DISPLAY_DRAW_INVERS: u16 = 308;
pub const SPICE_MSG_DISPLAY_DRAW_ROP3: u16 = 309;
pub const SPICE_MSG_DISPLAY_DRAW_STROKE: u16 = 310;
pub const SPICE_MSG_DISPLAY_DRAW_TEXT: u16 = 311;
pub const SPICE_MSG_DISPLAY_DRAW_TRANSPARENT: u16 = 312;
pub const SPICE_MSG_DISPLAY_DRAW_ALPHA_BLEND: u16 = 313;
pub const SPICE_MSG_DISPLAY_SURFACE_CREATE: u16 = 314;
pub const SPICE_MSG_DISPLAY_SURFACE_DESTROY: u16 = 315;
pub const SPICE_MSG_DISPLAY_STREAM_DATA_SIZED: u16 = 316;
pub const SPICE_MSG_DISPLAY_MONITORS_CONFIG: u16 = 317;
pub const SPICE_MSG_DISPLAY_DRAW_COMPOSITE: u16 = 318;
pub const SPICE_MSG_DISPLAY_STREAM_ACTIVATE_REPORT: u16 = 319;
pub const SPICE_MSG_DISPLAY_GL_SCANOUT_UNIX: u16 = 320;
pub const SPICE_MSG_DISPLAY_GL_DRAW: u16 = 321;
pub const SPICE_MSG_DISPLAY_QUALITY_INDICATOR: u16 = 322;
pub const SPICE_MSG_DISPLAY_GL_SCANOUT2_UNIX: u16 = 323;

const SPICE_CLIP_TYPE_RECTS: u8 = 1;

const SPICE_BRUSH_TYPE_NONE: u8 = 0;
const SPICE_BRUSH_TYPE_SOLID: u8 = 1;
const SPICE_BRUSH_TYPE_PATTERN: u8 = 2;

/// `SPICE_ROPD_OP_PUT`, the only raster op we render.
const SPICE_ROPD_OP_PUT: u16 = 1 << 3;

const SPICE_IMAGE_TYPE_BITMAP: u8 = 0;
const SPICE_IMAGE_TYPE_FROM_CACHE: u8 = 103;
const SPICE_IMAGE_TYPE_FROM_CACHE_LOSSLESS: u8 = 106;

/// `SPICE_IMAGE_FLAGS_CACHE_ME`: the server may later refer to this image by id.
const SPICE_IMAGE_FLAGS_CACHE_ME: u8 = 1 << 0;

const SPICE_BITMAP_FMT_1BIT_LE: u8 = 1;
const SPICE_BITMAP_FMT_1BIT_BE: u8 = 2;
const SPICE_BITMAP_FMT_8BIT: u8 = 5;
const SPICE_BITMAP_FMT_16BIT: u8 = 6;
const SPICE_BITMAP_FMT_24BIT: u8 = 7;
const SPICE_BITMAP_FMT_32BIT: u8 = 8;
const SPICE_BITMAP_FMT_RGBA: u8 = 9;

/// `SPICE_BITMAP_FLAGS_PAL_FROM_CACHE`: the palette itself is in the palette cache.
const SPICE_BITMAP_FLAGS_PAL_FROM_CACHE: u8 = 1 << 1;
/// `SPICE_BITMAP_FLAGS_TOP_DOWN`: when absent the rows arrive bottom-up.
const SPICE_BITMAP_FLAGS_TOP_DOWN: u8 = 1 << 2;

/// The primary surface; the one [`DisplayCapture::png`] encodes.
const PRIMARY_SURFACE_ID: u32 = 0;

/// Fixed wire sizes from `docs/spice.proto`.
const RECT_SIZE: usize = 16; // Rect: 4 * int32
const POINT_SIZE: usize = 8; // Point: 2 * int32
const IMAGE_DESCRIPTOR_SIZE: usize = 18; // id u64, type u8, flags u8, width u32, height u32

/// Pixmap cache size advertised to the server (`10 MiB`).
const PIXMAP_CACHE_SIZE: i64 = 10 * 1024 * 1024;
/// Refuse to allocate absurd surfaces/images from a malformed or hostile message.
/// 64M pixels covers an 8K display (33M pixels) with room to spare.
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// Body of `SPICE_MSGC_DISPLAY_INIT`: the pixmap cache we can hold and a disabled
/// GLZ dictionary (`docs/spice.proto`, `SpiceMsgcDisplayInit`).
pub fn display_init_payload(cache_id: u8) -> Vec<u8> {
    let mut payload = Vec::with_capacity(14);
    payload.push(cache_id);
    payload.extend_from_slice(&PIXMAP_CACHE_SIZE.to_le_bytes());
    payload.push(0); // glz_dictionary_id: none, we never accept GLZ images
    payload.extend_from_slice(&0i32.to_le_bytes()); // glz_dictionary_window_size
    payload
}

/// Body of `SPICE_MSGC_DISPLAY_PREFERRED_COMPRESSION`: the single `image_compression`
/// byte. `SpiceSession::screenshot` sends `proto::IMAGE_COMPRESSION_OFF` (1) so
/// bitmaps arrive raw and no QUIC/GLZ/LZ decoder is needed.
pub fn preferred_compression_payload(compression: u8) -> Vec<u8> {
    vec![compression]
}

/// A rectangle as it is read off the wire (`top, left, bottom, right`). Coordinates
/// are signed in `docs/spice.proto`; negatives are read as very large unsigned
/// values and clamped away, which skips the draw rather than wrapping it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    top: u32,
    left: u32,
    bottom: u32,
    right: u32,
}

impl Rect {
    fn width(&self) -> u32 {
        self.right.saturating_sub(self.left)
    }

    fn height(&self) -> u32 {
        self.bottom.saturating_sub(self.top)
    }

    /// The part of this rectangle inside a `width` x `height` area at the origin,
    /// or None when the two do not overlap.
    fn clamp_to(&self, width: u32, height: u32) -> Option<Rect> {
        if self.bottom <= self.top || self.right <= self.left {
            return None;
        }
        if self.left >= width || self.top >= height {
            return None;
        }
        Some(Rect {
            top: self.top,
            left: self.left,
            bottom: self.bottom.min(height),
            right: self.right.min(width),
        })
    }
}

/// `DisplayBase`: the surface, the destination box and the clip (`docs/spice.proto`).
#[derive(Clone, Copy, Debug)]
struct DisplayBase {
    surface_id: u32,
    box_: Rect,
}

/// An image decoded to 24-bit RGB, rows top-down, 3 bytes per pixel.
#[derive(Debug)]
struct DecodedImage {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

impl DecodedImage {
    /// The pixel at (x, y); out-of-range coordinates read as black rather than
    /// panicking (callers clamp, this is the safety net).
    fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let offset = rgb_offset(self.width, x, y);
        match self.rgb.get(offset..offset + 3) {
            Some(pixel) => [pixel[0], pixel[1], pixel[2]],
            None => [0, 0, 0],
        }
    }
}

/// Byte offset of pixel (x, y) in a 3-bytes-per-pixel buffer that is `width` wide.
fn rgb_offset(width: u32, x: u32, y: u32) -> usize {
    (y as usize * width as usize + x as usize) * 3
}

/// One SPICE surface's framebuffer: 24-bit RGB, row-major, row 0 at the top.
struct Surface {
    id: u32,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    /// Set once a drawing command has written into the framebuffer.
    drawn: bool,
}

impl Surface {
    fn new(id: u32, width: u32, height: u32) -> Surface {
        Surface {
            id,
            width,
            height,
            pixels: vec![0u8; width as usize * height as usize * 3],
            drawn: false,
        }
    }

    /// Fill `rect` (already clamped to this surface) with `rgb`.
    fn fill(&mut self, rect: Rect, rgb: [u8; 3]) {
        let row_bytes = self.width as usize * 3;
        let start = rect.left as usize * 3;
        let len = rect.width() as usize * 3;
        let mut row = Vec::with_capacity(len);
        for _ in 0..rect.width() {
            row.extend_from_slice(&rgb);
        }
        for y in rect.top..rect.bottom {
            let base = y as usize * row_bytes + start;
            if let Some(dst) = self.pixels.get_mut(base..base + len) {
                dst.copy_from_slice(&row);
            }
        }
        self.drawn = true;
    }
}

/// Blit `src_area` of `image` into `dst`, nearest-neighbour scaling when the two
/// have different sizes. Both rectangles must already be clamped.
fn blit_rgb(surface: &mut Surface, dst: Rect, image: &DecodedImage, src_area: Rect) {
    let dst_width = dst.width();
    let dst_height = dst.height();
    let src_width = src_area.width();
    let src_height = src_area.height();
    if dst_width == 0 || dst_height == 0 || src_width == 0 || src_height == 0 {
        return;
    }

    if src_width == dst_width && src_height == dst_height {
        let len = dst_width as usize * 3;
        for row in 0..dst_height {
            let src_offset = rgb_offset(image.width, src_area.left, src_area.top + row);
            let dst_offset = rgb_offset(surface.width, dst.left, dst.top + row);
            if let (Some(src), Some(dst)) = (
                image.rgb.get(src_offset..src_offset + len),
                surface.pixels.get_mut(dst_offset..dst_offset + len),
            ) {
                dst.copy_from_slice(src);
            }
        }
        surface.drawn = true;
        return;
    }

    for row in 0..dst_height {
        let sy = src_area.top + (row as u64 * src_height as u64 / dst_height as u64) as u32;
        for col in 0..dst_width {
            let sx = src_area.left + (col as u64 * src_width as u64 / dst_width as u64) as u32;
            let rgb = image.pixel(sx, sy);
            let offset = rgb_offset(surface.width, dst.left + col, dst.top + row);
            if let Some(pixel) = surface.pixels.get_mut(offset..offset + 3) {
                pixel.copy_from_slice(&rgb);
            }
        }
    }
    surface.drawn = true;
}

/// `0x00RRGGBB` (the layout of a solid brush colour and of a palette entry) as RGB.
fn rgb_from_color(color: u32) -> [u8; 3] {
    [(color >> 16) as u8, (color >> 8) as u8, color as u8]
}

fn le_u16(data: &[u8], offset: usize) -> u16 {
    match data.get(offset..offset + 2) {
        Some(bytes) => u16::from_le_bytes([bytes[0], bytes[1]]),
        None => 0,
    }
}

fn le_u32(data: &[u8], offset: usize) -> u32 {
    match data.get(offset..offset + 4) {
        Some(bytes) => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        None => 0,
    }
}

fn byte_at(data: &[u8], offset: usize) -> u8 {
    data.get(offset).copied().unwrap_or(0)
}

/// Expand a 5-bit colour channel to 8 bits.
fn expand_5bit(value: u16) -> u8 {
    let value = value as u8;
    (value << 3) | (value >> 2)
}

/// Fail early and clearly instead of letting the reader run off the end of the body.
fn need(reader: &Reader, bytes: usize, what: &str) -> Result<(), String> {
    if reader.remaining() < bytes {
        return Err(format!(
            "{}: message body is truncated ({} bytes left, {} needed)",
            what,
            reader.remaining(),
            bytes
        ));
    }
    Ok(())
}

/// Human-readable name of a display message, for error messages.
fn message_name(msg_type: u16) -> &'static str {
    match msg_type {
        SPICE_MSG_DISPLAY_SURFACE_CREATE => "SPICE_MSG_DISPLAY_SURFACE_CREATE",
        SPICE_MSG_DISPLAY_SURFACE_DESTROY => "SPICE_MSG_DISPLAY_SURFACE_DESTROY",
        SPICE_MSG_DISPLAY_COPY_BITS => "SPICE_MSG_DISPLAY_COPY_BITS",
        SPICE_MSG_DISPLAY_DRAW_FILL => "SPICE_MSG_DISPLAY_DRAW_FILL",
        SPICE_MSG_DISPLAY_DRAW_OPAQUE => "SPICE_MSG_DISPLAY_DRAW_OPAQUE",
        SPICE_MSG_DISPLAY_DRAW_COPY => "SPICE_MSG_DISPLAY_DRAW_COPY",
        SPICE_MSG_DISPLAY_DRAW_BLEND => "SPICE_MSG_DISPLAY_DRAW_BLEND",
        SPICE_MSG_DISPLAY_DRAW_TRANSPARENT => "SPICE_MSG_DISPLAY_DRAW_TRANSPARENT",
        SPICE_MSG_DISPLAY_DRAW_ALPHA_BLEND => "SPICE_MSG_DISPLAY_DRAW_ALPHA_BLEND",
        SPICE_MSG_DISPLAY_INVAL_LIST => "SPICE_MSG_DISPLAY_INVAL_LIST",
        _ => "display message",
    }
}

fn read_rect(reader: &mut Reader) -> Rect {
    Rect {
        top: reader.u32(),
        left: reader.u32(),
        bottom: reader.u32(),
        right: reader.u32(),
    }
}

/// Read `DisplayBase`: surface id, box, clip. Clip rectangles are consumed but not
/// applied — the box already describes the region the server wants updated.
fn read_display_base(reader: &mut Reader, what: &str) -> Result<DisplayBase, String> {
    need(reader, 4 + RECT_SIZE + 1, what)?;
    let surface_id = reader.u32();
    let box_ = read_rect(reader);
    let clip_type = reader.u8();
    if clip_type == SPICE_CLIP_TYPE_RECTS {
        need(reader, 4, what)?;
        let count = reader.u32() as usize;
        let bytes = count
            .checked_mul(RECT_SIZE)
            .ok_or_else(|| format!("{}: implausible clip rectangle count {}", what, count))?;
        need(reader, bytes, what)?;
        let _clip_rects = reader.bytes(bytes);
    }
    Ok(DisplayBase { surface_id, box_ })
}

/// Read a `QMask` and return the absolute offset of its bitmap (0 = no mask).
/// Masks are not applied: they are used for cursor shadows and anti-aliased text,
/// and the region is drawn unmasked instead — the same simplification the reference
/// client makes.
fn read_qmask_offset(reader: &mut Reader, what: &str) -> Result<usize, String> {
    need(reader, 1 + POINT_SIZE + 4, what)?;
    let _flags = reader.u8();
    let _pos_x = reader.u32();
    let _pos_y = reader.u32();
    Ok(reader.u32() as usize)
}

enum Brush {
    None,
    Solid(u32),
    Pattern,
}

fn read_brush(reader: &mut Reader, what: &str) -> Result<Brush, String> {
    need(reader, 1, what)?;
    match reader.u8() {
        SPICE_BRUSH_TYPE_NONE => Ok(Brush::None),
        SPICE_BRUSH_TYPE_SOLID => {
            need(reader, 4, what)?;
            Ok(Brush::Solid(reader.u32()))
        }
        // Pattern = { Image *pat, Point pos }: 12 bytes, and the pattern image is
        // not decoded, so a patterned fill is skipped by the caller.
        SPICE_BRUSH_TYPE_PATTERN => {
            need(reader, 12, what)?;
            let _pattern_offset = reader.u32();
            let _pos_x = reader.u32();
            let _pos_y = reader.u32();
            Ok(Brush::Pattern)
        }
        other => Err(format!("{}: unsupported brush type {}", what, other)),
    }
}

#[derive(Clone, Copy, Debug)]
struct ImageDescriptor {
    id: u64,
    type_: u8,
    flags: u8,
}

fn read_image_descriptor(reader: &mut Reader) -> ImageDescriptor {
    let id = reader.u64();
    let type_ = reader.u8();
    let flags = reader.u8();
    // width and height are part of the descriptor, but a bitmap carries its own
    // x/y and those are what the pixel data is laid out for.
    let _width = reader.u32();
    let _height = reader.u32();
    ImageDescriptor { id, type_, flags }
}

/// Read the image descriptor that `offset` points at inside `body`.
fn image_descriptor_at(body: &[u8], offset: usize, what: &str) -> Result<ImageDescriptor, String> {
    let end = offset
        .checked_add(IMAGE_DESCRIPTOR_SIZE)
        .ok_or_else(|| format!("{}: implausible image offset {}", what, offset))?;
    if end > body.len() {
        return Err(format!(
            "{}: image offset {} is outside the {}-byte message body",
            what,
            offset,
            body.len()
        ));
    }
    let mut reader = Reader::new(body);
    reader.seek_abs(offset);
    Ok(read_image_descriptor(&mut reader))
}

/// Read a `Palette` at an absolute offset in `body`: `unique u64, num_ents u16,
/// ents[num_ents] u32`. Entries are `0x00RRGGBB`, the same layout as a brush colour.
fn read_palette(body: &[u8], offset: usize, what: &str) -> Result<Vec<u32>, String> {
    let end = offset
        .checked_add(10)
        .ok_or_else(|| format!("{}: implausible palette offset {}", what, offset))?;
    if end > body.len() {
        return Err(format!(
            "{}: palette offset {} is outside the {}-byte message body",
            what,
            offset,
            body.len()
        ));
    }
    let mut reader = Reader::new(body);
    reader.seek_abs(offset);
    let _unique = reader.u64();
    let count = reader.u16() as usize;
    let bytes = count * 4;
    if reader.remaining() < bytes {
        return Err(format!(
            "{}: palette of {} entries does not fit in the message body",
            what, count
        ));
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(reader.u32());
    }
    Ok(entries)
}

/// Decode the image whose descriptor sits at `offset` in `body`; `None` for a null
/// pointer. Images that are not plain bitmaps (QUIC, LZ, JPEG, a surface reference)
/// are an error: with preferred compression OFF the server only sends bitmaps.
fn decode_image(
    body: &[u8],
    offset: usize,
    what: &str,
) -> Result<Option<(ImageDescriptor, DecodedImage)>, String> {
    if offset == 0 {
        return Ok(None);
    }
    let descriptor = image_descriptor_at(body, offset, what)?;
    if descriptor.type_ != SPICE_IMAGE_TYPE_BITMAP {
        return Err(format!("unsupported image type {}", descriptor.type_));
    }
    let mut reader = Reader::new(body);
    reader.seek_abs(offset + IMAGE_DESCRIPTOR_SIZE);
    let image = decode_bitmap(&mut reader, body, what)?;
    Ok(Some((descriptor, image)))
}

/// Decode a `SpiceBitmap` (`docs/spice.proto`): format, flags, x, y, stride, an
/// optional palette, then exactly `stride * y` raw pixel bytes. There is **no chunk
/// header** in front of the pixel data: the demarshaller wraps the remainder of the
/// message as a single synthetic chunk (`docs/codegen/demarshal.py`).
fn decode_bitmap(reader: &mut Reader, body: &[u8], what: &str) -> Result<DecodedImage, String> {
    // BitmapData: format u8, flags u8, x u32, y u32, stride u32.
    need(reader, 14, what)?;
    let format = reader.u8();
    let flags = reader.u8();
    let width = reader.u32();
    let height = reader.u32();
    let stride = reader.u32();

    if width == 0 || height == 0 {
        return Err(format!("{}: bitmap is {}x{}", what, width, height));
    }
    if width as u64 * height as u64 > MAX_PIXELS {
        return Err(format!(
            "{}: bitmap {}x{} is too large",
            what, width, height
        ));
    }

    let palette = if flags & SPICE_BITMAP_FLAGS_PAL_FROM_CACHE != 0 {
        need(reader, 8, what)?;
        let palette_id = reader.u64();
        return Err(format!(
            "{}: bitmap uses palette {} from the palette cache, which is not supported",
            what, palette_id
        ));
    } else {
        need(reader, 4, what)?;
        let offset = reader.u32() as usize;
        if offset == 0 {
            None
        } else {
            Some(read_palette(body, offset, what)?)
        }
    };

    let data_len = stride as u64 * height as u64;
    if data_len > reader.remaining() as u64 {
        return Err(format!(
            "{}: bitmap needs {} bytes of pixels but the message body has {} left",
            what,
            data_len,
            reader.remaining()
        ));
    }
    let data = reader.bytes(data_len as usize);

    decode_bitmap_pixels(
        what,
        format,
        flags,
        width,
        height,
        stride,
        data,
        palette.as_deref(),
    )
}

/// Convert raw bitmap bytes into RGB. Rows are bottom-up unless
/// [`SPICE_BITMAP_FLAGS_TOP_DOWN`] is set (`bitmap.js` in the reference client).
#[allow(clippy::too_many_arguments)]
fn decode_bitmap_pixels(
    what: &str,
    format: u8,
    flags: u8,
    width: u32,
    height: u32,
    stride: u32,
    data: &[u8],
    palette: Option<&[u32]>,
) -> Result<DecodedImage, String> {
    let row_bytes = match format {
        SPICE_BITMAP_FMT_32BIT | SPICE_BITMAP_FMT_RGBA => width.checked_mul(4),
        SPICE_BITMAP_FMT_24BIT => width.checked_mul(3),
        SPICE_BITMAP_FMT_16BIT => width.checked_mul(2),
        SPICE_BITMAP_FMT_8BIT => Some(width),
        SPICE_BITMAP_FMT_1BIT_LE | SPICE_BITMAP_FMT_1BIT_BE => Some((width + 7) / 8),
        other => return Err(format!("{}: unsupported bitmap format {}", what, other)),
    };
    let row_bytes =
        row_bytes.ok_or_else(|| format!("{}: bitmap width {} is out of range", what, width))?;
    if stride < row_bytes {
        return Err(format!(
            "{}: bitmap stride {} is smaller than the {} bytes of one row",
            what, stride, row_bytes
        ));
    }
    if format == SPICE_BITMAP_FMT_8BIT && palette.is_none() {
        return Err(format!("{}: 8BIT bitmap has no palette", what));
    }

    let top_down = flags & SPICE_BITMAP_FLAGS_TOP_DOWN != 0;
    let bytes_per_pixel = match format {
        SPICE_BITMAP_FMT_32BIT | SPICE_BITMAP_FMT_RGBA => 4,
        SPICE_BITMAP_FMT_24BIT => 3,
        SPICE_BITMAP_FMT_16BIT => 2,
        SPICE_BITMAP_FMT_8BIT => 1,
        _ => 0, // the 1-bit formats index into their own row of packed bits
    };
    let mut rgb = vec![0u8; width as usize * height as usize * 3];

    for row in 0..height {
        let source_row = if top_down { row } else { height - 1 - row };
        let row_start = (source_row as u64 * stride as u64) as usize;
        let row_out = row as usize * width as usize * 3;
        for col in 0..width {
            let col_start = row_start + col as usize * bytes_per_pixel;
            let pixel: [u8; 3] = match format {
                // Memory order is B, G, R, A/X: the low byte of the little-endian
                // word is blue. 32BIT ignores the fourth byte, RGBA ignores alpha
                // (the framebuffer has no alpha channel).
                SPICE_BITMAP_FMT_32BIT | SPICE_BITMAP_FMT_RGBA => {
                    let word = le_u32(data, col_start);
                    rgb_from_color(word & 0x00ff_ffff)
                }
                // 24BIT is packed B, G, R.
                SPICE_BITMAP_FMT_24BIT => [
                    byte_at(data, col_start + 2),
                    byte_at(data, col_start + 1),
                    byte_at(data, col_start),
                ],
                // 16BIT is little-endian 555 with blue in the low bits, like the
                // blue-low 32-bit layout above.
                SPICE_BITMAP_FMT_16BIT => {
                    let word = le_u16(data, col_start);
                    [
                        expand_5bit((word >> 10) & 0x1f),
                        expand_5bit((word >> 5) & 0x1f),
                        expand_5bit(word & 0x1f),
                    ]
                }
                SPICE_BITMAP_FMT_8BIT => {
                    let index = byte_at(data, col_start) as usize;
                    let entry = palette
                        .and_then(|ents| ents.get(index).copied())
                        .unwrap_or(0);
                    rgb_from_color(entry)
                }
                SPICE_BITMAP_FMT_1BIT_LE | SPICE_BITMAP_FMT_1BIT_BE => {
                    let byte = byte_at(data, col_start + col as usize / 8);
                    let shift = if format == SPICE_BITMAP_FMT_1BIT_LE {
                        col % 8
                    } else {
                        7 - col % 8
                    };
                    let bit = (byte >> shift) & 1;
                    match palette {
                        Some(ents) => {
                            let fallback = if bit == 1 { 0x00ff_ffff } else { 0 };
                            rgb_from_color(ents.get(bit as usize).copied().unwrap_or(fallback))
                        }
                        // No palette: the reference client's default is black/white.
                        None => {
                            if bit == 1 {
                                [255, 255, 255]
                            } else {
                                [0, 0, 0]
                            }
                        }
                    }
                }
                other => return Err(format!("{}: unsupported bitmap format {}", what, other)),
            };

            let out = row_out + col as usize * 3;
            if let Some(slot) = rgb.get_mut(out..out + 3) {
                slot.copy_from_slice(&pixel);
            }
        }
    }

    Ok(DecodedImage { width, height, rgb })
}

/// Framebuffer capture for one display channel.
pub struct DisplayCapture {
    surfaces: Vec<Surface>,
    /// Decoded images the server asked us to remember, by descriptor id.
    cache: HashMap<u64, DecodedImage>,
}

impl DisplayCapture {
    pub fn new() -> Self {
        DisplayCapture {
            surfaces: Vec::new(),
            cache: HashMap::new(),
        }
    }

    /// Process one display-channel server message body.
    ///
    /// `Ok(())` covers both handled messages and the ones this capture ignores on
    /// purpose (see the module documentation). `Err` is fatal for the capture: a
    /// body we cannot parse, an image we cannot decode, or a cached image the
    /// server never sent.
    pub fn handle_message(&mut self, msg_type: u16, body: &[u8]) -> Result<(), String> {
        match msg_type {
            SPICE_MSG_DISPLAY_SURFACE_CREATE => self.surface_create(body),
            SPICE_MSG_DISPLAY_SURFACE_DESTROY => self.surface_destroy(body),
            SPICE_MSG_DISPLAY_COPY_BITS => self.copy_bits(body),
            SPICE_MSG_DISPLAY_DRAW_FILL => self.draw_fill(body),
            SPICE_MSG_DISPLAY_DRAW_OPAQUE => self.draw_opaque(body),
            SPICE_MSG_DISPLAY_DRAW_COPY | SPICE_MSG_DISPLAY_DRAW_BLEND => {
                self.draw_copy(body, msg_type)
            }
            SPICE_MSG_DISPLAY_DRAW_TRANSPARENT => self.draw_transparent(body),
            SPICE_MSG_DISPLAY_DRAW_ALPHA_BLEND => self.draw_alpha_blend(body),
            SPICE_MSG_DISPLAY_INVAL_LIST => self.inval_list(body),
            SPICE_MSG_DISPLAY_INVAL_ALL_PIXMAPS => {
                self.cache.clear();
                Ok(())
            }
            // MARK announces a full redraw that follows; the framebuffer is already
            // in a defined state (black on creation), so nothing to reset.
            SPICE_MSG_DISPLAY_MARK => Ok(()),
            // RESET discards the client-side picture. The pixmap cache is separately
            // invalidated by the server through INVAL_ALL_PIXMAPS/INVAL_LIST, so it
            // is kept: a reset must not turn into a cache miss.
            SPICE_MSG_DISPLAY_RESET => {
                self.reset();
                Ok(())
            }
            // The primary surface's geometry arrives with SURFACE_CREATE; MODE and
            // MONITORS_CONFIG only describe monitors.
            SPICE_MSG_DISPLAY_MODE | SPICE_MSG_DISPLAY_MONITORS_CONFIG => Ok(()),
            // We hold no palette cache, so palette invalidations are no-ops.
            SPICE_MSG_DISPLAY_INVAL_PALETTE | SPICE_MSG_DISPLAY_INVAL_ALL_PALETTES => Ok(()),
            // Ignored, as PLAN.md specifies: video streams and the raster ops we do
            // not approximate. They leave the previous framebuffer content.
            SPICE_MSG_DISPLAY_STREAM_CREATE
            | SPICE_MSG_DISPLAY_STREAM_DATA
            | SPICE_MSG_DISPLAY_STREAM_CLIP
            | SPICE_MSG_DISPLAY_STREAM_DESTROY
            | SPICE_MSG_DISPLAY_STREAM_DESTROY_ALL
            | SPICE_MSG_DISPLAY_STREAM_DATA_SIZED
            | SPICE_MSG_DISPLAY_STREAM_ACTIVATE_REPORT
            | SPICE_MSG_DISPLAY_DRAW_BLACKNESS
            | SPICE_MSG_DISPLAY_DRAW_WHITENESS
            | SPICE_MSG_DISPLAY_DRAW_INVERS
            | SPICE_MSG_DISPLAY_DRAW_ROP3
            | SPICE_MSG_DISPLAY_DRAW_STROKE
            | SPICE_MSG_DISPLAY_DRAW_TEXT
            | SPICE_MSG_DISPLAY_DRAW_COMPOSITE
            | SPICE_MSG_DISPLAY_GL_SCANOUT_UNIX
            | SPICE_MSG_DISPLAY_GL_DRAW
            | SPICE_MSG_DISPLAY_QUALITY_INDICATOR
            | SPICE_MSG_DISPLAY_GL_SCANOUT2_UNIX => Ok(()),
            other => Err(format!("unexpected display message type {}", other)),
        }
    }

    /// The primary surface as a PNG, or `None` until surface 0 has been drawn at
    /// least once.
    pub fn png(&self) -> Option<PngImage> {
        let surface = self.surfaces.iter().find(|s| s.id == PRIMARY_SURFACE_ID)?;
        if !surface.drawn {
            return None;
        }

        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, surface.width, surface.height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().ok()?;
            writer.write_image_data(&surface.pixels).ok()?;
        }
        Some(PngImage {
            width: surface.width,
            height: surface.height,
            png,
        })
    }

    /// `SPICE_MSG_DISPLAY_SURFACE_CREATE`: allocate a black framebuffer. Draws to a
    /// surface we never saw are ignored, so a replay of a draw before its surface is
    /// not fatal.
    fn surface_create(&mut self, body: &[u8]) -> Result<(), String> {
        let what = message_name(SPICE_MSG_DISPLAY_SURFACE_CREATE);
        let mut reader = Reader::new(body);
        need(&reader, 20, what)?;
        let surface_id = reader.u32();
        let width = reader.u32();
        let height = reader.u32();
        let _format = reader.u32();
        let _flags = reader.u32();

        if width == 0 || height == 0 {
            return Err(format!(
                "{}: surface {} is {}x{}",
                what, surface_id, width, height
            ));
        }
        if width as u64 * height as u64 > MAX_PIXELS {
            return Err(format!(
                "{}: surface {} is too large at {}x{}",
                what, surface_id, width, height
            ));
        }

        self.surfaces.retain(|s| s.id != surface_id);
        self.surfaces.push(Surface::new(surface_id, width, height));
        Ok(())
    }

    /// `SPICE_MSG_DISPLAY_SURFACE_DESTROY`.
    fn surface_destroy(&mut self, body: &[u8]) -> Result<(), String> {
        let what = message_name(SPICE_MSG_DISPLAY_SURFACE_DESTROY);
        let mut reader = Reader::new(body);
        need(&reader, 4, what)?;
        let surface_id = reader.u32();
        self.surfaces.retain(|s| s.id != surface_id);
        Ok(())
    }

    /// `SPICE_MSG_DISPLAY_RESET`: every framebuffer goes back to black.
    fn reset(&mut self) {
        for surface in &mut self.surfaces {
            surface.pixels.iter_mut().for_each(|byte| *byte = 0);
            surface.drawn = false;
        }
    }

    /// `SPICE_MSG_DISPLAY_COPY_BITS`: move pixels inside one surface, using a
    /// temporary buffer so overlapping source and destination stay correct.
    fn copy_bits(&mut self, body: &[u8]) -> Result<(), String> {
        let what = message_name(SPICE_MSG_DISPLAY_COPY_BITS);
        let mut reader = Reader::new(body);
        let base = read_display_base(&mut reader, what)?;
        need(&reader, POINT_SIZE, what)?;
        let src_x = reader.u32();
        let src_y = reader.u32();

        let Some(surface) = self.surfaces.iter_mut().find(|s| s.id == base.surface_id) else {
            return Ok(());
        };
        let Some(dst) = base.box_.clamp_to(surface.width, surface.height) else {
            return Ok(());
        };
        // The reference client copies min(box size, surface extent from src_pos).
        let width = dst.width().min(surface.width.saturating_sub(src_x));
        let height = dst.height().min(surface.height.saturating_sub(src_y));
        if width == 0 || height == 0 {
            return Ok(());
        }

        let stride = surface.width as usize * 3;
        let row_bytes = width as usize * 3;
        let mut scratch = vec![0u8; row_bytes * height as usize];
        for row in 0..height as usize {
            let src_offset = (src_y as usize + row) * stride + src_x as usize * 3;
            match (
                surface.pixels.get(src_offset..src_offset + row_bytes),
                scratch.get_mut(row * row_bytes..(row + 1) * row_bytes),
            ) {
                (Some(src), Some(dst)) => dst.copy_from_slice(src),
                _ => return Ok(()),
            }
        }
        for row in 0..height as usize {
            let dst_offset = (dst.top as usize + row) * stride + dst.left as usize * 3;
            if let (Some(src), Some(dst_slot)) = (
                scratch.get(row * row_bytes..(row + 1) * row_bytes),
                surface.pixels.get_mut(dst_offset..dst_offset + row_bytes),
            ) {
                dst_slot.copy_from_slice(src);
            }
        }
        surface.drawn = true;
        Ok(())
    }

    /// `SPICE_MSG_DISPLAY_DRAW_FILL`: a solid brush fills the box. Patterned brushes
    /// and other raster ops are skipped (a pattern needs its pattern image decoded).
    fn draw_fill(&mut self, body: &[u8]) -> Result<(), String> {
        let what = message_name(SPICE_MSG_DISPLAY_DRAW_FILL);
        let mut reader = Reader::new(body);
        let base = read_display_base(&mut reader, what)?;
        let brush = read_brush(&mut reader, what)?;
        need(&reader, 2, what)?;
        let rop = reader.u16();
        let _mask = read_qmask_offset(&mut reader, what)?;

        if rop != SPICE_ROPD_OP_PUT {
            return Ok(());
        }
        let Brush::Solid(color) = brush else {
            return Ok(());
        };
        let Some(surface) = self.surfaces.iter_mut().find(|s| s.id == base.surface_id) else {
            return Ok(());
        };
        if let Some(rect) = base.box_.clamp_to(surface.width, surface.height) {
            surface.fill(rect, rgb_from_color(color));
        }
        Ok(())
    }

    /// `SPICE_MSG_DISPLAY_DRAW_OPAQUE`: like DRAW_COPY, plus a brush that names the
    /// transparency colour of the source. The source is blitted as-is.
    fn draw_opaque(&mut self, body: &[u8]) -> Result<(), String> {
        let what = message_name(SPICE_MSG_DISPLAY_DRAW_OPAQUE);
        let mut reader = Reader::new(body);
        let base = read_display_base(&mut reader, what)?;
        need(&reader, 4 + RECT_SIZE, what)?;
        let src_offset = reader.u32() as usize;
        let src_area = read_rect(&mut reader);
        let _brush = read_brush(&mut reader, what)?;
        need(&reader, 2 + 1, what)?;
        let _rop = reader.u16();
        let _scale_mode = reader.u8();
        let _mask = read_qmask_offset(&mut reader, what)?;
        self.composite_image(body, base, src_offset, src_area, what)
    }

    /// `SPICE_MSG_DISPLAY_DRAW_COPY` (304) and `DRAW_BLEND` (305), which share a body.
    fn draw_copy(&mut self, body: &[u8], msg_type: u16) -> Result<(), String> {
        let what = message_name(msg_type);
        let mut reader = Reader::new(body);
        let base = read_display_base(&mut reader, what)?;
        need(&reader, 4 + RECT_SIZE + 2 + 1, what)?;
        let src_offset = reader.u32() as usize;
        let src_area = read_rect(&mut reader);
        let _rop = reader.u16();
        let _scale_mode = reader.u8();
        let _mask = read_qmask_offset(&mut reader, what)?;
        self.composite_image(body, base, src_offset, src_area, what)
    }

    /// `SPICE_MSG_DISPLAY_DRAW_TRANSPARENT`: the source bitmap is blitted without
    /// colour-keying it against `src_color`/`true_color`.
    fn draw_transparent(&mut self, body: &[u8]) -> Result<(), String> {
        let what = message_name(SPICE_MSG_DISPLAY_DRAW_TRANSPARENT);
        let mut reader = Reader::new(body);
        let base = read_display_base(&mut reader, what)?;
        need(&reader, 4 + RECT_SIZE + 4 + 4, what)?;
        let src_offset = reader.u32() as usize;
        let src_area = read_rect(&mut reader);
        let _src_color = reader.u32();
        let _true_color = reader.u32();
        self.composite_image(body, base, src_offset, src_area, what)
    }

    /// `SPICE_MSG_DISPLAY_DRAW_ALPHA_BLEND`: blitted like a copy; the per-draw alpha
    /// byte is ignored because the framebuffer has no alpha channel.
    fn draw_alpha_blend(&mut self, body: &[u8]) -> Result<(), String> {
        let what = message_name(SPICE_MSG_DISPLAY_DRAW_ALPHA_BLEND);
        let mut reader = Reader::new(body);
        let base = read_display_base(&mut reader, what)?;
        need(&reader, 1 + 1 + 4 + RECT_SIZE, what)?;
        let _alpha_flags = reader.u8();
        let _alpha = reader.u8();
        let src_offset = reader.u32() as usize;
        let src_area = read_rect(&mut reader);
        self.composite_image(body, base, src_offset, src_area, what)
    }

    /// `SPICE_MSG_DISPLAY_INVAL_LIST`: the listed resources leave the pixmap cache.
    fn inval_list(&mut self, body: &[u8]) -> Result<(), String> {
        let what = message_name(SPICE_MSG_DISPLAY_INVAL_LIST);
        let mut reader = Reader::new(body);
        need(&reader, 2, what)?;
        let count = reader.u16() as usize;
        need(&reader, count * 9, what)?;
        for _ in 0..count {
            let _resource_type = reader.u8();
            let id = reader.u64();
            self.cache.remove(&id);
        }
        Ok(())
    }

    /// Draw the image at `src_offset` into `base`. An inline bitmap is decoded (and
    /// cached when it asks to be); a `FROM_CACHE` reference reuses the decoded image.
    fn composite_image(
        &mut self,
        body: &[u8],
        base: DisplayBase,
        src_offset: usize,
        src_area: Rect,
        what: &str,
    ) -> Result<(), String> {
        if src_offset == 0 {
            return Ok(());
        }
        let descriptor = image_descriptor_at(body, src_offset, what)?;

        if descriptor.type_ == SPICE_IMAGE_TYPE_FROM_CACHE
            || descriptor.type_ == SPICE_IMAGE_TYPE_FROM_CACHE_LOSSLESS
        {
            let Some(image) = self.cache.get(&descriptor.id) else {
                return Err(format!(
                    "{}: image {} is referenced from the cache but was never sent",
                    what, descriptor.id
                ));
            };
            // Disjoint fields: the cache is borrowed, the destination surface mutated.
            if let Some(surface) = self.surfaces.iter_mut().find(|s| s.id == base.surface_id) {
                if let (Some(dst), Some(src)) = (
                    base.box_.clamp_to(surface.width, surface.height),
                    src_area.clamp_to(image.width, image.height),
                ) {
                    blit_rgb(surface, dst, image, src);
                }
            }
            return Ok(());
        }

        let Some((descriptor, image)) = decode_image(body, src_offset, what)? else {
            return Ok(());
        };
        if let Some(surface) = self.surfaces.iter_mut().find(|s| s.id == base.surface_id) {
            if let (Some(dst), Some(src)) = (
                base.box_.clamp_to(surface.width, surface.height),
                src_area.clamp_to(image.width, image.height),
            ) {
                blit_rgb(surface, dst, &image, src);
            }
        }
        // The reference client caches the image when the draw that carried it runs,
        // which is here: any FROM_CACHE reference arrives in a later message.
        if descriptor.flags & SPICE_IMAGE_FLAGS_CACHE_ME != 0 {
            self.cache.insert(descriptor.id, image);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16b(value: u16, out: &mut Vec<u8>) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn u32b(value: u32, out: &mut Vec<u8>) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn u64b(value: u64, out: &mut Vec<u8>) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn rect_bytes(top: u32, left: u32, bottom: u32, right: u32, out: &mut Vec<u8>) {
        u32b(top, out);
        u32b(left, out);
        u32b(bottom, out);
        u32b(right, out);
    }

    /// `SpiceMsgSurfaceCreate` body.
    fn surface_create_body(id: u32, width: u32, height: u32) -> Vec<u8> {
        let mut body = Vec::new();
        u32b(id, &mut body);
        u32b(width, &mut body);
        u32b(height, &mut body);
        u32b(32, &mut body); // SPICE_SURFACE_FMT_32_xRGB
        u32b(1, &mut body); // SPICE_SURFACE_FLAGS_PRIMARY
        body
    }

    /// `DisplayBase` with no clip rectangles.
    fn display_base(body: &mut Vec<u8>, surface_id: u32, rect: (u32, u32, u32, u32)) {
        u32b(surface_id, body);
        rect_bytes(rect.0, rect.1, rect.2, rect.3, body);
        body.push(0); // SPICE_CLIP_TYPE_NONE
    }

    /// A null `QMask`: flags, position, null bitmap pointer.
    fn qmask_none(body: &mut Vec<u8>) {
        body.push(0);
        u32b(0, body);
        u32b(0, body);
        u32b(0, body);
    }

    /// An `Image` body: descriptor, a `SpiceBitmap` header, an optional absolute
    /// palette pointer and raw pixels with no chunk header.
    fn bitmap_image_body(
        id: u64,
        descriptor_flags: u8,
        format: u8,
        bitmap_flags: u8,
        width: u32,
        height: u32,
        stride: u32,
        palette_absolute: u32,
        pixels: &[u8],
    ) -> Vec<u8> {
        let mut body = Vec::new();
        u64b(id, &mut body);
        body.push(SPICE_IMAGE_TYPE_BITMAP);
        body.push(descriptor_flags);
        u32b(width, &mut body);
        u32b(height, &mut body);
        body.push(format);
        body.push(bitmap_flags);
        u32b(width, &mut body);
        u32b(height, &mut body);
        u32b(stride, &mut body);
        u32b(palette_absolute, &mut body);
        body.extend_from_slice(pixels);
        body
    }

    fn pixel_at(rgb: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
        let offset = (y as usize * width as usize + x as usize) * 3;
        match rgb.get(offset..offset + 3) {
            Some(pixel) => [pixel[0], pixel[1], pixel[2]],
            None => panic!("pixel ({x}, {y}) is outside the {width}-wide image"),
        }
    }

    fn decode_png(png: &[u8]) -> (u32, u32, Vec<u8>) {
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().expect("valid PNG header");
        let mut buffer = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buffer).expect("valid PNG frame");
        assert_eq!(info.color_type, png::ColorType::Rgb);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        buffer.truncate(info.buffer_size());
        (info.width, info.height, buffer)
    }

    #[test]
    fn init_payloads_match_spice_proto() {
        assert_eq!(SPICE_MSGC_DISPLAY_INIT, 101);
        assert_eq!(SPICE_MSGC_DISPLAY_PREFERRED_COMPRESSION, 103);
        assert_eq!(
            display_init_payload(1),
            vec![1, 0x00, 0x00, 0xA0, 0x00, 0x00, 0x00, 0x00, 0x00, 0, 0, 0, 0, 0]
        );
        assert_eq!(display_init_payload(1).len(), 14);
        assert_eq!(preferred_compression_payload(1), vec![1]);
    }

    #[test]
    fn server_message_values_match_enums_h() {
        assert_eq!(SPICE_MSG_DISPLAY_DRAW_FILL, 302);
        assert_eq!(SPICE_MSG_DISPLAY_DRAW_OPAQUE, 303);
        assert_eq!(SPICE_MSG_DISPLAY_DRAW_COPY, 304);
        assert_eq!(SPICE_MSG_DISPLAY_DRAW_BLEND, 305);
        assert_eq!(SPICE_MSG_DISPLAY_DRAW_TRANSPARENT, 312);
        assert_eq!(SPICE_MSG_DISPLAY_DRAW_ALPHA_BLEND, 313);
        assert_eq!(SPICE_MSG_DISPLAY_SURFACE_CREATE, 314);
        assert_eq!(SPICE_MSG_DISPLAY_SURFACE_DESTROY, 315);
        assert_eq!(SPICE_MSG_DISPLAY_MONITORS_CONFIG, 317);
        assert_eq!(SPICE_MSG_DISPLAY_DRAW_COMPOSITE, 318);
        assert_eq!(SPICE_IMAGE_TYPE_FROM_CACHE, 103);
        assert_eq!(SPICE_IMAGE_TYPE_FROM_CACHE_LOSSLESS, 106);
    }

    #[test]
    fn surface_create_allocates_a_black_primary() {
        let mut capture = DisplayCapture::new();
        capture
            .handle_message(
                SPICE_MSG_DISPLAY_SURFACE_CREATE,
                &surface_create_body(0, 3, 2),
            )
            .expect("surface create");
        assert!(capture.png().is_none(), "nothing drawn yet");

        // A zero-sized surface is a protocol error, not a silent no-op.
        let err = capture
            .handle_message(
                SPICE_MSG_DISPLAY_SURFACE_CREATE,
                &surface_create_body(1, 0, 5),
            )
            .expect_err("zero height");
        assert!(err.contains("surface 1"), "{err}");
    }

    #[test]
    fn draw_fill_writes_the_box_and_png_reads_back() {
        let mut capture = DisplayCapture::new();
        capture
            .handle_message(
                SPICE_MSG_DISPLAY_SURFACE_CREATE,
                &surface_create_body(0, 4, 2),
            )
            .expect("surface create");

        let mut body = Vec::new();
        display_base(&mut body, 0, (0, 0, 2, 2));
        body.push(SPICE_BRUSH_TYPE_SOLID);
        u32b(0x00_ff_00_00, &mut body); // red
        u16b(SPICE_ROPD_OP_PUT, &mut body);
        qmask_none(&mut body);
        capture
            .handle_message(SPICE_MSG_DISPLAY_DRAW_FILL, &body)
            .expect("draw fill");

        let image = capture.png().expect("surface 0 was drawn");
        assert_eq!(image.width, 4);
        assert_eq!(image.height, 2);
        let (width, height, rgb) = decode_png(&image.png);
        assert_eq!((width, height), (4, 2));
        assert_eq!(pixel_at(&rgb, width, 0, 0), [255, 0, 0]);
        assert_eq!(pixel_at(&rgb, width, 1, 1), [255, 0, 0]);
        assert_eq!(pixel_at(&rgb, width, 2, 0), [0, 0, 0]);
        assert_eq!(pixel_at(&rgb, width, 3, 1), [0, 0, 0]);
    }

    #[test]
    fn draw_fill_clamps_to_the_surface() {
        let mut capture = DisplayCapture::new();
        capture
            .handle_message(
                SPICE_MSG_DISPLAY_SURFACE_CREATE,
                &surface_create_body(0, 2, 2),
            )
            .expect("surface create");

        let mut body = Vec::new();
        display_base(&mut body, 0, (1, 1, 9, 9)); // mostly outside
        body.push(SPICE_BRUSH_TYPE_SOLID);
        u32b(0x00_00_ff_00, &mut body); // green
        u16b(SPICE_ROPD_OP_PUT, &mut body);
        qmask_none(&mut body);
        capture
            .handle_message(SPICE_MSG_DISPLAY_DRAW_FILL, &body)
            .expect("draw fill");

        let image = capture.png().expect("drawn");
        let (width, _, rgb) = decode_png(&image.png);
        assert_eq!(pixel_at(&rgb, width, 1, 1), [0, 255, 0]);
        assert_eq!(pixel_at(&rgb, width, 0, 0), [0, 0, 0]);
    }

    #[test]
    fn bitmap_32bit_is_top_down_aware() {
        // Two rows: the memory order depends on SPICE_BITMAP_FLAGS_TOP_DOWN, and the
        // 32-bit word is little-endian xRGB (blue in the low byte).
        let mut pixels = Vec::new();
        pixels.extend_from_slice(&0x00_ff_00_00u32.to_le_bytes()); // red
        pixels.extend_from_slice(&0x00_00_ff_00u32.to_le_bytes()); // green
        pixels.extend_from_slice(&0x00_00_00_ffu32.to_le_bytes()); // blue
        pixels.extend_from_slice(&0x00_ff_ff_ffu32.to_le_bytes()); // white

        // Bottom-up (flags 0): the first row in memory is the bottom row.
        let mut body = vec![0u8; 4]; // pad so the image offset is not 0 (null)
        body.extend(bitmap_image_body(
            1,
            0,
            SPICE_BITMAP_FMT_32BIT,
            0,
            2,
            2,
            8,
            0,
            &pixels,
        ));
        let (_, image) = decode_image(&body, 4, "test").unwrap().expect("image");
        assert_eq!(image.width, 2);
        assert_eq!(image.pixel(0, 0), [0, 0, 255]); // blue row is on top
        assert_eq!(image.pixel(1, 0), [255, 255, 255]);
        assert_eq!(image.pixel(0, 1), [255, 0, 0]);
        assert_eq!(image.pixel(1, 1), [0, 255, 0]);

        // Top-down: rows arrive in image order.
        let mut body = vec![0u8; 4];
        body.extend(bitmap_image_body(
            2,
            0,
            SPICE_BITMAP_FMT_32BIT,
            SPICE_BITMAP_FLAGS_TOP_DOWN,
            2,
            2,
            8,
            0,
            &pixels,
        ));
        let (_, image) = decode_image(&body, 4, "test").unwrap().expect("image");
        assert_eq!(image.pixel(0, 0), [255, 0, 0]);
        assert_eq!(image.pixel(1, 1), [255, 255, 255]);
    }

    #[test]
    fn bitmap_rgba_ignores_alpha() {
        let pixels = [0x11, 0x22, 0x33, 0x00]; // B G R A, fully transparent
        let mut body = vec![0u8; 4];
        body.extend(bitmap_image_body(
            3,
            0,
            SPICE_BITMAP_FMT_RGBA,
            SPICE_BITMAP_FLAGS_TOP_DOWN,
            1,
            1,
            4,
            0,
            &pixels,
        ));
        let (_, image) = decode_image(&body, 4, "test").unwrap().expect("image");
        assert_eq!(image.pixel(0, 0), [0x33, 0x22, 0x11]);
    }

    #[test]
    fn bitmap_24bit_is_packed_bgr() {
        let pixels = [0x10, 0x20, 0x30]; // B G R
        let mut body = vec![0u8; 4];
        body.extend(bitmap_image_body(
            4,
            0,
            SPICE_BITMAP_FMT_24BIT,
            SPICE_BITMAP_FLAGS_TOP_DOWN,
            1,
            1,
            3,
            0,
            &pixels,
        ));
        let (_, image) = decode_image(&body, 4, "test").unwrap().expect("image");
        assert_eq!(image.pixel(0, 0), [0x30, 0x20, 0x10]);
    }

    #[test]
    fn bitmap_16bit_is_555_with_blue_low() {
        // 0b0_RRRRR_GGGGG_BBBBB, little-endian on the wire.
        let red = 0x7c00u16.to_le_bytes();
        let mut body = vec![0u8; 4];
        body.extend(bitmap_image_body(
            5,
            0,
            SPICE_BITMAP_FMT_16BIT,
            SPICE_BITMAP_FLAGS_TOP_DOWN,
            1,
            1,
            2,
            0,
            &red,
        ));
        let (_, image) = decode_image(&body, 4, "test").unwrap().expect("image");
        assert_eq!(image.pixel(0, 0), [255, 0, 0]);

        let blue = 0x001fu16.to_le_bytes();
        let mut body = vec![0u8; 4];
        body.extend(bitmap_image_body(
            6,
            0,
            SPICE_BITMAP_FMT_16BIT,
            SPICE_BITMAP_FLAGS_TOP_DOWN,
            1,
            1,
            2,
            0,
            &blue,
        ));
        let (_, image) = decode_image(&body, 4, "test").unwrap().expect("image");
        assert_eq!(image.pixel(0, 0), [0, 0, 255]);
    }

    #[test]
    fn bitmap_8bit_uses_the_palette_at_its_absolute_offset() {
        // Layout: 4 pad + 18 descriptor + 14 bitmap header + 4 palette pointer + 2
        // pixels = 42, so the palette table starts right after the pixels at 42.
        let mut body = vec![0u8; 4];
        body.extend(bitmap_image_body(
            7,
            0,
            SPICE_BITMAP_FMT_8BIT,
            SPICE_BITMAP_FLAGS_TOP_DOWN,
            2,
            1,
            2,
            42,
            &[0, 1],
        ));
        assert_eq!(body.len(), 42, "the palette table follows the pixel data");
        u64b(0xabcd, &mut body); // palette unique id
        u16b(2, &mut body); // num_ents
        u32b(0x00_11_22_33, &mut body);
        u32b(0x00_44_55_66, &mut body);

        let (_, decoded) = decode_image(&body, 4, "test").unwrap().expect("image");
        assert_eq!(decoded.pixel(0, 0), [0x11, 0x22, 0x33]);
        assert_eq!(decoded.pixel(1, 0), [0x44, 0x55, 0x66]);
    }

    #[test]
    fn bitmap_8bit_without_a_palette_is_an_error() {
        let mut body = vec![0u8; 4];
        body.extend(bitmap_image_body(
            8,
            0,
            SPICE_BITMAP_FMT_8BIT,
            SPICE_BITMAP_FLAGS_TOP_DOWN,
            1,
            1,
            1,
            0,
            &[7],
        ));
        let err = decode_image(&body, 4, "test").expect_err("no palette");
        assert!(err.contains("8BIT bitmap has no palette"), "{err}");
    }

    #[test]
    fn bitmap_1bit_defaults_to_black_and_white() {
        // Least-significant bit first.
        let mut body = vec![0u8; 4];
        body.extend(bitmap_image_body(
            9,
            0,
            SPICE_BITMAP_FMT_1BIT_LE,
            SPICE_BITMAP_FLAGS_TOP_DOWN,
            8,
            1,
            1,
            0,
            &[0b0000_0001],
        ));
        let (_, image) = decode_image(&body, 4, "test").unwrap().expect("image");
        assert_eq!(image.pixel(0, 0), [255, 255, 255]);
        assert_eq!(image.pixel(1, 0), [0, 0, 0]);

        // Most-significant bit first.
        let mut body = vec![0u8; 4];
        body.extend(bitmap_image_body(
            10,
            0,
            SPICE_BITMAP_FMT_1BIT_BE,
            SPICE_BITMAP_FLAGS_TOP_DOWN,
            8,
            1,
            1,
            0,
            &[0b1000_0000],
        ));
        let (_, image) = decode_image(&body, 4, "test").unwrap().expect("image");
        assert_eq!(image.pixel(0, 0), [255, 255, 255]);
        assert_eq!(image.pixel(7, 0), [0, 0, 0]);
    }

    #[test]
    fn bitmap_errors_are_actionable() {
        // 4BIT is not in the supported set.
        let mut body = vec![0u8; 4];
        body.extend(bitmap_image_body(11, 0, 3, 4, 2, 1, 1, 0, &[0x00]));
        let err = decode_image(&body, 4, "test").expect_err("4BIT unsupported");
        assert!(err.contains("unsupported bitmap format 3"), "{err}");

        // A stride that cannot hold one row.
        let mut body = vec![0u8; 4];
        body.extend(bitmap_image_body(
            12,
            0,
            SPICE_BITMAP_FMT_32BIT,
            4,
            4,
            1,
            8,
            0,
            &[0u8; 8],
        ));
        let err = decode_image(&body, 4, "test").expect_err("short stride");
        assert!(err.contains("stride 8"), "{err}");

        // Pixel data that is not in the message.
        let mut body = vec![0u8; 4];
        body.extend(bitmap_image_body(
            13,
            0,
            SPICE_BITMAP_FMT_32BIT,
            4,
            2,
            2,
            8,
            0,
            &[0u8; 4],
        ));
        let err = decode_image(&body, 4, "test").expect_err("short pixels");
        assert!(err.contains("bytes of pixels"), "{err}");

        // Unsupported image type (QUIC).
        let mut body = vec![0u8; 4];
        u64b(14, &mut body);
        body.push(1); // SPICE_IMAGE_TYPE_QUIC
        body.push(0);
        u32b(1, &mut body);
        u32b(1, &mut body);
        let err = decode_image(&body, 4, "test").expect_err("QUIC unsupported");
        assert_eq!(err, "unsupported image type 1");
    }

    #[test]
    fn draw_copy_blits_an_inline_bitmap_and_a_cached_one() {
        let mut capture = DisplayCapture::new();
        capture
            .handle_message(
                SPICE_MSG_DISPLAY_SURFACE_CREATE,
                &surface_create_body(0, 4, 4),
            )
            .expect("surface create");

        // 2x2 top-down 32-bit image: red, green / blue, white.
        let mut pixels = Vec::new();
        pixels.extend_from_slice(&0x00_ff_00_00u32.to_le_bytes());
        pixels.extend_from_slice(&0x00_00_ff_00u32.to_le_bytes());
        pixels.extend_from_slice(&0x00_00_00_ffu32.to_le_bytes());
        pixels.extend_from_slice(&0x00_ff_ff_ffu32.to_le_bytes());

        // SpiceCopy: base (21) + Image* (4) + src_area (16) + rop (2) + scale (1)
        // + QMask (13) = 57, so the image starts at 57.
        let mut body = Vec::new();
        display_base(&mut body, 0, (1, 1, 3, 3));
        u32b(57, &mut body);
        rect_bytes(0, 0, 2, 2, &mut body);
        u16b(SPICE_ROPD_OP_PUT, &mut body);
        body.push(0); // image_scale_mode
        qmask_none(&mut body);
        assert_eq!(body.len(), 57);
        body.extend(bitmap_image_body(
            42,
            SPICE_IMAGE_FLAGS_CACHE_ME,
            SPICE_BITMAP_FMT_32BIT,
            SPICE_BITMAP_FLAGS_TOP_DOWN,
            2,
            2,
            8,
            0,
            &pixels,
        ));
        capture
            .handle_message(SPICE_MSG_DISPLAY_DRAW_COPY, &body)
            .expect("draw copy");

        // The same image, referenced from the pixmap cache in the top-left corner.
        let mut body = Vec::new();
        display_base(&mut body, 0, (0, 0, 2, 2));
        u32b(57, &mut body);
        rect_bytes(0, 0, 2, 2, &mut body);
        u16b(SPICE_ROPD_OP_PUT, &mut body);
        body.push(0);
        qmask_none(&mut body);
        u64b(42, &mut body);
        body.push(SPICE_IMAGE_TYPE_FROM_CACHE);
        body.push(0);
        u32b(2, &mut body);
        u32b(2, &mut body);
        assert_eq!(body.len(), 57 + IMAGE_DESCRIPTOR_SIZE);
        capture
            .handle_message(SPICE_MSG_DISPLAY_DRAW_COPY, &body)
            .expect("draw from cache");

        let image = capture.png().expect("drawn");
        let (width, _, rgb) = decode_png(&image.png);
        // The cached draw lands in the top-left corner: red, green / blue, white.
        assert_eq!(pixel_at(&rgb, width, 0, 0), [255, 0, 0]);
        assert_eq!(pixel_at(&rgb, width, 1, 0), [0, 255, 0]);
        assert_eq!(pixel_at(&rgb, width, 0, 1), [0, 0, 255]);
        assert_eq!(pixel_at(&rgb, width, 1, 1), [255, 255, 255]);
        // The inline draw landed at (1,1)-(3,3), so its bottom-right is still white.
        assert_eq!(pixel_at(&rgb, width, 2, 2), [255, 255, 255]);
        // Untouched pixel.
        assert_eq!(pixel_at(&rgb, width, 3, 3), [0, 0, 0]);
    }

    #[test]
    fn cache_is_dropped_by_inval_all_pixmaps() {
        let mut capture = DisplayCapture::new();
        capture
            .handle_message(
                SPICE_MSG_DISPLAY_SURFACE_CREATE,
                &surface_create_body(0, 2, 2),
            )
            .expect("surface create");

        let pixels = vec![0u8; 4];
        let mut body = Vec::new();
        display_base(&mut body, 0, (0, 0, 1, 1));
        u32b(57, &mut body);
        rect_bytes(0, 0, 1, 1, &mut body);
        u16b(SPICE_ROPD_OP_PUT, &mut body);
        body.push(0);
        qmask_none(&mut body);
        body.extend(bitmap_image_body(
            7,
            SPICE_IMAGE_FLAGS_CACHE_ME,
            SPICE_BITMAP_FMT_32BIT,
            SPICE_BITMAP_FLAGS_TOP_DOWN,
            1,
            1,
            4,
            0,
            &pixels,
        ));
        capture
            .handle_message(SPICE_MSG_DISPLAY_DRAW_COPY, &body)
            .expect("draw copy");
        assert!(capture.cache.contains_key(&7));

        capture
            .handle_message(SPICE_MSG_DISPLAY_INVAL_ALL_PIXMAPS, &[])
            .expect("invalidate");
        assert!(capture.cache.is_empty());

        // A later reference to the dropped image is reported, not ignored.
        let mut body = Vec::new();
        display_base(&mut body, 0, (0, 0, 1, 1));
        u32b(57, &mut body);
        rect_bytes(0, 0, 1, 1, &mut body);
        u16b(SPICE_ROPD_OP_PUT, &mut body);
        body.push(0);
        qmask_none(&mut body);
        u64b(7, &mut body);
        body.push(SPICE_IMAGE_TYPE_FROM_CACHE);
        body.push(0);
        u32b(1, &mut body);
        u32b(1, &mut body);
        let err = capture
            .handle_message(SPICE_MSG_DISPLAY_DRAW_COPY, &body)
            .expect_err("cache miss");
        assert!(err.contains("image 7"), "{err}");
    }

    #[test]
    fn inval_list_drops_the_listed_ids() {
        let mut capture = DisplayCapture::new();
        capture.cache.insert(
            5,
            DecodedImage {
                width: 1,
                height: 1,
                rgb: vec![0, 0, 0],
            },
        );
        let mut body = Vec::new();
        u16b(1, &mut body); // count
        body.push(0); // resource type
        u64b(5, &mut body);
        capture
            .handle_message(SPICE_MSG_DISPLAY_INVAL_LIST, &body)
            .expect("invalidate list");
        assert!(capture.cache.is_empty());
    }

    #[test]
    fn copy_bits_moves_pixels_inside_the_surface() {
        let mut capture = DisplayCapture::new();
        capture
            .handle_message(
                SPICE_MSG_DISPLAY_SURFACE_CREATE,
                &surface_create_body(0, 2, 2),
            )
            .expect("surface create");

        // Left column red.
        let mut body = Vec::new();
        display_base(&mut body, 0, (0, 0, 2, 1));
        body.push(SPICE_BRUSH_TYPE_SOLID);
        u32b(0x00_ff_00_00, &mut body);
        u16b(SPICE_ROPD_OP_PUT, &mut body);
        qmask_none(&mut body);
        capture
            .handle_message(SPICE_MSG_DISPLAY_DRAW_FILL, &body)
            .expect("fill");

        // Copy it over the right column.
        let mut body = Vec::new();
        display_base(&mut body, 0, (0, 1, 2, 2));
        u32b(0, &mut body); // src_pos x
        u32b(0, &mut body); // src_pos y
        capture
            .handle_message(SPICE_MSG_DISPLAY_COPY_BITS, &body)
            .expect("copy bits");

        let image = capture.png().expect("drawn");
        let (width, _, rgb) = decode_png(&image.png);
        assert_eq!(pixel_at(&rgb, width, 0, 0), [255, 0, 0]);
        assert_eq!(pixel_at(&rgb, width, 1, 0), [255, 0, 0]);
        assert_eq!(pixel_at(&rgb, width, 1, 1), [255, 0, 0]);
    }

    #[test]
    fn surface_destroy_and_reset_drop_the_frame() {
        let mut capture = DisplayCapture::new();
        capture
            .handle_message(
                SPICE_MSG_DISPLAY_SURFACE_CREATE,
                &surface_create_body(0, 2, 2),
            )
            .expect("surface create");
        let mut body = Vec::new();
        display_base(&mut body, 0, (0, 0, 2, 2));
        body.push(SPICE_BRUSH_TYPE_SOLID);
        u32b(0x00_ff_ff_ff, &mut body);
        u16b(SPICE_ROPD_OP_PUT, &mut body);
        qmask_none(&mut body);
        capture
            .handle_message(SPICE_MSG_DISPLAY_DRAW_FILL, &body)
            .expect("fill");
        assert!(capture.png().is_some());

        capture
            .handle_message(SPICE_MSG_DISPLAY_RESET, &[])
            .expect("reset");
        assert!(capture.png().is_none(), "reset clears the frame");

        capture
            .handle_message(
                SPICE_MSG_DISPLAY_SURFACE_CREATE,
                &surface_create_body(0, 2, 2),
            )
            .expect("surface create");
        capture
            .handle_message(SPICE_MSG_DISPLAY_DRAW_FILL, &body)
            .expect("fill");
        assert!(capture.png().is_some());

        let mut destroy = Vec::new();
        u32b(0, &mut destroy);
        capture
            .handle_message(SPICE_MSG_DISPLAY_SURFACE_DESTROY, &destroy)
            .expect("surface destroy");
        assert!(capture.png().is_none());
    }

    #[test]
    fn ignored_messages_stay_quiet_and_unknown_ones_do_not() {
        let mut capture = DisplayCapture::new();
        for msg_type in [
            SPICE_MSG_DISPLAY_MARK,
            SPICE_MSG_DISPLAY_MODE,
            SPICE_MSG_DISPLAY_STREAM_CREATE,
            SPICE_MSG_DISPLAY_STREAM_DATA,
            SPICE_MSG_DISPLAY_DRAW_STROKE,
            SPICE_MSG_DISPLAY_DRAW_TEXT,
            SPICE_MSG_DISPLAY_DRAW_BLACKNESS,
            SPICE_MSG_DISPLAY_DRAW_ROP3,
            SPICE_MSG_DISPLAY_DRAW_COMPOSITE,
            SPICE_MSG_DISPLAY_MONITORS_CONFIG,
            SPICE_MSG_DISPLAY_INVAL_PALETTE,
        ] {
            capture
                .handle_message(msg_type, &[0u8; 64])
                .unwrap_or_else(|e| panic!("message {msg_type} should be ignored: {e}"));
        }

        let err = capture.handle_message(9999, &[]).expect_err("unknown type");
        assert_eq!(err, "unexpected display message type 9999");

        let err = capture
            .handle_message(SPICE_MSG_DISPLAY_SURFACE_CREATE, &[0, 0, 0])
            .expect_err("truncated");
        assert!(err.contains("truncated"), "{err}");
    }
}
