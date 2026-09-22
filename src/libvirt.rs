//! libvirt access through the official `virt` crate (FFI bindings to libvirt).
//!
//! The public contract (`Libvirt`, `DomainInfo`, `SpiceEndpoint`) is unchanged
//! from the earlier virsh-based implementation; only the transport changed.
//! A connection is opened for each call and dropped afterwards, which keeps
//! `Libvirt` a plain shareable handle holding just the connection URI.

use virt::connect::Connect;
use virt::domain::Domain;
use virt::sys;

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
        let connection = self.open()?;
        let domains = connection
            .list_all_domains(0)
            .map_err(|e| format!("cannot list domains on '{}': {}", self.uri, e))?;
        let mut infos = Vec::with_capacity(domains.len());
        for domain in &domains {
            let (name, state) = domain_name_and_state(domain)?;
            // A listing must not fail because one domain's XML cannot be read
            // (the domain may have gone away between the two calls).
            let spice = match spice_endpoint(domain, &name) {
                Ok(endpoint) => endpoint,
                Err(_) => None,
            };
            infos.push(DomainInfo {
                name,
                id: domain.get_id(),
                state,
                spice,
            });
        }
        Ok(infos)
    }

    pub fn domain(&self, name: &str) -> Result<DomainInfo, String> {
        let connection = self.open()?;
        let domain = Domain::lookup_by_name(&connection, name)
            .map_err(|e| format!("domain '{}' is not available: {}", name, e))?;
        let (name, state) = domain_name_and_state(&domain)?;
        // The tools need the endpoint, so XML problems are reported here rather
        // than being mistaken for "this domain has no SPICE display".
        let spice = spice_endpoint(&domain, &name)?;
        Ok(DomainInfo {
            name,
            id: domain.get_id(),
            state,
            spice,
        })
    }

    fn open(&self) -> Result<Connect, String> {
        Connect::open(Some(&self.uri))
            .map_err(|e| format!("cannot connect to libvirt at '{}': {}", self.uri, e))
    }
}

fn domain_name_and_state(domain: &Domain) -> Result<(String, String), String> {
    let name = domain
        .get_name()
        .map_err(|e| format!("cannot read the name of domain id {:?}: {}", domain.get_id(), e))?;
    let (state, _reason) = domain
        .get_state()
        .map_err(|e| format!("cannot read the state of domain '{}': {}", name, e))?;
    Ok((name, state_name(state)))
}

/// SPICE endpoint of a domain, taken from its live XML (flags 0). For a running
/// domain with `autoport='yes'` the live XML carries the port that was actually
/// assigned.
fn spice_endpoint(domain: &Domain, name: &str) -> Result<Option<SpiceEndpoint>, String> {
    let xml = domain
        .get_xml_desc(0)
        .map_err(|e| format!("cannot read the XML of domain '{}': {}", name, e))?;
    Ok(parse_spice_from_xml(&xml))
}

/// libvirt's numeric `virDomainState` as the human-readable strings used in
/// tool output.
fn state_name(state: sys::virDomainState) -> String {
    match state {
        sys::VIR_DOMAIN_NOSTATE => "no state".to_string(),
        sys::VIR_DOMAIN_RUNNING => "running".to_string(),
        sys::VIR_DOMAIN_BLOCKED => "blocked".to_string(),
        sys::VIR_DOMAIN_PAUSED => "paused".to_string(),
        sys::VIR_DOMAIN_SHUTDOWN => "shutdown".to_string(),
        sys::VIR_DOMAIN_SHUTOFF => "shut off".to_string(),
        sys::VIR_DOMAIN_CRASHED => "crashed".to_string(),
        sys::VIR_DOMAIN_PMSUSPENDED => "pmsuspended".to_string(),
        other => format!("unknown (state {})", other),
    }
}

/// Parse the SPICE endpoint out of a domain's XML description.
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

fn spice_endpoint_from_element(
    graphics_attrs: &[(String, String)],
    body: &str,
) -> Option<SpiceEndpoint> {
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
    fn state_names_match_libvirt_states() {
        assert_eq!(state_name(sys::VIR_DOMAIN_NOSTATE), "no state");
        assert_eq!(state_name(sys::VIR_DOMAIN_RUNNING), "running");
        assert_eq!(state_name(sys::VIR_DOMAIN_BLOCKED), "blocked");
        assert_eq!(state_name(sys::VIR_DOMAIN_PAUSED), "paused");
        assert_eq!(state_name(sys::VIR_DOMAIN_SHUTDOWN), "shutdown");
        assert_eq!(state_name(sys::VIR_DOMAIN_SHUTOFF), "shut off");
        assert_eq!(state_name(sys::VIR_DOMAIN_CRASHED), "crashed");
        assert_eq!(state_name(sys::VIR_DOMAIN_PMSUSPENDED), "pmsuspended");
        assert_eq!(state_name(42), "unknown (state 42)");
    }

    #[test]
    fn xml_of_a_stopped_autoport_domain_has_no_known_port() {
        // Live XML of a stopped domain with autoport: no port attribute yet.
        let xml = "    <graphics type='spice' autoport='yes'>\n      <listen type='address'/>\n      <image compression='off'/>\n    </graphics>\n";
        assert_eq!(parse_spice_from_xml(xml), None);
    }

    #[test]
    fn xml_of_a_running_autoport_domain_reports_the_assigned_port() {
        // Live XML of a running domain: libvirt fills in the assigned port.
        let xml = "    <graphics type='spice' port='5900' autoport='yes'>\n      <listen type='address' address='127.0.0.1'/>\n    </graphics>\n";
        assert_eq!(
            parse_spice_from_xml(xml),
            Some(SpiceEndpoint::Tcp {
                host: "127.0.0.1".to_string(),
                port: 5900
            })
        );
    }

    #[test]
    fn xml_fixed_port_with_address_listen() {
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
    fn xml_socket_listen_wins() {
        let xml = "    <graphics type='spice' autoport='no' port='5900'>\n      <listen type='socket' path='/run/libvirt/qemu/spice.sock'/>\n    </graphics>\n";
        assert_eq!(
            parse_spice_from_xml(xml),
            Some(SpiceEndpoint::Unix {
                path: "/run/libvirt/qemu/spice.sock".to_string()
            })
        );
    }

    #[test]
    fn xml_double_quotes_and_legacy_listen_attribute() {
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
    fn xml_non_spice_graphics_is_none() {
        let xml = "    <graphics type='vnc' port='5901' autoport='no' listen='127.0.0.1'/>\n";
        assert_eq!(parse_spice_from_xml(xml), None);
    }

    #[test]
    fn xml_picks_spice_among_several_graphics() {
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
    fn xml_port_zero_is_unknown() {
        let xml = "    <graphics type='spice' port='0' autoport='yes'/>\n";
        assert_eq!(parse_spice_from_xml(xml), None);
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
