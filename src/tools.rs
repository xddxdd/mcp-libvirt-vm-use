//! MCP tool definitions and dispatch.
//!
//! Every tool that touches a guest resolves its domain through `libvirt`, opens
//! a fresh `SpiceSession`, performs the operation and drops the connection —
//! connections are never pooled.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::libvirt::{DomainInfo, Libvirt};
use crate::spice::{Button, ScrollDir, SpiceSession};

const DEFAULT_SCREENSHOT_WAIT_MS: u32 = 500;
const DEFAULT_TYPE_INTERVAL_MS: u64 = 20;
const DEFAULT_SCROLL_CLICKS: u32 = 1;

/// Result of a `tools/call`, ready to be embedded in the JSON-RPC result.
pub struct ToolOutput {
    pub content: Vec<Value>,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn error(message: impl Into<String>) -> Self {
        ToolOutput {
            content: vec![text_content(message.into())],
            is_error: true,
        }
    }
}

/// JSON-Schema definitions of all eight tools, in `tools/list` order.
pub fn tool_definitions() -> Vec<Value> {
    let domain = json!({
        "type": "string",
        "description": "Domain name as listed by list_domains, e.g. \"Windows10\".",
    });
    vec![
        json!({
            "name": "list_domains",
            "description": "List all libvirt domains with their id, state and SPICE display endpoint.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": [],
            },
        }),
        json!({
            "name": "screenshot",
            "description": "Capture the SPICE display of a domain and return it as a PNG image.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "domain": domain,
                    "wait_ms": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Quiet period in milliseconds to wait for display updates before capturing (default 500).",
                    },
                },
                "required": ["domain"],
            },
        }),
        json!({
            "name": "type_text",
            "description": "Type ASCII text into a domain through the SPICE inputs channel.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "domain": domain,
                    "text": {
                        "type": "string",
                        "description": "ASCII text to type (US keyboard layout).",
                    },
                    "interval_ms": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Delay between keystrokes in milliseconds (default 20).",
                    },
                },
                "required": ["domain", "text"],
            },
        }),
        json!({
            "name": "key_press",
            "description": "Send a key combination such as \"ctrl+alt+t\" or a single key such as \"enter\" to a domain.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "domain": domain,
                    "keys": {
                        "type": "string",
                        "description": "Key names joined with '+', e.g. \"ctrl+alt+t\". Names are case-insensitive.",
                    },
                },
                "required": ["domain", "keys"],
            },
        }),
        json!({
            "name": "mouse_move",
            "description": "Move the mouse pointer to an absolute position inside a domain's display.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "domain": domain,
                    "x": { "type": "integer", "minimum": 0, "description": "X coordinate in guest pixels." },
                    "y": { "type": "integer", "minimum": 0, "description": "Y coordinate in guest pixels." },
                },
                "required": ["domain", "x", "y"],
            },
        }),
        json!({
            "name": "mouse_click",
            "description": "Click a mouse button in a domain, optionally moving to a position first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "domain": domain,
                    "button": {
                        "type": "string",
                        "enum": ["left", "middle", "right"],
                        "description": "Mouse button to click.",
                    },
                    "x": { "type": "integer", "minimum": 0, "description": "Optional X coordinate to move to before clicking." },
                    "y": { "type": "integer", "minimum": 0, "description": "Optional Y coordinate to move to before clicking." },
                    "double_click": {
                        "type": "boolean",
                        "description": "Click twice in quick succession (default false).",
                    },
                },
                "required": ["domain", "button"],
            },
        }),
        json!({
            "name": "mouse_scroll",
            "description": "Scroll the mouse wheel inside a domain's display.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "domain": domain,
                    "direction": {
                        "type": "string",
                        "enum": ["up", "down"],
                        "description": "Scroll direction.",
                    },
                    "clicks": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Number of wheel clicks (default 1).",
                    },
                },
                "required": ["domain", "direction"],
            },
        }),
        json!({
            "name": "mouse_drag",
            "description": "Press the left mouse button at one position, drag to another position and release.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "domain": domain,
                    "from_x": { "type": "integer", "minimum": 0, "description": "Start X coordinate." },
                    "from_y": { "type": "integer", "minimum": 0, "description": "Start Y coordinate." },
                    "to_x": { "type": "integer", "minimum": 0, "description": "End X coordinate." },
                    "to_y": { "type": "integer", "minimum": 0, "description": "End Y coordinate." },
                },
                "required": ["domain", "from_x", "from_y", "to_x", "to_y"],
            },
        }),
    ]
}

/// Run a tool call. Tool failures are normal results carrying `isError`, not
/// JSON-RPC errors.
pub fn call_tool(libvirt: &Libvirt, name: &str, arguments: Option<&Value>) -> ToolOutput {
    if name.is_empty() {
        return ToolOutput::error("tools/call: 'name' must be a non-empty string");
    }
    let args = match arguments {
        Some(value @ Value::Object(_)) => value.clone(),
        _ => json!({}),
    };
    match run_tool(libvirt, name, &args) {
        Ok(content) => ToolOutput {
            content,
            is_error: false,
        },
        Err(message) => ToolOutput::error(message),
    }
}

fn run_tool(libvirt: &Libvirt, name: &str, args: &Value) -> Result<Vec<Value>, String> {
    match name {
        "list_domains" => {
            let domains = libvirt.list_domains()?;
            Ok(vec![text_content(format_domain_table(&domains))])
        }
        "screenshot" => {
            let domain = arg_string(args, "domain")?;
            let wait_ms = arg_u32(args, "wait_ms", DEFAULT_SCREENSHOT_WAIT_MS)?;
            let mut session = connect_spice(libvirt, &domain)?;
            let image = session.screenshot(wait_ms)?;
            Ok(vec![
                image_content(BASE64.encode(&image.png)),
                text_content(format!(
                    "screenshot of '{}': {}x{} PNG ({} bytes)",
                    domain,
                    image.width,
                    image.height,
                    image.png.len()
                )),
            ])
        }
        "type_text" => {
            let domain = arg_string(args, "domain")?;
            let text = arg_string(args, "text")?;
            let interval_ms = arg_u64(args, "interval_ms", DEFAULT_TYPE_INTERVAL_MS)?;
            let mut session = connect_spice(libvirt, &domain)?;
            session.type_text(&text, interval_ms)?;
            Ok(vec![text_content(format!(
                "typed {} character(s) into '{}'",
                text.chars().count(),
                domain
            ))])
        }
        "key_press" => {
            let domain = arg_string(args, "domain")?;
            let keys = arg_string(args, "keys")?;
            let mut session = connect_spice(libvirt, &domain)?;
            session.key_press(&keys)?;
            Ok(vec![text_content(format!(
                "sent key combination '{}' to '{}'",
                keys, domain
            ))])
        }
        "mouse_move" => {
            let domain = arg_string(args, "domain")?;
            let x = arg_required_u32(args, "x")?;
            let y = arg_required_u32(args, "y")?;
            let mut session = connect_spice(libvirt, &domain)?;
            session.mouse_move(x, y)?;
            Ok(vec![text_content(format!(
                "moved mouse to ({}, {}) on '{}'",
                x, y, domain
            ))])
        }
        "mouse_click" => {
            let domain = arg_string(args, "domain")?;
            let button = parse_button(&arg_string(args, "button")?)?;
            let x = arg_optional_u32(args, "x")?;
            let y = arg_optional_u32(args, "y")?;
            if x.is_some() != y.is_some() {
                return Err(
                    "arguments 'x' and 'y' must be provided together for mouse_click".to_string(),
                );
            }
            let double = arg_bool(args, "double_click", false)?;
            let mut session = connect_spice(libvirt, &domain)?;
            session.mouse_click(button, x, y, double)?;
            let action = if double { "double-clicked" } else { "clicked" };
            let position = match (x, y) {
                (Some(x), Some(y)) => format!(" at ({}, {})", x, y),
                _ => String::new(),
            };
            Ok(vec![text_content(format!(
                "{} {} button{} on '{}'",
                action,
                button_label(&button),
                position,
                domain
            ))])
        }
        "mouse_scroll" => {
            let domain = arg_string(args, "domain")?;
            let direction = parse_scroll_direction(&arg_string(args, "direction")?)?;
            let clicks = arg_u32(args, "clicks", DEFAULT_SCROLL_CLICKS)?;
            let mut session = connect_spice(libvirt, &domain)?;
            session.mouse_scroll(direction, clicks)?;
            Ok(vec![text_content(format!(
                "scrolled {} {} click(s) on '{}'",
                direction_label(&direction),
                clicks,
                domain
            ))])
        }
        "mouse_drag" => {
            let domain = arg_string(args, "domain")?;
            let from_x = arg_required_u32(args, "from_x")?;
            let from_y = arg_required_u32(args, "from_y")?;
            let to_x = arg_required_u32(args, "to_x")?;
            let to_y = arg_required_u32(args, "to_y")?;
            let mut session = connect_spice(libvirt, &domain)?;
            session.mouse_drag((from_x, from_y), (to_x, to_y))?;
            Ok(vec![text_content(format!(
                "dragged ({}, {}) -> ({}, {}) on '{}'",
                from_x, from_y, to_x, to_y, domain
            ))])
        }
        other => Err(format!(
            "unknown tool '{}'; available tools: {}",
            other,
            tool_name_list()
        )),
    }
}

/// Resolve a domain name to a SPICE endpoint and open a fresh session.
fn connect_spice(libvirt: &Libvirt, domain: &str) -> Result<SpiceSession, String> {
    let info = libvirt.domain(domain)?;
    let endpoint = match info.spice.as_ref() {
        Some(endpoint) => endpoint,
        None => {
            return Err(format!(
                "domain '{}' has no SPICE display endpoint (state: {}); \
                 start the domain and check that its XML has <graphics type='spice'>",
                domain, info.state
            ))
        }
    };
    let password = match std::env::var("MCP_LIBVIRT_SPICE_PASSWORD") {
        Ok(value) => value,
        Err(_) => String::new(),
    };
    SpiceSession::connect(endpoint, &password)
}

fn format_domain_table(domains: &[DomainInfo]) -> String {
    if domains.is_empty() {
        return "no domains found".to_string();
    }
    let headers: [String; 4] = [
        "NAME".to_string(),
        "ID".to_string(),
        "STATE".to_string(),
        "SPICE".to_string(),
    ];
    let rows: Vec<[String; 4]> = domains
        .iter()
        .map(|domain| {
            [
                domain.name.clone(),
                match domain.id {
                    Some(id) => id.to_string(),
                    None => "-".to_string(),
                },
                domain.state.clone(),
                match domain.spice.as_ref() {
                    Some(endpoint) => endpoint.display(),
                    None => "-".to_string(),
                },
            ]
        })
        .collect();

    let mut widths = [0usize; 4];
    for (index, header) in headers.iter().enumerate() {
        widths[index] = header.len();
    }
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            if cell.len() > widths[index] {
                widths[index] = cell.len();
            }
        }
    }

    let mut lines = vec![format_table_row(&headers, &widths)];
    for row in &rows {
        lines.push(format_table_row(row, &widths));
    }
    lines.join("\n")
}

fn format_table_row(row: &[String; 4], widths: &[usize; 4]) -> String {
    let mut line = String::new();
    for index in 0..4 {
        if index > 0 {
            line.push_str("  ");
        }
        line.push_str(&row[index]);
        if index < 3 {
            for _ in row[index].len()..widths[index] {
                line.push(' ');
            }
        }
    }
    line
}

fn tool_name_list() -> String {
    tool_definitions()
        .iter()
        .filter_map(|definition| definition["name"].as_str().map(|name| name.to_string()))
        .collect::<Vec<String>>()
        .join(", ")
}

fn parse_button(text: &str) -> Result<Button, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "left" => Ok(Button::Left),
        "middle" => Ok(Button::Middle),
        "right" => Ok(Button::Right),
        other => Err(format!(
            "argument 'button' must be one of \"left\", \"middle\", \"right\" (got \"{}\")",
            other
        )),
    }
}

fn parse_scroll_direction(text: &str) -> Result<ScrollDir, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "up" => Ok(ScrollDir::Up),
        "down" => Ok(ScrollDir::Down),
        other => Err(format!(
            "argument 'direction' must be \"up\" or \"down\" (got \"{}\")",
            other
        )),
    }
}

fn button_label(button: &Button) -> String {
    match button {
        Button::Left => "left".to_string(),
        Button::Middle => "middle".to_string(),
        Button::Right => "right".to_string(),
    }
}

fn direction_label(direction: &ScrollDir) -> String {
    match direction {
        ScrollDir::Up => "up".to_string(),
        ScrollDir::Down => "down".to_string(),
    }
}

fn text_content(text: String) -> Value {
    json!({ "type": "text", "text": text })
}

fn image_content(base64_png: String) -> Value {
    json!({ "type": "image", "data": base64_png, "mimeType": "image/png" })
}

fn arg_string(args: &Value, key: &str) -> Result<String, String> {
    match args.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(format!("argument '{}' must be a string", key)),
        None => Err(format!("missing required argument '{}'", key)),
    }
}

fn arg_u32(args: &Value, key: &str, default: u32) -> Result<u32, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => number_to_u32(value)
            .ok_or_else(|| format!("argument '{}' must be a non-negative integer", key)),
    }
}

fn arg_required_u32(args: &Value, key: &str) -> Result<u32, String> {
    match args.get(key) {
        None | Some(Value::Null) => Err(format!("missing required argument '{}'", key)),
        Some(value) => number_to_u32(value)
            .ok_or_else(|| format!("argument '{}' must be a non-negative integer", key)),
    }
}

fn arg_optional_u32(args: &Value, key: &str) -> Result<Option<u32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match number_to_u32(value) {
            Some(number) => Ok(Some(number)),
            None => Err(format!(
                "argument '{}' must be a non-negative integer",
                key
            )),
        },
    }
}

fn arg_u64(args: &Value, key: &str, default: u64) -> Result<u64, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| format!("argument '{}' must be a non-negative integer", key)),
    }
}

fn arg_bool(args: &Value, key: &str, default: bool) -> Result<bool, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("argument '{}' must be a boolean", key)),
    }
}

fn number_to_u32(value: &Value) -> Option<u32> {
    let number = value.as_u64()?;
    u32::try_from(number).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_libvirt() -> Libvirt {
        Libvirt {
            uri: "qemu:///system".to_string(),
        }
    }

    #[test]
    fn tool_definitions_cover_the_documented_tool_set() {
        let definitions = tool_definitions();
        let names: Vec<&str> = definitions
            .iter()
            .map(|definition| definition["name"].as_str().expect("tool name"))
            .collect();
        assert_eq!(
            names,
            vec![
                "list_domains",
                "screenshot",
                "type_text",
                "key_press",
                "mouse_move",
                "mouse_click",
                "mouse_scroll",
                "mouse_drag"
            ]
        );
        for definition in &definitions {
            let description = definition["description"].as_str().expect("description");
            assert!(!description.is_empty());
            assert_eq!(definition["inputSchema"]["type"], "object");
            assert!(definition["inputSchema"]["properties"].is_object());
            assert!(definition["inputSchema"]["required"].is_array());
        }
        assert_eq!(
            tool_definitions()[1]["inputSchema"]["required"],
            json!(["domain"])
        );
        assert_eq!(
            tool_definitions()[5]["inputSchema"]["properties"]["button"]["enum"],
            json!(["left", "middle", "right"])
        );
    }

    #[test]
    fn unknown_tool_reports_an_error_with_the_tool_list() {
        let output = call_tool(&test_libvirt(), "teleport", None);
        assert!(output.is_error);
        let text = output.content[0]["text"].as_str().expect("error text");
        assert!(text.contains("unknown tool 'teleport'"), "got: {}", text);
        assert!(text.contains("list_domains"), "got: {}", text);
    }

    #[test]
    fn empty_tool_name_is_reported() {
        let output = call_tool(&test_libvirt(), "", Some(&json!({})));
        assert!(output.is_error);
        let text = output.content[0]["text"].as_str().expect("error text");
        assert!(text.contains("'name'"), "got: {}", text);
    }

    #[test]
    fn domain_tools_require_the_domain_argument() {
        let libvirt = test_libvirt();
        for name in [
            "screenshot",
            "type_text",
            "key_press",
            "mouse_move",
            "mouse_click",
            "mouse_scroll",
            "mouse_drag",
        ] {
            let output = call_tool(&libvirt, name, Some(&json!({})));
            assert!(output.is_error, "{} must fail without a domain", name);
            let text = output.content[0]["text"].as_str().expect("error text");
            assert!(text.contains("'domain'"), "{}: got {}", name, text);
        }
    }

    #[test]
    fn mouse_click_rejects_unknown_button() {
        let output = call_tool(
            &test_libvirt(),
            "mouse_click",
            Some(&json!({"domain": "x", "button": "thumb"})),
        );
        assert!(output.is_error);
        let text = output.content[0]["text"].as_str().expect("error text");
        assert!(text.contains("'button'"), "got: {}", text);
    }

    #[test]
    fn mouse_click_requires_both_coordinates() {
        let output = call_tool(
            &test_libvirt(),
            "mouse_click",
            Some(&json!({"domain": "x", "button": "left", "x": 3})),
        );
        assert!(output.is_error);
        let text = output.content[0]["text"].as_str().expect("error text");
        assert!(text.contains("'x' and 'y'"), "got: {}", text);
    }

    #[test]
    fn mouse_scroll_rejects_unknown_direction() {
        let output = call_tool(
            &test_libvirt(),
            "mouse_scroll",
            Some(&json!({"domain": "x", "direction": "sideways"})),
        );
        assert!(output.is_error);
        let text = output.content[0]["text"].as_str().expect("error text");
        assert!(text.contains("'direction'"), "got: {}", text);
    }

    #[test]
    fn integer_arguments_reject_wrong_types() {
        let output = call_tool(
            &test_libvirt(),
            "mouse_move",
            Some(&json!({"domain": "x", "x": "3", "y": 4})),
        );
        assert!(output.is_error);
        let text = output.content[0]["text"].as_str().expect("error text");
        assert!(text.contains("'x'"), "got: {}", text);
    }

    #[test]
    fn domain_table_is_aligned_and_renders_placeholders() {
        let domains = vec![
            DomainInfo {
                name: "Windows10".to_string(),
                id: None,
                state: "shut off".to_string(),
                spice: None,
            },
            DomainInfo {
                name: "Debian".to_string(),
                id: Some(3),
                state: "running".to_string(),
                spice: Some(crate::libvirt::SpiceEndpoint::Tcp {
                    host: "127.0.0.1".to_string(),
                    port: 5900,
                }),
            },
        ];
        let table = format_domain_table(&domains);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("NAME"));
        assert!(lines[0].contains("STATE"));
        assert!(lines[0].contains("SPICE"));
        assert!(lines[1].starts_with("Windows10"));
        assert!(lines[1].ends_with('-'));
        assert!(lines[2].contains("127.0.0.1:5900"));
        assert!(lines[2].contains("running"));
    }

    #[test]
    fn empty_domain_list_says_so() {
        assert_eq!(format_domain_table(&[]), "no domains found");
    }

    #[test]
    fn image_content_uses_base64_png_shape() {
        let content = image_content("aGVsbG8=".to_string());
        assert_eq!(content["type"], "image");
        assert_eq!(content["mimeType"], "image/png");
        assert_eq!(content["data"], "aGVsbG8=");
    }
}
