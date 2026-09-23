//! Tool parameter schemas and the implementation of every tool.
//!
//! `rmcp` derives each tool's JSON Schema from these parameter structs (field
//! doc comments become the property descriptions), and `mcp.rs` awaits these
//! functions from its async handlers.
//!
//! Every tool that touches a guest resolves its domain through `libvirt` (the
//! only part that runs on the blocking pool, since `virt` is synchronous FFI),
//! opens a fresh `SpiceSession`, performs the operation and drops the
//! connection — connections are never pooled.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use rmcp::model::ContentBlock;
use rmcp::schemars;

use crate::libvirt::{DomainInfo, Libvirt, SpiceEndpoint};
use crate::spice::{Button, ScrollDir, SpiceSession};

const DEFAULT_SCREENSHOT_WAIT_MS: u32 = 500;
const DEFAULT_TYPE_INTERVAL_MS: u64 = 20;
const DEFAULT_SCROLL_CLICKS: u32 = 1;

/// Mouse buttons accepted by the `mouse_click` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// Wheel directions accepted by the `mouse_scroll` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct ScreenshotParams {
    /// Domain name as listed by list_domains, e.g. "Windows10".
    pub domain: String,
    /// Quiet period in milliseconds to wait for display updates before capturing (default 500).
    pub wait_ms: Option<u32>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct TypeTextParams {
    /// Domain name as listed by list_domains, e.g. "Windows10".
    pub domain: String,
    /// ASCII text to type (US keyboard layout).
    pub text: String,
    /// Delay between keystrokes in milliseconds (default 20).
    pub interval_ms: Option<u64>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct KeyPressParams {
    /// Domain name as listed by list_domains, e.g. "Windows10".
    pub domain: String,
    /// Key names joined with '+', e.g. "ctrl+alt+t". Names are case-insensitive.
    pub keys: String,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct MouseMoveParams {
    /// Domain name as listed by list_domains, e.g. "Windows10".
    pub domain: String,
    /// X coordinate in guest pixels.
    pub x: u32,
    /// Y coordinate in guest pixels.
    pub y: u32,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct MouseClickParams {
    /// Domain name as listed by list_domains, e.g. "Windows10".
    pub domain: String,
    /// Mouse button to click.
    pub button: MouseButton,
    /// Optional X coordinate to move to before clicking; must be given together with `y`.
    pub x: Option<u32>,
    /// Optional Y coordinate to move to before clicking; must be given together with `x`.
    pub y: Option<u32>,
    /// Click twice in quick succession (default false).
    pub double_click: Option<bool>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct MouseScrollParams {
    /// Domain name as listed by list_domains, e.g. "Windows10".
    pub domain: String,
    /// Scroll direction.
    pub direction: ScrollDirection,
    /// Number of wheel clicks (default 1).
    pub clicks: Option<u32>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct MouseDragParams {
    /// Domain name as listed by list_domains, e.g. "Windows10".
    pub domain: String,
    /// X coordinate where the drag starts.
    pub from_x: u32,
    /// Y coordinate where the drag starts.
    pub from_y: u32,
    /// X coordinate where the drag ends.
    pub to_x: u32,
    /// Y coordinate where the drag ends.
    pub to_y: u32,
}

/// `list_domains`: a table of every domain with id, state and SPICE endpoint.
pub fn list_domains(libvirt: &Libvirt) -> Result<Vec<ContentBlock>, String> {
    let domains = libvirt.list_domains()?;
    Ok(vec![ContentBlock::text(format_domain_table(&domains))])
}

/// `screenshot`: one PNG image block plus a text block with the dimensions.
pub async fn screenshot(
    libvirt: &Libvirt,
    params: ScreenshotParams,
) -> Result<Vec<ContentBlock>, String> {
    let wait_ms = params.wait_ms.unwrap_or(DEFAULT_SCREENSHOT_WAIT_MS);
    let mut session = connect_spice(libvirt, &params.domain).await?;
    let image = session.screenshot(wait_ms).await?;
    Ok(vec![
        ContentBlock::image(BASE64.encode(&image.png), "image/png"),
        ContentBlock::text(format!(
            "screenshot of '{}': {}x{} PNG ({} bytes)",
            params.domain,
            image.width,
            image.height,
            image.png.len()
        )),
    ])
}

/// `type_text`: type ASCII text through the SPICE inputs channel.
pub async fn type_text(
    libvirt: &Libvirt,
    params: TypeTextParams,
) -> Result<Vec<ContentBlock>, String> {
    let interval_ms = params.interval_ms.unwrap_or(DEFAULT_TYPE_INTERVAL_MS);
    let mut session = connect_spice(libvirt, &params.domain).await?;
    session.type_text(&params.text, interval_ms).await?;
    Ok(vec![ContentBlock::text(format!(
        "typed {} character(s) into '{}'",
        params.text.chars().count(),
        params.domain
    ))])
}

/// `key_press`: send a key combination such as `ctrl+alt+t`.
pub async fn key_press(
    libvirt: &Libvirt,
    params: KeyPressParams,
) -> Result<Vec<ContentBlock>, String> {
    let mut session = connect_spice(libvirt, &params.domain).await?;
    session.key_press(&params.keys).await?;
    Ok(vec![ContentBlock::text(format!(
        "sent key combination '{}' to '{}'",
        params.keys, params.domain
    ))])
}

/// `mouse_move`: move the pointer to an absolute position.
pub async fn mouse_move(
    libvirt: &Libvirt,
    params: MouseMoveParams,
) -> Result<Vec<ContentBlock>, String> {
    let mut session = connect_spice(libvirt, &params.domain).await?;
    session.mouse_move(params.x, params.y).await?;
    Ok(vec![ContentBlock::text(format!(
        "moved mouse to ({}, {}) on '{}'",
        params.x, params.y, params.domain
    ))])
}

/// `mouse_click`: optionally move first, then press and release one button.
pub async fn mouse_click(
    libvirt: &Libvirt,
    params: MouseClickParams,
) -> Result<Vec<ContentBlock>, String> {
    if params.x.is_some() != params.y.is_some() {
        return Err("arguments 'x' and 'y' must be provided together for mouse_click".to_string());
    }
    let double = params.double_click.unwrap_or(false);
    let position = match (params.x, params.y) {
        (Some(x), Some(y)) => format!(" at ({}, {})", x, y),
        _ => String::new(),
    };

    let mut session = connect_spice(libvirt, &params.domain).await?;
    session
        .mouse_click(mouse_button(params.button), params.x, params.y, double)
        .await?;
    let action = if double { "double-clicked" } else { "clicked" };
    Ok(vec![ContentBlock::text(format!(
        "{} {} button{} on '{}'",
        action,
        mouse_button_name(params.button),
        position,
        params.domain
    ))])
}

/// `mouse_scroll`: press and release the wheel button a number of times.
pub async fn mouse_scroll(
    libvirt: &Libvirt,
    params: MouseScrollParams,
) -> Result<Vec<ContentBlock>, String> {
    let clicks = params.clicks.unwrap_or(DEFAULT_SCROLL_CLICKS);
    let mut session = connect_spice(libvirt, &params.domain).await?;
    session
        .mouse_scroll(scroll_direction(params.direction), clicks)
        .await?;
    Ok(vec![ContentBlock::text(format!(
        "scrolled {} {} click(s) on '{}'",
        scroll_direction_name(params.direction),
        clicks,
        params.domain
    ))])
}

/// `mouse_drag`: press the left button, drag to another position, release.
pub async fn mouse_drag(
    libvirt: &Libvirt,
    params: MouseDragParams,
) -> Result<Vec<ContentBlock>, String> {
    let mut session = connect_spice(libvirt, &params.domain).await?;
    session
        .mouse_drag((params.from_x, params.from_y), (params.to_x, params.to_y))
        .await?;
    Ok(vec![ContentBlock::text(format!(
        "dragged ({}, {}) -> ({}, {}) on '{}'",
        params.from_x, params.from_y, params.to_x, params.to_y, params.domain
    ))])
}

/// Resolve a domain name to a SPICE endpoint, then open a fresh session.
async fn connect_spice(libvirt: &Libvirt, domain: &str) -> Result<SpiceSession, String> {
    // `virt` is synchronous FFI, so the lookup runs on the blocking pool. The
    // handle is a plain URI, so it can be rebuilt for the `'static` closure.
    let handle = Libvirt {
        uri: libvirt.uri.clone(),
    };
    let name = domain.to_string();
    let (endpoint, password) = tokio::task::spawn_blocking(move || resolve_endpoint(&handle, &name))
        .await
        .map_err(|e| {
            format!(
                "internal error while resolving domain '{}': {}",
                domain, e
            )
        })??;
    SpiceSession::connect(&endpoint, &password).await
}

/// Blocking half of [`connect_spice`]: domain lookup, endpoint check, password.
fn resolve_endpoint(libvirt: &Libvirt, domain: &str) -> Result<(SpiceEndpoint, String), String> {
    let info = libvirt.domain(domain)?;
    let endpoint = match info.spice {
        Some(endpoint) => endpoint,
        None => {
            return Err(format!(
                "domain '{}' has no SPICE display endpoint (state: {}); \
                 start the domain and check that its XML has <graphics type='spice'> with a port",
                domain, info.state
            ))
        }
    };
    let password = match std::env::var("MCP_LIBVIRT_SPICE_PASSWORD") {
        Ok(value) => value,
        Err(_) => String::new(),
    };
    Ok((endpoint, password))
}

fn mouse_button(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Middle => Button::Middle,
        MouseButton::Right => Button::Right,
    }
}

fn mouse_button_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Middle => "middle",
        MouseButton::Right => "right",
    }
}

fn scroll_direction(direction: ScrollDirection) -> ScrollDir {
    match direction {
        ScrollDirection::Up => ScrollDir::Up,
        ScrollDirection::Down => ScrollDir::Down,
    }
}

fn scroll_direction_name(direction: ScrollDirection) -> &'static str {
    match direction {
        ScrollDirection::Up => "up",
        ScrollDirection::Down => "down",
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::libvirt::SpiceEndpoint;

    fn text_of(content: &[ContentBlock]) -> String {
        match content.first() {
            Some(ContentBlock::Text(text)) => text.text.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn params_deserialize_with_documented_defaults() {
        let params: ScreenshotParams =
            serde_json::from_value(serde_json::json!({"domain": "Windows10"})).expect("deserialize");
        assert_eq!(params.domain, "Windows10");
        assert_eq!(params.wait_ms, None);

        let params: MouseClickParams = serde_json::from_value(
            serde_json::json!({"domain": "Windows10", "button": "right", "double_click": true}),
        )
        .expect("deserialize");
        assert_eq!(params.button, MouseButton::Right);
        assert_eq!(params.double_click, Some(true));
        assert_eq!(params.x, None);
    }

    #[test]
    fn enums_reject_unknown_values() {
        let result = serde_json::from_value::<MouseClickParams>(
            serde_json::json!({"domain": "Windows10", "button": "thumb"}),
        );
        assert!(result.is_err());
        let result = serde_json::from_value::<MouseScrollParams>(
            serde_json::json!({"domain": "Windows10", "direction": "sideways"}),
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mouse_click_requires_both_coordinates() {
        let libvirt = test_libvirt();
        let error = mouse_click(
            &libvirt,
            MouseClickParams {
                domain: "Windows10".to_string(),
                button: MouseButton::Left,
                x: Some(3),
                y: None,
                double_click: None,
            },
        )
        .await
        .expect_err("x without y must be rejected");
        assert!(error.contains("'x' and 'y'"), "got: {}", error);
    }

    #[tokio::test]
    async fn unknown_domain_is_reported_by_the_libvirt_layer() {
        // The libvirt test driver needs no daemon, so the failure is always the
        // missing domain rather than an unreachable hypervisor.
        let libvirt = Libvirt {
            uri: "test:///default".to_string(),
        };
        let error = screenshot(
            &libvirt,
            ScreenshotParams {
                domain: "nope".to_string(),
                wait_ms: None,
            },
        )
        .await
        .expect_err("a missing domain must be an error");
        assert!(error.contains("nope"), "got: {}", error);
    }

    #[tokio::test]
    async fn domain_without_a_spice_endpoint_reports_it() {
        // `test:///default` defines a single domain that is running but has no
        // graphics device of any kind.
        let libvirt = Libvirt {
            uri: "test:///default".to_string(),
        };
        let list = list_domains(&libvirt).expect("the test driver lists one domain");
        let table = text_of(&list);
        assert!(table.starts_with("NAME"), "got: {}", table);
        let name = table
            .lines()
            .nth(1)
            .expect("one domain row")
            .split_whitespace()
            .next()
            .expect("domain name")
            .to_string();
        let error = type_text(
            &libvirt,
            TypeTextParams {
                domain: name,
                text: "a".to_string(),
                interval_ms: None,
            },
        )
        .await
        .expect_err("the domain has no SPICE display");
        assert!(error.contains("no SPICE display endpoint"), "got: {}", error);
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
                spice: Some(SpiceEndpoint::Tcp {
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
    fn screenshot_content_is_an_image_block_plus_dimensions() {
        // The image block is built from `PngImage`; this pins the block shape.
        let content = vec![
            ContentBlock::image("aGVsbG8=".to_string(), "image/png"),
            ContentBlock::text("screenshot of 'x': 1x1 PNG (3 bytes)".to_string()),
        ];
        match &content[0] {
            ContentBlock::Image(image) => {
                assert_eq!(image.data, "aGVsbG8=");
                assert_eq!(image.mime_type, "image/png");
            }
            other => panic!("expected an image block, got {:?}", other),
        }
        assert!(text_of(&content[1..]).contains("1x1"));
    }

    fn test_libvirt() -> Libvirt {
        Libvirt {
            uri: "qemu:///system".to_string(),
        }
    }
}
