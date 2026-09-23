{
  description = "mcp-libvirt-vm-use: MCP server for libvirt VMs over SPICE";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
  };

  outputs = inputs @ { self, flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" "aarch64-linux" ];
      perSystem = { pkgs, ... }:
        let
          mcp-libvirt-vm-use = pkgs.callPackage ./default.nix { inherit pkgs; };
        in {
          packages.default = mcp-libvirt-vm-use;
          packages.mcp-libvirt-vm-use = mcp-libvirt-vm-use;

          devShells.default = pkgs.mkShell {
            packages = [
              pkgs.rustc
              pkgs.cargo
              pkgs.pkg-config
              pkgs.libvirt
            ];
          };
        };
    };
}
