# mcp-libvirt-vm-use

An MCP server that drives libvirt domains over SPICE. It exposes eight tools to
an MCP client: list domains, take screenshots, and send keyboard and mouse input.
It is a single Rust binary that speaks MCP on stdio.

## Requirements

- A Linux host running libvirt, with the domains you want to control.
- Each VM's SPICE display must be reachable over TCP. Unix-socket SPICE endpoints
  are not supported.
- Nix with flakes enabled, or a Rust toolchain plus `pkg-config` and libvirt
  headers for a plain `cargo` build.

## Build and run

With flakes:

```sh
nix build            # binary at ./result/bin/mcp-libvirt-vm-use
nix run              # start the MCP server on stdio
```

For development:

```sh
nix develop -c cargo build
nix develop -c cargo test
```

The server reads its libvirt connection from `LIBVIRT_DEFAULT_URI` and falls back
to `qemu:///system`. It prints the chosen URI to stderr at startup; stdout is
reserved for the MCP transport.

## MCP client configuration

Point the client at the binary. For a generic MCP client:

```json
{
  "mcpServers": {
    "libvirt-vm-use": {
      "command": "/path/to/mcp-libvirt-vm-use",
      "env": { "LIBVIRT_DEFAULT_URI": "qemu:///system" }
    }
  }
}
```

You can also let Nix fetch and run it directly:

```json
{
  "mcpServers": {
    "libvirt-vm-use": {
      "command": "nix",
      "args": ["run", "github:xddxdd/mcp-libvirt-vm-use"]
    }
  }
}
```

## Tools

| Tool | Purpose | Parameters |
| --- | --- | --- |
| `list_domains` | Table of domains with id, state and SPICE endpoint | none |
| `screenshot` | Capture the SPICE display as a PNG | `domain`, `wait_ms` |
| `type_text` | Type ASCII text | `domain`, `text`, `interval_ms` |
| `key_press` | Press a key combination | `domain`, `keys` (e.g. `ctrl+alt+t`) |
| `mouse_move` | Move the pointer | `domain`, `x`, `y` |
| `mouse_click` | Click a button | `domain`, `button`, `x`, `y`, `double_click` |
| `mouse_scroll` | Scroll the wheel | `domain`, `direction`, `clicks` |
| `mouse_drag` | Drag with the left button | `domain`, `from_x`, `from_y`, `to_x`, `to_y` |

Optional parameters are `wait_ms` (default 500), `interval_ms` (default 20),
`clicks` (default 1) and `double_click` (default false). Coordinates are in guest
pixels.

## Notes and limitations

- `type_text` assumes a US keyboard layout.
- Every screenshot or input call opens a short-lived SPICE session; nothing is
  kept between calls.
- Screenshots come back through the SPICE display channel, so they reflect
  whatever the guest is currently rendering.

## License

GPL-3.0-only. See [LICENSE](LICENSE).
