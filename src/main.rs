//! mcp-libvirt: an MCP server that drives libvirt domains over SPICE.
//!
//! `main.rs` is the crate root (there is no `lib.rs`). It builds the libvirt
//! handle, wires up the tool registry and serves the JSON-RPC loop on stdio.

mod libvirt;
mod mcp;
mod spice;
mod tools;

use libvirt::Libvirt;
use mcp::McpServer;

fn main() {
    let libvirt = Libvirt::new();
    eprintln!(
        "mcp-libvirt {}: libvirt URI '{}'",
        mcp::SERVER_VERSION,
        libvirt.uri
    );

    let server = McpServer::new(libvirt);
    if let Err(message) = server.run() {
        eprintln!("mcp-libvirt: fatal: {}", message);
        std::process::exit(1);
    }
}
