//! SPICE inputs channel: AT set-1 keyboard scancodes and keyboard/mouse payloads.
//!
//! Ported from the reference client in `docs/spice-html5`:
//! * `code_to_scancode.js` — the key name to scancode table (every entry ported);
//! * `utils.js` — `keycode_to_start_scan` / `keycode_to_end_scan` (break-bit encoding);
//! * `inputs.js` — `typeText` timing and the US-layout character table;
//! * `docs/spice.proto` / `docs/enums.h` — the exact client message bodies and values.
//!
//! The `SPICE_MSGC_INPUTS_*` and `SPICE_MOUSE_*` values repeat the numbers from
//! `docs/spice-protocol/enums.h` rather than importing them from
//! [`crate::spice::proto`], so this module does not depend on how `proto.rs` names
//! its constants. All integers are little-endian on the wire.
//!
//! As in `proto.rs`, the value tables below are deliberately complete: a value
//! that only a sibling module or the unit tests read is not dead code.
#![allow(dead_code)]

use crate::spice::Button;

/// One inputs-channel event: wait `delay_ms`, then send message `msg_type` with
/// `payload` as its body. The first event of a sequence carries delay 0, so the
/// events can be run in order without a leading sleep.
pub type InputEvent = (u16, u16, Vec<u8>);

// Client message types, `docs/enums.h` (InputsChannel in `docs/spice.proto`).
pub const SPICE_MSGC_INPUTS_KEY_DOWN: u16 = 101;
pub const SPICE_MSGC_INPUTS_KEY_UP: u16 = 102;
pub const SPICE_MSGC_INPUTS_MOUSE_MOTION: u16 = 111;
pub const SPICE_MSGC_INPUTS_MOUSE_POSITION: u16 = 112;
pub const SPICE_MSGC_INPUTS_MOUSE_PRESS: u16 = 113;
pub const SPICE_MSGC_INPUTS_MOUSE_RELEASE: u16 = 114;

// Mouse buttons and button masks, `docs/enums.h`. Wheel buttons are not part of
// `Button`, but `mouse_scroll` sends them through the press/release payloads.
pub const SPICE_MOUSE_BUTTON_LEFT: u8 = 1;
pub const SPICE_MOUSE_BUTTON_MIDDLE: u8 = 2;
pub const SPICE_MOUSE_BUTTON_RIGHT: u8 = 3;
pub const SPICE_MOUSE_BUTTON_UP: u8 = 4;
pub const SPICE_MOUSE_BUTTON_DOWN: u8 = 5;

pub const SPICE_MOUSE_BUTTON_MASK_LEFT: u16 = 1 << 0;
pub const SPICE_MOUSE_BUTTON_MASK_MIDDLE: u16 = 1 << 1;
pub const SPICE_MOUSE_BUTTON_MASK_RIGHT: u16 = 1 << 2;
pub const SPICE_MOUSE_BUTTON_MASK_UP: u16 = 1 << 3;
pub const SPICE_MOUSE_BUTTON_MASK_DOWN: u16 = 1 << 4;

/// AT set-1 make code of the left Shift key (used for shifted characters).
const SCAN_SHIFT_LEFT: u16 = 0x2A;
/// AT set-1 make code of the right Shift key (only to detect explicit use).
const SCAN_SHIFT_RIGHT: u16 = 0x36;

/// Lower bound for the gap between typed characters. A guest with a small
/// keyboard buffer silently drops keys that arrive faster than this (see the
/// `typeText` comments in `docs/spice-html5/inputs.js`), so a caller-supplied
/// interval below it is raised to it.
const MIN_TYPE_INTERVAL_MS: u16 = 10;
/// How long a key is held down, and how long Shift settles, in milliseconds.
/// Both values are `KEY_HOLD_MS` / `SHIFT_SETTLE_MS` from `inputs.js`.
const KEY_HOLD_MS: u16 = 12;
const SHIFT_SETTLE_MS: u16 = 12;
/// Gap between the events of a key combo ("10-20 ms between events", PLAN.md).
const COMBO_DELAY_MS: u16 = 20;

/// Key name (`KeyboardEvent.code`) to raw AT set-1 scancode, a verbatim port of
/// `docs/spice-html5/code_to_scancode.js`. Extended keys carry the PS/2 `0xE0`
/// prefix in the low byte, so `ArrowUp` is `0x48E0`.
///
/// `PrintScreen` and `Pause` are assigned twice in the reference file; lookup keeps
/// the last assignment, which is what a JavaScript object does (`0x37E0`, `0x46E0`).
const CODE_TO_SCANCODE: &[(&str, u16)] = &[
    ("Escape", 0x01),
    ("Digit1", 0x02),
    ("Digit2", 0x03),
    ("Digit3", 0x04),
    ("Digit4", 0x05),
    ("Digit5", 0x06),
    ("Digit6", 0x07),
    ("Digit7", 0x08),
    ("Digit8", 0x09),
    ("Digit9", 0x0A),
    ("Digit0", 0x0B),
    ("Minus", 0x0C),
    ("Equal", 0x0D),
    ("Backspace", 0x0E),
    ("Tab", 0x0F),
    ("KeyQ", 0x10),
    ("KeyW", 0x11),
    ("KeyE", 0x12),
    ("KeyR", 0x13),
    ("KeyT", 0x14),
    ("KeyY", 0x15),
    ("KeyU", 0x16),
    ("KeyI", 0x17),
    ("KeyO", 0x18),
    ("KeyP", 0x19),
    ("BracketLeft", 0x1A),
    ("BracketRight", 0x1B),
    ("Enter", 0x1C),
    ("ControlLeft", 0x1D),
    ("KeyA", 0x1E),
    ("KeyS", 0x1F),
    ("KeyD", 0x20),
    ("KeyF", 0x21),
    ("KeyG", 0x22),
    ("KeyH", 0x23),
    ("KeyJ", 0x24),
    ("KeyK", 0x25),
    ("KeyL", 0x26),
    ("Semicolon", 0x27),
    ("Quote", 0x28),
    ("Backquote", 0x29),
    ("ShiftLeft", 0x2A),
    ("Backslash", 0x2B),
    ("KeyZ", 0x2C),
    ("KeyX", 0x2D),
    ("KeyC", 0x2E),
    ("KeyV", 0x2F),
    ("KeyB", 0x30),
    ("KeyN", 0x31),
    ("KeyM", 0x32),
    ("Comma", 0x33),
    ("Period", 0x34),
    ("Slash", 0x35),
    ("ShiftRight", 0x36),
    ("NumpadMultiply", 0x37),
    ("AltLeft", 0x38),
    ("Space", 0x39),
    ("CapsLock", 0x3A),
    ("F1", 0x3B),
    ("F2", 0x3C),
    ("F3", 0x3D),
    ("F4", 0x3E),
    ("F5", 0x3F),
    ("F6", 0x40),
    ("F7", 0x41),
    ("F8", 0x42),
    ("F9", 0x43),
    ("F10", 0x44),
    ("Pause", 0x45),
    ("ScrollLock", 0x46),
    ("Numpad7", 0x47),
    ("Numpad8", 0x48),
    ("Numpad9", 0x49),
    ("NumpadSubtract", 0x4A),
    ("Numpad4", 0x4B),
    ("Numpad5", 0x4C),
    ("Numpad6", 0x4D),
    ("NumpadAdd", 0x4E),
    ("Numpad1", 0x4F),
    ("Numpad2", 0x50),
    ("Numpad3", 0x51),
    ("Numpad0", 0x52),
    ("NumpadDecimal", 0x53),
    ("PrintScreen", 0x54),
    ("IntlBackslash", 0x56),
    ("F11", 0x57),
    ("F12", 0x58),
    ("NumpadEqual", 0x59),
    ("F13", 0x64),
    ("F14", 0x65),
    ("F15", 0x66),
    ("F16", 0x67),
    ("F17", 0x68),
    ("F18", 0x69),
    ("F19", 0x6A),
    ("F20", 0x6B),
    ("F21", 0x6C),
    ("F22", 0x6D),
    ("F23", 0x6E),
    ("KanaMode", 0x70),
    ("IntlRo", 0x73),
    ("F24", 0x76),
    ("Convert", 0x79),
    ("NonConvert", 0x7B),
    ("IntlYen", 0x7D),
    ("NumpadComma", 0x7E),
    ("MediaTrackPrevious", 0x10E0),
    ("MediaTrackNext", 0x19E0),
    ("NumpadEnter", 0x1CE0),
    ("ControlRight", 0x1DE0),
    ("AudioVolumeMute", 0x20E0),
    ("LaunchApp2", 0x21E0),
    ("MediaPlayPause", 0x22E0),
    ("MediaStop", 0x24E0),
    ("VolumeDown", 0x2EE0),
    ("VolumeUp", 0x30E0),
    ("BrowserHome", 0x32E0),
    ("NumpadDivide", 0x35E0),
    ("PrintScreen", 0x37E0),
    ("AltRight", 0x38E0),
    ("NumLock", 0x45E0),
    ("Pause", 0x46E0),
    ("Home", 0x47E0),
    ("ArrowUp", 0x48E0),
    ("PageUp", 0x49E0),
    ("ArrowLeft", 0x4BE0),
    ("ArrowRight", 0x4DE0),
    ("End", 0x4FE0),
    ("ArrowDown", 0x50E0),
    ("PageDown", 0x51E0),
    ("Insert", 0x52E0),
    ("Delete", 0x53E0),
    ("MetaLeft", 0x5BE0),
    ("MetaRight", 0x5CE0),
    ("ContextMenu", 0x5DE0),
    ("Power", 0x5EE0),
    ("BrowserSearch", 0x65E0),
    ("BrowserFavorites", 0x66E0),
    ("BrowserRefresh", 0x67E0),
    ("BrowserStop", 0x68E0),
    ("BrowserForward", 0x69E0),
    ("BrowserBack", 0x6AE0),
    ("LaunchApp1", 0x6BE0),
    ("LaunchMail", 0x6CE0),
    ("MediaSelect", 0x6DE0),
];

/// Short names accepted in a combo (`"ctrl+alt+t"`), mapped to a `KeyboardEvent.code`
/// name from [`CODE_TO_SCANCODE`]. PLAN.md fixes the required spellings; the extra
/// left/right variants are the same keys under their obvious names.
const KEY_ALIASES: &[(&str, &str)] = &[
    ("ctrl", "ControlLeft"),
    ("control", "ControlLeft"),
    ("lctrl", "ControlLeft"),
    ("leftctrl", "ControlLeft"),
    ("rctrl", "ControlRight"),
    ("rightctrl", "ControlRight"),
    ("alt", "AltLeft"),
    ("lalt", "AltLeft"),
    ("leftalt", "AltLeft"),
    ("ralt", "AltRight"),
    ("rightalt", "AltRight"),
    ("altgr", "AltRight"),
    ("shift", "ShiftLeft"),
    ("lshift", "ShiftLeft"),
    ("leftshift", "ShiftLeft"),
    ("rshift", "ShiftRight"),
    ("rightshift", "ShiftRight"),
    ("meta", "MetaLeft"),
    ("lmeta", "MetaLeft"),
    ("leftmeta", "MetaLeft"),
    ("super", "MetaLeft"),
    ("win", "MetaLeft"),
    ("windows", "MetaLeft"),
    ("cmd", "MetaLeft"),
    ("rmeta", "MetaRight"),
    ("rightmeta", "MetaRight"),
    ("esc", "Escape"),
    ("escape", "Escape"),
    ("enter", "Enter"),
    ("return", "Enter"),
    ("numpadenter", "NumpadEnter"),
    ("kpenter", "NumpadEnter"),
    ("tab", "Tab"),
    ("space", "Space"),
    ("spacebar", "Space"),
    ("backspace", "Backspace"),
    ("bksp", "Backspace"),
    ("delete", "Delete"),
    ("del", "Delete"),
    ("insert", "Insert"),
    ("ins", "Insert"),
    ("home", "Home"),
    ("end", "End"),
    ("pgup", "PageUp"),
    ("pageup", "PageUp"),
    ("pgdn", "PageDown"),
    ("pagedown", "PageDown"),
    ("up", "ArrowUp"),
    ("down", "ArrowDown"),
    ("left", "ArrowLeft"),
    ("right", "ArrowRight"),
    ("capslock", "CapsLock"),
    ("numlock", "NumLock"),
    ("scrolllock", "ScrollLock"),
    ("printscreen", "PrintScreen"),
    ("prtsc", "PrintScreen"),
    ("sysrq", "PrintScreen"),
    ("pause", "Pause"),
    ("break", "Pause"),
    ("menu", "ContextMenu"),
    ("apps", "ContextMenu"),
    ("contextmenu", "ContextMenu"),
];

/// US-layout digits and their shifted symbols, in scancode order
/// (`Digit1`..`Digit0`), from the `US_TYPEABLE` table in `docs/spice-html5/inputs.js`.
const US_DIGITS: &[(char, char)] = &[
    ('1', '!'),
    ('2', '@'),
    ('3', '#'),
    ('4', '$'),
    ('5', '%'),
    ('6', '^'),
    ('7', '&'),
    ('8', '*'),
    ('9', '('),
    ('0', ')'),
];

/// US-layout punctuation: (unshifted, shifted, `KeyboardEvent.code`).
const US_PUNCTUATION: &[(char, char, &str)] = &[
    ('-', '_', "Minus"),
    ('=', '+', "Equal"),
    ('[', '{', "BracketLeft"),
    (']', '}', "BracketRight"),
    ('\\', '|', "Backslash"),
    (';', ':', "Semicolon"),
    ('\'', '"', "Quote"),
    (',', '<', "Comma"),
    ('.', '>', "Period"),
    ('/', '?', "Slash"),
    ('`', '~', "Backquote"),
];

/// A resolved keystroke: an AT set-1 make code, whether it needs the `0xE0`
/// prefix, and whether Shift has to be held for the intended character to appear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub make: u16,
    pub extended: bool,
    pub shift: bool,
}

/// Split a raw [`CODE_TO_SCANCODE`] value into (make code, extended flag).
fn split_scan(value: u16) -> (u16, bool) {
    if value < 0x100 {
        (value, false)
    } else {
        (value >> 8, true)
    }
}

/// Look up a `KeyboardEvent.code` name exactly as spelled in the reference table.
/// Later entries win, matching the duplicate assignments in `code_to_scancode.js`.
fn lookup_code_name(name: &str) -> Option<u16> {
    let mut found = None;
    for (code, value) in CODE_TO_SCANCODE {
        if *code == name {
            found = Some(*value);
        }
    }
    found
}

/// Case-insensitive variant of [`lookup_code_name`], so `"arrowup"` and `"ArrowUp"`
/// both work.
fn lookup_code_name_ci(name: &str) -> Option<u16> {
    let mut found = None;
    for (code, value) in CODE_TO_SCANCODE {
        if code.eq_ignore_ascii_case(name) {
            found = Some(*value);
        }
    }
    found
}

/// The scancode sent on a key-down for a make code: plain keys are one byte,
/// extended keys are `0xE0 | (make << 8)` (`utils.js::keycode_to_start_scan`).
pub fn key_down_code(make: u16, extended: bool) -> u32 {
    if extended {
        0xE0 | ((make as u32) << 8)
    } else {
        make as u32
    }
}

/// The scancode sent on a key-up: the make code with the break bit, which is
/// `| 0x80` for one-byte scancodes and `| 0x8000` for extended ones
/// (`utils.js::keycode_to_end_scan`).
pub fn key_up_code(make: u16, extended: bool) -> u32 {
    if extended {
        0x8000 | key_down_code(make, true)
    } else {
        (make | 0x80) as u32
    }
}

/// Body of `SPICE_MSGC_INPUTS_KEY_DOWN` / `KEY_UP`: the scancode as a `u32`.
pub fn key_payload(code: u32) -> Vec<u8> {
    code.to_le_bytes().to_vec()
}

/// Resolve a key name to (make code, extended flag). Accepts every
/// `KeyboardEvent.code` name from `code_to_scancode.js` (case-insensitively), the
/// short combo names listed in [`KEY_ALIASES`], `a`-`z`, `0`-`9`, `f1`-`f24`, and
/// US-layout symbol characters.
///
/// The return value cannot express Shift, so a shifted symbol such as `"+"` maps to
/// its physical key (`Equal`) without the Shift that would produce a `+`. Callers
/// that need the character rather than the key want [`char_to_scancode`].
pub fn key_name_to_scancode(name: &str) -> Option<(u16, bool)> {
    let trimmed = name.trim();
    if let Some(value) = lookup_code_name(trimmed).or_else(|| lookup_code_name_ci(trimmed)) {
        return Some(split_scan(value));
    }

    let lower = trimmed.to_ascii_lowercase();
    if let Some((_, code)) = KEY_ALIASES.iter().find(|(alias, _)| *alias == lower) {
        if let Some(value) = lookup_code_name(code) {
            return Some(split_scan(value));
        }
    }

    if lower.len() == 1 {
        let c = lower.chars().next()?;
        if c.is_ascii_lowercase() {
            let code = format!("Key{}", c.to_ascii_uppercase());
            return lookup_code_name(&code).map(split_scan);
        }
        if c.is_ascii_digit() {
            let code = format!("Digit{}", c);
            return lookup_code_name(&code).map(split_scan);
        }
        if let Ok(stroke) = char_to_scancode(c) {
            return Some((stroke.make, stroke.extended));
        }
    }

    if let Some(digits) = lower.strip_prefix('f') {
        if let Ok(n) = digits.parse::<u8>() {
            if (1..=24).contains(&n) {
                let code = format!("F{}", n);
                return lookup_code_name(&code).map(split_scan);
            }
        }
    }

    None
}

/// Resolve a printable character on a US layout to the keystroke that produces it,
/// including whether Shift is needed (the `US_TYPEABLE` table and `typeText` in
/// `docs/spice-html5/inputs.js`). Non-ASCII characters have no US-layout key and are
/// rejected with an error naming the character.
pub fn char_to_scancode(c: char) -> Result<KeyStroke, String> {
    let (name, shift) = if c.is_ascii_lowercase() {
        (format!("Key{}", c.to_ascii_uppercase()), false)
    } else if c.is_ascii_uppercase() {
        (format!("Key{}", c), true)
    } else if let Some((digit, _)) = US_DIGITS.iter().find(|(plain, _)| *plain == c) {
        (format!("Digit{}", digit), false)
    } else if let Some((digit, _)) = US_DIGITS.iter().find(|(_, shifted)| *shifted == c) {
        (format!("Digit{}", digit), true)
    } else if let Some((_, _, code)) = US_PUNCTUATION.iter().find(|(plain, _, _)| *plain == c) {
        ((*code).to_string(), false)
    } else if let Some((_, _, code)) = US_PUNCTUATION.iter().find(|(_, shifted, _)| *shifted == c) {
        ((*code).to_string(), true)
    } else {
        match c {
            ' ' => ("Space".to_string(), false),
            '\n' => ("Enter".to_string(), false),
            '\t' => ("Tab".to_string(), false),
            _ => {
                return Err(format!(
                    "cannot type {:?} (U+{:04X}): the SPICE inputs channel sends US-layout \
                     scancodes and no key produces this character",
                    c, c as u32
                ))
            }
        }
    };

    let value = lookup_code_name(&name)
        .ok_or_else(|| format!("scancode table has no entry for {}", name))?;
    let (make, extended) = split_scan(value);
    Ok(KeyStroke {
        make,
        extended,
        shift,
    })
}

/// Build the key-down / key-up / Shift events that type one character. `delay` is
/// the wait before the first of them (0 for the first character of a string).
fn push_stroke(events: &mut Vec<InputEvent>, stroke: KeyStroke, delay: u16) {
    if stroke.shift {
        events.push((
            delay,
            SPICE_MSGC_INPUTS_KEY_DOWN,
            key_payload(key_down_code(SCAN_SHIFT_LEFT, false)),
        ));
        events.push((
            SHIFT_SETTLE_MS,
            SPICE_MSGC_INPUTS_KEY_DOWN,
            key_payload(key_down_code(stroke.make, stroke.extended)),
        ));
        events.push((
            KEY_HOLD_MS,
            SPICE_MSGC_INPUTS_KEY_UP,
            key_payload(key_up_code(stroke.make, stroke.extended)),
        ));
        events.push((
            SHIFT_SETTLE_MS,
            SPICE_MSGC_INPUTS_KEY_UP,
            key_payload(key_up_code(SCAN_SHIFT_LEFT, false)),
        ));
    } else {
        events.push((
            delay,
            SPICE_MSGC_INPUTS_KEY_DOWN,
            key_payload(key_down_code(stroke.make, stroke.extended)),
        ));
        events.push((
            KEY_HOLD_MS,
            SPICE_MSGC_INPUTS_KEY_UP,
            key_payload(key_up_code(stroke.make, stroke.extended)),
        ));
    }
}

/// Event list that types `text` on a US layout, paced by `interval_ms` between
/// characters. Each character is a key-down followed by a key-up `KEY_HOLD_MS`
/// later, with Shift pressed and released around shifted characters. Carriage
/// returns are folded into a single Enter (`typeText` in `inputs.js`).
///
/// This is the PLAN.md signature and has no way to report a character it cannot
/// type: un-typeable text yields an empty list. Call [`try_type_text_events`]
/// (which `SpiceSession::type_text` should use) to get the error instead.
pub fn type_text_events(text: &str, interval_ms: u64) -> Vec<InputEvent> {
    match try_type_text_events(text, interval_ms) {
        Ok(events) => events,
        Err(_) => Vec::new(),
    }
}

/// [`type_text_events`] with the error channel: rejects characters the US layout
/// cannot produce (see [`char_to_scancode`]).
pub fn try_type_text_events(text: &str, interval_ms: u64) -> Result<Vec<InputEvent>, String> {
    // A CRLF must land as a single Enter, not Enter twice.
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    // Delays are u16 on the wire contract, and a very short gap makes guests drop
    // characters, so the interval is floored and capped.
    let interval = interval_ms.clamp(MIN_TYPE_INTERVAL_MS as u64, u16::MAX as u64) as u16;

    let mut events = Vec::new();
    let mut delay = 0u16;
    for c in text.chars() {
        let stroke = char_to_scancode(c)?;
        push_stroke(&mut events, stroke, delay);
        delay = interval;
    }
    Ok(events)
}

/// Event list for a key combination such as `"ctrl+alt+t"`: all keys are pressed
/// left to right, then released in reverse, `COMBO_DELAY_MS` apart. A symbol that
/// needs Shift gets a synthetic left Shift unless the combo already names one.
pub fn key_combo_events(combo: &str) -> Result<Vec<InputEvent>, String> {
    let mut strokes: Vec<KeyStroke> = Vec::new();
    for token in combo.split('+') {
        let token = token.trim();
        if token.is_empty() {
            return Err(format!(
                "empty key name in combo {:?} (expected e.g. \"ctrl+alt+t\")",
                combo
            ));
        }
        let mut chars = token.chars();
        let single = match (chars.next(), chars.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        };
        let stroke = if let Some(c) = single {
            char_to_scancode(c)
                .map_err(|e| format!("bad key {} in combo {:?}: {}", token, combo, e))?
        } else {
            match key_name_to_scancode(token) {
                Some((make, extended)) => KeyStroke {
                    make,
                    extended,
                    shift: false,
                },
                None => {
                    return Err(format!(
                        "unknown key name {:?} in combo {:?} (try e.g. \"ctrl+alt+t\", \
                         \"ctrl+c\", \"f5\", \"up\", \"shift+tab\")",
                        token, combo
                    ))
                }
            }
        };
        strokes.push(stroke);
    }
    if strokes.is_empty() {
        return Err("key combo is empty".to_string());
    }

    let names_shift = strokes
        .iter()
        .any(|s| !s.extended && (s.make == SCAN_SHIFT_LEFT || s.make == SCAN_SHIFT_RIGHT));
    if !names_shift && strokes.iter().any(|s| s.shift) {
        strokes.insert(
            0,
            KeyStroke {
                make: SCAN_SHIFT_LEFT,
                extended: false,
                shift: false,
            },
        );
    }

    let mut events = Vec::new();
    let mut delay = 0u16;
    for stroke in &strokes {
        events.push((
            delay,
            SPICE_MSGC_INPUTS_KEY_DOWN,
            key_payload(key_down_code(stroke.make, stroke.extended)),
        ));
        delay = COMBO_DELAY_MS;
    }
    for stroke in strokes.iter().rev() {
        events.push((
            delay,
            SPICE_MSGC_INPUTS_KEY_UP,
            key_payload(key_up_code(stroke.make, stroke.extended)),
        ));
        delay = COMBO_DELAY_MS;
    }
    Ok(events)
}

/// Body of `SPICE_MSGC_INPUTS_MOUSE_POSITION`: `x u32, y u32, buttons_state u16,
/// display_id u8`. `display_id` is 0: we only ever drive the primary display.
pub fn mouse_position_payload(x: u32, y: u32, buttons: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(11);
    payload.extend_from_slice(&x.to_le_bytes());
    payload.extend_from_slice(&y.to_le_bytes());
    payload.extend_from_slice(&buttons.to_le_bytes());
    payload.push(0);
    payload
}

/// Body of `SPICE_MSGC_INPUTS_MOUSE_MOTION`: `dx i32, dy i32, buttons_state u16`.
pub fn mouse_motion_payload(dx: i32, dy: i32, buttons: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(10);
    payload.extend_from_slice(&dx.to_le_bytes());
    payload.extend_from_slice(&dy.to_le_bytes());
    payload.extend_from_slice(&buttons.to_le_bytes());
    payload
}

/// The `mouse_button` value for a [`Button`].
pub fn button_code(button: Button) -> u8 {
    match button {
        Button::Left => SPICE_MOUSE_BUTTON_LEFT,
        Button::Middle => SPICE_MOUSE_BUTTON_MIDDLE,
        Button::Right => SPICE_MOUSE_BUTTON_RIGHT,
    }
}

/// The `mouse_button_mask` bit for a [`Button`].
pub fn button_mask(button: Button) -> u16 {
    button_mask_for_code(button_code(button))
}

/// The `mouse_button_mask` bit for a raw button number (1 = left … 5 = wheel down).
/// A wheel button has no [`Button`] variant, so `mouse_scroll` needs this path.
pub fn button_mask_for_code(button: u8) -> u16 {
    if button >= 1 && button <= 16 {
        1u16 << (button - 1)
    } else {
        0
    }
}

/// Body of `SPICE_MSGC_INPUTS_MOUSE_PRESS`: `button u8, buttons_state u16`.
pub fn mouse_press_payload(button: Button, buttons: u16) -> Vec<u8> {
    mouse_press_code_payload(button_code(button), buttons)
}

/// Body of `SPICE_MSGC_INPUTS_MOUSE_RELEASE`: `button u8, buttons_state u16`.
pub fn mouse_release_payload(button: Button, buttons: u16) -> Vec<u8> {
    mouse_release_code_payload(button_code(button), buttons)
}

/// `SPICE_MSGC_INPUTS_MOUSE_PRESS` for a raw button number, used for the wheel
/// buttons ([`SPICE_MOUSE_BUTTON_UP`] / [`SPICE_MOUSE_BUTTON_DOWN`]).
pub fn mouse_press_code_payload(button: u8, buttons: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(3);
    payload.push(button);
    payload.extend_from_slice(&buttons.to_le_bytes());
    payload
}

/// `SPICE_MSGC_INPUTS_MOUSE_RELEASE` for a raw button number.
pub fn mouse_release_code_payload(button: u8, buttons: u16) -> Vec<u8> {
    mouse_press_code_payload(button, buttons)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn code_of(event: &InputEvent) -> u32 {
        assert_eq!(event.2.len(), 4, "key payloads are a u32");
        u32::from_le_bytes([event.2[0], event.2[1], event.2[2], event.2[3]])
    }

    #[test]
    fn scancode_spot_values_match_reference() {
        assert_eq!(key_name_to_scancode("a"), Some((0x1E, false)));
        assert_eq!(key_name_to_scancode("A"), Some((0x1E, false)));
        assert_eq!(key_name_to_scancode("KeyA"), Some((0x1E, false)));
        assert_eq!(key_name_to_scancode("ArrowUp"), Some((0x48, true)));
        assert_eq!(key_name_to_scancode("arrowup"), Some((0x48, true)));
        assert_eq!(key_name_to_scancode("up"), Some((0x48, true)));
        assert_eq!(key_name_to_scancode("NumpadEnter"), Some((0x1C, true)));
        assert_eq!(key_name_to_scancode("F12"), Some((0x58, false)));
        assert_eq!(key_name_to_scancode("f12"), Some((0x58, false)));
        assert_eq!(key_name_to_scancode("0"), Some((0x0B, false)));
        assert_eq!(key_name_to_scancode("PrintScreen"), Some((0x37, true)));
        assert_eq!(key_name_to_scancode("Pause"), Some((0x46, true)));
        assert_eq!(key_name_to_scancode("no-such-key"), None);
    }

    #[test]
    fn key_up_sets_the_break_bit() {
        // "a" -> 0x1E make, 0x9E break.
        assert_eq!(key_down_code(0x1E, false), 0x0000_001E);
        assert_eq!(key_up_code(0x1E, false), 0x0000_009E);
        // ArrowUp -> 0x48E0 make, 0xC8E0 break (utils.js::keycode_to_end_scan).
        assert_eq!(key_down_code(0x48, true), 0x0000_48E0);
        assert_eq!(key_up_code(0x48, true), 0x0000_C8E0);
        assert_eq!(key_down_code(0x1C, true), 0x0000_1CE0);
        assert_eq!(key_up_code(0x1C, true), 0x0000_9CE0);
    }

    /// Every entry of the reference table must be present with the same value, and
    /// no extra names may be invented.
    #[test]
    fn scancode_table_matches_reference_js() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/docs/spice-html5/code_to_scancode.js"
        );
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            // The reference checkout is not part of every build; the spot values
            // above still pin the important entries.
            Err(_) => return,
        };

        let mut reference: HashMap<String, u32> = HashMap::new();
        for line in source.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("code_to_scancode[") else {
                continue;
            };
            let Some(close) = rest.find(']') else {
                continue;
            };
            let name = rest[..close].trim().trim_matches('"').to_string();
            let expr = rest[close + 1..]
                .trim()
                .trim_start_matches('=')
                .trim()
                .trim_end_matches(';')
                .trim();
            let value = parse_js_scancode(expr)
                .unwrap_or_else(|| panic!("cannot parse reference value {expr}"));
            reference.insert(name, value);
        }
        assert!(reference.len() > 120, "reference table looks incomplete");

        for (name, value) in &reference {
            let (make, extended) = key_name_to_scancode(name)
                .unwrap_or_else(|| panic!("missing scancode table entry for {name}"));
            let raw = key_down_code(make, extended) as u16;
            assert_eq!(raw, *value as u16, "scancode mismatch for {name}");
        }

        let mut ours: Vec<&str> = CODE_TO_SCANCODE.iter().map(|(name, _)| *name).collect();
        ours.sort_unstable();
        ours.dedup();
        assert_eq!(
            ours.len(),
            reference.len(),
            "table has extra or missing names"
        );
    }

    fn parse_js_scancode(expr: &str) -> Option<u32> {
        let (base, shift) = match expr.split_once('|') {
            Some((base, shift)) => (base.trim(), Some(shift.trim())),
            None => (expr.trim(), None),
        };
        let base = u32::from_str_radix(base.trim_start_matches("0x"), 16).ok()?;
        match shift {
            None => Some(base),
            Some(shift) => {
                let inner = shift.trim_start_matches('(').trim_end_matches(')');
                let (value, amount) = inner.split_once("<<")?;
                let value = u32::from_str_radix(value.trim().trim_start_matches("0x"), 16).ok()?;
                let amount: u32 = amount.trim().parse().ok()?;
                Some(base | (value << amount))
            }
        }
    }

    #[test]
    fn types_hello_world() {
        let events = try_type_text_events("Hello World!", 25).expect("typeable");
        // H, W and ! are shifted (4 events each); the other 9 characters are 2 each.
        assert_eq!(events.len(), 3 * 4 + 9 * 2);

        // First event: Shift down for 'H' at once.
        assert_eq!(events[0].0, 0);
        assert_eq!(events[0].1, SPICE_MSGC_INPUTS_KEY_DOWN);
        assert_eq!(code_of(&events[0]), key_down_code(SCAN_SHIFT_LEFT, false));
        assert_eq!(code_of(&events[1]), key_down_code(0x23, false)); // KeyH
        assert_eq!(events[1].0, SHIFT_SETTLE_MS);
        assert_eq!(events[1].1, SPICE_MSGC_INPUTS_KEY_DOWN);
        assert_eq!(events[2].1, SPICE_MSGC_INPUTS_KEY_UP);
        assert_eq!(code_of(&events[2]), key_up_code(0x23, false));
        assert_eq!(events[3].1, SPICE_MSGC_INPUTS_KEY_UP);
        assert_eq!(code_of(&events[3]), key_up_code(SCAN_SHIFT_LEFT, false));

        // 'e' is plain and starts after the caller's interval.
        assert_eq!(events[4].0, 25);
        assert_eq!(events[4].1, SPICE_MSGC_INPUTS_KEY_DOWN);
        assert_eq!(code_of(&events[4]), key_down_code(0x12, false)); // KeyE
        assert_eq!(events[5].0, KEY_HOLD_MS);
        assert_eq!(code_of(&events[5]), key_up_code(0x12, false));

        // Last character '!': Shift + Digit1.
        let tail = &events[events.len() - 4..];
        assert_eq!(tail[0].0, 25);
        assert_eq!(code_of(&tail[0]), key_down_code(SCAN_SHIFT_LEFT, false));
        assert_eq!(code_of(&tail[1]), key_down_code(0x02, false)); // Digit1
        assert_eq!(code_of(&tail[2]), key_up_code(0x02, false));
        assert_eq!(code_of(&tail[3]), key_up_code(SCAN_SHIFT_LEFT, false));

        // No event is ever an empty payload.
        assert!(events.iter().all(|e| !e.2.is_empty()));
    }

    #[test]
    fn interval_is_floored_for_the_guest() {
        let events = try_type_text_events("ab", 0).expect("typeable");
        assert_eq!(events[0].0, 0);
        // The second character still waits the floor, not zero.
        assert_eq!(events[2].0, MIN_TYPE_INTERVAL_MS);
    }

    #[test]
    fn carriage_returns_become_enter() {
        // The CRLF collapses to one newline: "a\nb\nc", five characters.
        let events = try_type_text_events("a\r\nb\rc", 20).expect("typeable");
        assert_eq!(events.len(), 5 * 2);
        // Enter is 0x1C make / 0x9C break, twice.
        assert_eq!(code_of(&events[2]), key_down_code(0x1C, false));
        assert_eq!(code_of(&events[3]), key_up_code(0x1C, false));
        assert_eq!(code_of(&events[6]), key_down_code(0x1C, false));
        assert_eq!(code_of(&events[7]), key_up_code(0x1C, false));
        assert_eq!(events[3].1, SPICE_MSGC_INPUTS_KEY_UP);
        assert_eq!(code_of(&events[4]), key_down_code(0x30, false)); // KeyB
    }

    #[test]
    fn non_ascii_is_rejected() {
        let err = try_type_text_events("caf\u{e9}", 20).expect_err("must reject");
        assert!(
            err.contains("U+00E9"),
            "error should name the character: {err}"
        );
        assert!(type_text_events("caf\u{e9}", 20).is_empty());
        assert!(char_to_scancode('\u{4e2d}').is_err());
    }

    #[test]
    fn us_layout_chars() {
        assert_eq!(
            char_to_scancode('a').unwrap(),
            KeyStroke {
                make: 0x1E,
                extended: false,
                shift: false
            }
        );
        assert_eq!(
            char_to_scancode('A').unwrap(),
            KeyStroke {
                make: 0x1E,
                extended: false,
                shift: true
            }
        );
        assert_eq!(
            char_to_scancode('7').unwrap(),
            KeyStroke {
                make: 0x08,
                extended: false,
                shift: false
            }
        );
        assert_eq!(
            char_to_scancode('&').unwrap(),
            KeyStroke {
                make: 0x08,
                extended: false,
                shift: true
            }
        );
        assert_eq!(
            char_to_scancode('?').unwrap(),
            KeyStroke {
                make: 0x35,
                extended: false,
                shift: true
            }
        );
        assert_eq!(
            char_to_scancode(' ').unwrap(),
            KeyStroke {
                make: 0x39,
                extended: false,
                shift: false
            }
        );
        assert_eq!(
            char_to_scancode('\n').unwrap(),
            KeyStroke {
                make: 0x1C,
                extended: false,
                shift: false
            }
        );
    }

    #[test]
    fn combo_ctrl_alt_t() {
        let events = key_combo_events("ctrl+alt+t").expect("valid combo");
        assert_eq!(events.len(), 6);
        let codes: Vec<u32> = events.iter().map(code_of).collect();
        assert_eq!(
            codes,
            vec![
                key_down_code(0x1D, false), // ControlLeft
                key_down_code(0x38, false), // AltLeft
                key_down_code(0x14, false), // KeyT
                key_up_code(0x14, false),
                key_up_code(0x38, false),
                key_up_code(0x1D, false),
            ]
        );
        assert_eq!(events[0].1, SPICE_MSGC_INPUTS_KEY_DOWN);
        assert_eq!(events[5].1, SPICE_MSGC_INPUTS_KEY_UP);
        assert_eq!(
            events.iter().map(|e| e.0).collect::<Vec<u16>>(),
            vec![
                0,
                COMBO_DELAY_MS,
                COMBO_DELAY_MS,
                COMBO_DELAY_MS,
                COMBO_DELAY_MS,
                COMBO_DELAY_MS
            ]
        );
    }

    #[test]
    fn combo_accepts_codes_and_symbols() {
        let events = key_combo_events("ArrowUp").expect("valid combo");
        assert_eq!(events.len(), 2);
        assert_eq!(code_of(&events[0]), 0x48E0);
        assert_eq!(code_of(&events[1]), 0xC8E0);

        // '!' needs Shift on a US layout, so it is added for the caller.
        let events = key_combo_events("!").expect("valid combo");
        assert_eq!(events.len(), 4);
        assert_eq!(code_of(&events[0]), key_down_code(SCAN_SHIFT_LEFT, false));
        assert_eq!(code_of(&events[1]), key_down_code(0x02, false));
        assert_eq!(code_of(&events[2]), key_up_code(0x02, false));
        assert_eq!(code_of(&events[3]), key_up_code(SCAN_SHIFT_LEFT, false));

        // An explicit shift is not duplicated.
        let events = key_combo_events("shift+!").expect("valid combo");
        assert_eq!(events.len(), 4);
    }

    #[test]
    fn combo_errors_are_actionable() {
        let err = key_combo_events("ctrl+").expect_err("trailing plus");
        assert!(err.contains("empty key name"), "{err}");
        let err = key_combo_events("ctrl+nope").expect_err("unknown key");
        assert!(
            err.contains("unknown key name") && err.contains("nope"),
            "{err}"
        );
        let err = key_combo_events("").expect_err("empty combo");
        assert!(err.contains("empty key name"), "{err}");
    }

    #[test]
    fn mouse_payloads_are_little_endian() {
        assert_eq!(
            mouse_position_payload(0x0102_0304, 0x0506_0708, 0x090A),
            vec![0x04, 0x03, 0x02, 0x01, 0x08, 0x07, 0x06, 0x05, 0x0A, 0x09, 0x00]
        );
        assert_eq!(mouse_position_payload(10, 20, 0).len(), 11);

        // Negative deltas stay two's complement on the wire.
        assert_eq!(
            mouse_motion_payload(-1, -2, 1),
            vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFE, 0xFF, 0xFF, 0xFF, 0x01, 0x00]
        );
        assert_eq!(mouse_motion_payload(10, 20, 0).len(), 10);

        assert_eq!(
            mouse_press_payload(Button::Left, SPICE_MOUSE_BUTTON_MASK_LEFT),
            vec![1, 1, 0]
        );
        assert_eq!(
            mouse_release_payload(Button::Right, 0),
            vec![SPICE_MOUSE_BUTTON_RIGHT, 0, 0]
        );
        assert_eq!(
            mouse_press_code_payload(SPICE_MOUSE_BUTTON_UP, 0),
            vec![4, 0, 0]
        );
        assert_eq!(
            mouse_release_code_payload(SPICE_MOUSE_BUTTON_DOWN, 0),
            vec![5, 0, 0]
        );
    }

    #[test]
    fn button_mapping() {
        assert_eq!(button_code(Button::Left), SPICE_MOUSE_BUTTON_LEFT);
        assert_eq!(button_code(Button::Middle), SPICE_MOUSE_BUTTON_MIDDLE);
        assert_eq!(button_code(Button::Right), SPICE_MOUSE_BUTTON_RIGHT);
        assert_eq!(button_mask(Button::Left), SPICE_MOUSE_BUTTON_MASK_LEFT);
        assert_eq!(button_mask(Button::Middle), SPICE_MOUSE_BUTTON_MASK_MIDDLE);
        assert_eq!(button_mask(Button::Right), SPICE_MOUSE_BUTTON_MASK_RIGHT);
        assert_eq!(
            button_mask_for_code(SPICE_MOUSE_BUTTON_UP),
            SPICE_MOUSE_BUTTON_MASK_UP
        );
        assert_eq!(
            button_mask_for_code(SPICE_MOUSE_BUTTON_DOWN),
            SPICE_MOUSE_BUTTON_MASK_DOWN
        );
        assert_eq!(button_mask_for_code(0), 0);
    }

    #[test]
    fn message_type_values_match_enums_h() {
        assert_eq!(SPICE_MSGC_INPUTS_KEY_DOWN, 101);
        assert_eq!(SPICE_MSGC_INPUTS_KEY_UP, 102);
        assert_eq!(SPICE_MSGC_INPUTS_MOUSE_MOTION, 111);
        assert_eq!(SPICE_MSGC_INPUTS_MOUSE_POSITION, 112);
        assert_eq!(SPICE_MSGC_INPUTS_MOUSE_PRESS, 113);
        assert_eq!(SPICE_MSGC_INPUTS_MOUSE_RELEASE, 114);
    }
}
