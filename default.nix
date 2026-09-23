# Non-flake entry point: `nix-build ./default.nix`.
# The flake reuses this via `pkgs.callPackage ./default.nix { inherit pkgs; }`.
{ pkgs ? import <nixpkgs> { } }:

pkgs.rustPlatform.buildRustPackage {
  pname = "mcp-libvirt-vm-use";
  version = "0.1.0";

  src = builtins.path {
    path = ./.;
    name = "mcp-libvirt-vm-use-source";
    filter = path: type:
      let base = baseNameOf path;
      in base != "target" && base != ".git" && base != "result";
  };
  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [ pkgs.pkg-config ];
  buildInputs = [ pkgs.libvirt ];

  meta = {
    description = "MCP server that drives libvirt domains over SPICE";
    license = pkgs.lib.licenses.gpl3Only;
    mainProgram = "mcp-libvirt-vm-use";
  };
}
