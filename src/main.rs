//! mcp-libvirt: an MCP server that drives libvirt domains over SPICE.
//!
//! `main.rs` is the crate root (there is no `lib.rs`). It builds the libvirt
//! handle and serves the MCP protocol on stdio; stdout belongs to the `rmcp`
//! transport, diagnostics go to stderr.

mod libvirt;
mod mcp;
mod spice;
mod tools;

use std::sync::Arc;

use rmcp::{transport::stdio, ServiceExt};

use crate::libvirt::Libvirt;
use crate::mcp::LibvirtTools;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let libvirt = Arc::new(Libvirt::new());
    let tools = LibvirtTools::new(Arc::clone(&libvirt));
    eprintln!(
        "{} {}: libvirt URI '{}'",
        mcp::SERVER_NAME,
        mcp::SERVER_VERSION,
        libvirt.uri
    );

    let service = tools.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
