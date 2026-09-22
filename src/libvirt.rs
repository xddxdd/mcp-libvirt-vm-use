//! libvirt access through the `virsh` CLI.
//!
//! There are no libvirt development headers on the target host, so every lookup
//! shells out to `virsh --connect <uri> ...` and parses the human-readable
//! output. Only `list_domains`, `domain` and the pure parsing helpers below are
//! part of the public contract used by `tools.rs`.

use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainInfo {
    pub name: String,
    pub id: Option<u32>,
    pub state: String,
    pub spice: Option<SpiceEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpiceEndpoint {
    Tcp { host: String, port: u16 },
    Unix { path: String },
}

impl SpiceEndpoint {
    /// Short human-readable form used in tool output, e.g. `127.0.0.1:5900`
    /// or `unix:/run/libvirt/spice.sock`.
    pub fn display(&self) -> String {
        match self {
            SpiceEndpoint::Tcp { host, port } => format!("{}:{}", host, port),
            SpiceEndpoint::Unix { path } => format!("unix:{}", path),
        }
    }
}

pub struct Libvirt {
    pub uri: String,
}

impl Libvirt {
    /// URI from `LIBVIRT_DEFAULT_URI`, falling back to the system connection.
    pub fn new() -> Self {
        let uri = match std::env::var("LIBVIRT_DEFAULT_URI") {
            Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
            _ => "qemu:///system".to_string(),
        };
        Libvirt { uri }
    }

    pub fn list_domains(&self) -> Result<Vec<DomainInfo>, String> {
        let output = self.run(&["list", "--all", "--name"])?;
        let mut domains = Vec::new();
        for line in output.lines() {
            let name = line.trim();
            if name.is_empty() {
                continue;
            }
            domains.push(self.domain_info(name));
        }
        Ok(domains)
    }

    pub fn domain(&self, name: &str) -> Result<DomainInfo, String> {
        let state = self
            .run(&["domstate", name])
            .map_err(|e| format!("domain '{}' is not available: {}", name, e))?;
        Ok(DomainInfo {
            name: name.to_string(),
            id: self.domain_id(name),
            state: state.trim().to_string(),
            spice: self.spice_endpoint(name),
        })
    }

    /// Best-effort info for a name that came out of `virsh list`, so that a
    /// single domain disappearing mid-listing cannot fail the whole table.
    fn domain_info(&self, name: &str) -> DomainInfo {
        let state = match self.run(&["domstate", name]) {
            Ok(output) => output.trim().to_string(),
            Err(_) => "unknown".to_string(),
        };
        DomainInfo {
            name: name.to_string(),
            id: self.domain_id(name),
            state,
            spice: self.spice_endpoint(name),
        }
    }

    fn domain_id(&self, name: &str) -> Option<u32> {
        match self.run(&["domid", name]) {
            Ok(output) => parse_domid(&output),
            Err(_) => None,
        }
    }

    /// SPICE endpoint of a domain: `virsh domdisplay` first (authoritative for
    /// running domains, returns the autoport that was actually assigned), then
    /// the `<graphics type='spice'>` element of `virsh dumpxml` as a fallback
    /// for domains where the display port is fixed in the XML.
    fn spice_endpoint(&self, name: &str) -> Option<SpiceEndpoint> {
        if let Ok(output) = self.run(&["domdisplay", name]) {
            if let Some(endpoint) = parse_domdisplay(&output) {
                return Some(endpoint);
            }
        }
        // Without --type, domdisplay reports the first display, which may be a
        // VNC display on domains that expose both.
        if let Ok(output) = self.run(&["domdisplay", "--type", "spice", name]) {
            if let Some(endpoint) = parse_domdisplay(&output) {
                return Some(endpoint);
            }
        }
        match self.run(&["dumpxml", name]) {
            Ok(xml) => parse_spice_from_xml(&xml),
            Err(_) => None,
        }
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new("virsh")
            .arg("--connect")
            .arg(&self.uri)
            .args(args)
            .output()
            .map_err(|e| format!("failed to run virsh: {} (is libvirt installed?)", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            if stderr.is_empty() {
                return Err(format!("virsh {} failed with {}", args.join(" "), output.status));
            }
            return Err(stderr.to_string());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// `virsh domid` prints the numeric id, or `-` for a domain that is not running.
fn parse_domid(output: &str) -> Option<u32> {
    output.trim().parse::<u32>().ok()
}

/// Parse `virsh domdisplay` output. Accepted forms (whitespace trimmed):
/// `spice://HOST:PORT`, `spice+unix:///path/to/socket`, `spice://?socket=/path`.
/// Anything else (including `vnc://...` and error text) yields `None`.
fn parse_domdisplay(output: &str) -> Option<SpiceEndpoint> {
    let text = output.trim();
    if let Some(rest) = strip_prefix_ci(text, "spice+unix://") {
        return parse_spice_uri_rest(rest);
    }
    if let Some(rest) = strip_prefix_ci(text, "spice://") {
        return parse_spice_uri_rest(rest);
    }
    None
}

fn parse_spice_uri_rest(rest: &str) -> Option<SpiceEndpoint> {
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    if let Some(query) = rest.strip_prefix('?') {
        let mut host: Option<String> = None;
        let mut port: Option<u16> = None;
        let mut socket: Option<String> = None;
        for pair in query.split('&') {
            let (key, value) = match pair.split_once('=') {
                Some(key_value) => key_value,
                None => continue,
            };
            let value = percent_decode(value);
            match key.trim().to_ascii_lowercase().as_str() {
                "socket" => socket = Some(value),
                "host" => host = Some(value),
                "port" => port = value.trim().parse::<u16>().ok(),
                _ => {}
            }
        }
        if let Some(path) = socket {
            return Some(SpiceEndpoint::Unix { path });
        }
        if let Some(port) = port {
            return Some(SpiceEndpoint::Tcp {
                host: host.unwrap_or_else(|| "127.0.0.1".to_string()),
                port,
            });
        }
        return None;
    }
    if rest.starts_with('/') {
        return Some(SpiceEndpoint::Unix {
            path: rest.to_string(),
        });
    }
    match rest.rsplit_once(':') {
        Some((host, port_text)) if !host.is_empty() => port_text
            .trim()
            .parse::<u16>()
            .ok()
            .map(|port| SpiceEndpoint::Tcp {
                host: host.to_string(),
                port,
            }),
        _ => None,
    }
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    match text.get(..prefix.len()) {
        Some(head) if head.eq_ignore_ascii_case(prefix) => Some(&text[prefix.len()..]),
        _ => None,
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push(high * 16 + low);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Parse the SPICE endpoint out of `virsh dumpxml` output.
///
/// Returns `None` when there is no `<graphics type='spice'>` element, and also
/// when the element exists but the port is unknown (for example
/// `autoport='yes'` on a stopped domain) — guessing a port would be worse than
/// reporting no endpoint.
fn parse_spice_from_xml(xml: &str) -> Option<SpiceEndpoint> {
    let mut from = 0;
    loop {
        let open = find_start_tag(xml, "graphics", from)?;
        let tag_end = find_tag_end(xml, open)?;
        let tag = &xml[open..=tag_end];
        let attrs = parse_attributes(tag);
        let is_spice = attrs.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case("type") && value.eq_ignore_ascii_case("spice")
        });
        if is_spice {
            let body = if tag.trim_end().ends_with("/>") {
                ""
            } else {
                match find_close_tag(xml, "graphics", tag_end + 1) {
                    Some(close) => &xml[tag_end + 1..close],
                    None => "",
                }
            };
            return spice_endpoint_from_element(&attrs, body);
        }
        from = tag_end + 1;
    }
}

fn spice_endpoint_from_element(graphics_attrs: &[(String, String)], body: &str) -> Option<SpiceEndpoint> {
    let mut listen_host: Option<String> = None;
    let mut socket_path: Option<String> = None;

    let mut from = 0;
    while let Some(open) = find_start_tag(body, "listen", from) {
        let tag_end = match find_tag_end(body, open) {
            Some(end) => end,
            None => break,
        };
        let attrs = parse_attributes(&body[open..=tag_end]);
        let kind = attribute(&attrs, "type").unwrap_or_default();
        if kind.eq_ignore_ascii_case("socket") {
            if socket_path.is_none() {
                socket_path = attribute(&attrs, "path");
            }
        } else if kind.eq_ignore_ascii_case("address") && listen_host.is_none() {
            listen_host = attribute(&attrs, "address");
        }
        from = tag_end + 1;
    }

    if let Some(path) = socket_path {
        return Some(SpiceEndpoint::Unix { path });
    }

    // Modern libvirt uses a <listen type='address' address='...'/> child;
    // older XML put the address straight on the graphics element.
    let host = attribute(graphics_attrs, "listen")
        .or(listen_host)
        .unwrap_or_else(|| "127.0.0.1".to_string());

    let port_text = attribute(graphics_attrs, "port")?;
    let port: u16 = match port_text.trim().parse() {
        Ok(port) => port,
        Err(_) => return None,
    };
    if port == 0 {
        return None;
    }
    Some(SpiceEndpoint::Tcp { host, port })
}

fn attribute(attrs: &[(String, String)], key: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.clone())
}

/// Index of the `<` of the first `<name` start tag at or after `from`.
fn find_start_tag(text: &str, name: &str, from: usize) -> Option<usize> {
    let needle = format!("<{}", name);
    let mut search = from;
    while search < text.len() {
        let relative = match text[search..].find(&needle) {
            Some(relative) => relative,
            None => return None,
        };
        let open = search + relative;
        let after = &text[open + needle.len()..];
        match after.chars().next() {
            Some(c) if c.is_ascii_whitespace() || c == '>' || c == '/' => return Some(open),
            _ => search = open + needle.len(),
        }
    }
    None
}

/// Index of the `>` closing the start tag that begins at `open`, skipping
/// quoted attribute values.
fn find_tag_end(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = open;
    while i < bytes.len() {
        let byte = bytes[i];
        match quote {
            Some(open_quote) => {
                if byte == open_quote {
                    quote = None;
                }
            }
            None => {
                if byte == b'\'' || byte == b'"' {
                    quote = Some(byte);
                } else if byte == b'>' {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

/// Index of the `<` of the `</name>` end tag at or after `from`.
fn find_close_tag(text: &str, name: &str, from: usize) -> Option<usize> {
    let needle = format!("</{}", name);
    match text.get(from..) {
        Some(rest) => rest.find(&needle).map(|relative| from + relative),
        None => None,
    }
}

/// Extract `key='value'` / `key="value"` pairs from an XML start tag. Bare
/// tokens (the element name) are skipped.
fn parse_attributes(tag: &str) -> Vec<(String, String)> {
    let bytes = tag.as_bytes();
    let mut attrs = Vec::new();
    let mut i = 1; // skip '<'
    while i < bytes.len() {
        if !is_name_char(bytes[i]) {
            i += 1;
            continue;
        }
        let name_start = i;
        while i < bytes.len() && is_name_char(bytes[i]) {
            i += 1;
        }
        let name = tag[name_start..i].to_string();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || (bytes[i] != b'\'' && bytes[i] != b'"') {
            continue;
        }
        let quote = bytes[i];
        i += 1;
        let value_start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        attrs.push((name, tag[value_start..i].to_string()));
    }
    attrs
}

fn is_name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b':' || byte == b'.'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domid_parsing() {
        assert_eq!(parse_domid("-\n"), None);
        assert_eq!(parse_domid(""), None);
        assert_eq!(parse_domid("not a number\n"), None);
        assert_eq!(parse_domid("7\n"), Some(7));
        assert_eq!(parse_domid("  42  "), Some(42));
    }

    #[test]
    fn domdisplay_tcp() {
        assert_eq!(
            parse_domdisplay("spice://127.0.0.1:5900\n"),
            Some(SpiceEndpoint::Tcp {
                host: "127.0.0.1".to_string(),
                port: 5900
            })
        );
    }

    #[test]
    fn domdisplay_unix_socket() {
        assert_eq!(
            parse_domdisplay("spice+unix:///run/libvirt/qemu/spice.sock\n"),
            Some(SpiceEndpoint::Unix {
                path: "/run/libvirt/qemu/spice.sock".to_string()
            })
        );
    }

    #[test]
    fn domdisplay_query_form() {
        assert_eq!(
            parse_domdisplay("spice://?socket=%2Ftmp%2Fspice.sock"),
            Some(SpiceEndpoint::Unix {
                path: "/tmp/spice.sock".to_string()
            })
        );
        assert_eq!(
            parse_domdisplay("spice://?host=10.0.0.5&port=5910"),
            Some(SpiceEndpoint::Tcp {
                host: "10.0.0.5".to_string(),
                port: 5910
            })
        );
    }

    #[test]
    fn domdisplay_rejects_other_display_types() {
        assert_eq!(parse_domdisplay("vnc://127.0.0.1:5900\n"), None);
        assert_eq!(parse_domdisplay("error: Domain is not running\n"), None);
        assert_eq!(parse_domdisplay("spice://127.0.0.1\n"), None);
        assert_eq!(parse_domdisplay(""), None);
    }

    #[test]
    fn dumpxml_autoport_without_port_is_unknown() {
        // Real output of `virsh dumpxml Windows10` on this host.
        let xml = "    <graphics type='spice' autoport='yes'>\n      <listen type='address'/>\n      <image compression='off'/>\n    </graphics>\n";
        assert_eq!(parse_spice_from_xml(xml), None);
    }

    #[test]
    fn dumpxml_fixed_port_with_address_listen() {
        let xml = "    <graphics type='spice' port='5900' autoport='no'>\n      <listen type='address' address='127.0.0.1'/>\n    </graphics>\n";
        assert_eq!(
            parse_spice_from_xml(xml),
            Some(SpiceEndpoint::Tcp {
                host: "127.0.0.1".to_string(),
                port: 5900
            })
        );
    }

    #[test]
    fn dumpxml_socket_listen_wins() {
        let xml = "    <graphics type='spice' autoport='no' port='5900'>\n      <listen type='socket' path='/run/libvirt/qemu/spice.sock'/>\n    </graphics>\n";
        assert_eq!(
            parse_spice_from_xml(xml),
            Some(SpiceEndpoint::Unix {
                path: "/run/libvirt/qemu/spice.sock".to_string()
            })
        );
    }

    #[test]
    fn dumpxml_double_quotes_and_legacy_listen_attribute() {
        let xml = "  <devices>\n    <graphics type=\"spice\" listen=\"10.0.0.5\" port=\"5910\" autoport=\"no\"/>\n  </devices>\n";
        assert_eq!(
            parse_spice_from_xml(xml),
            Some(SpiceEndpoint::Tcp {
                host: "10.0.0.5".to_string(),
                port: 5910
            })
        );
    }

    #[test]
    fn dumpxml_non_spice_graphics_is_none() {
        let xml = "    <graphics type='vnc' port='5901' autoport='no' listen='127.0.0.1'/>\n";
        assert_eq!(parse_spice_from_xml(xml), None);
    }

    #[test]
    fn dumpxml_picks_spice_among_several_graphics() {
        let xml = "    <graphics type='vnc' port='5901' autoport='no'/>\n    <graphics type='spice' port='6000' autoport='no' listen='127.0.0.1'/>\n";
        assert_eq!(
            parse_spice_from_xml(xml),
            Some(SpiceEndpoint::Tcp {
                host: "127.0.0.1".to_string(),
                port: 6000
            })
        );
    }

    #[test]
    fn attributes_are_parsed_from_a_start_tag() {
        let attrs = parse_attributes("<graphics type='spice' autoport='yes'>");
        assert_eq!(
            attrs,
            vec![
                ("type".to_string(), "spice".to_string()),
                ("autoport".to_string(), "yes".to_string())
            ]
        );
    }

    #[test]
    fn endpoint_display_forms() {
        assert_eq!(
            SpiceEndpoint::Tcp {
                host: "127.0.0.1".to_string(),
                port: 5900
            }
            .display(),
            "127.0.0.1:5900"
        );
        assert_eq!(
            SpiceEndpoint::Unix {
                path: "/tmp/spice.sock".to_string()
            }
            .display(),
            "unix:/tmp/spice.sock"
        );
    }
}
