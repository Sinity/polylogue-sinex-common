{
  description = "Crates and development tooling shared by Polylogue and Sinex";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.rust-overlay = {
    url = "github:oxalica/rust-overlay/4748ec2f5ed4a881474ed4c98aa71a5308cdac8d";
    inputs.nixpkgs.follows = "nixpkgs";
  };
  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };
      toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        packages = [
          toolchain
          pkgs.git
          pkgs.cargo-nextest
          pkgs.gcc
          pkgs.pkg-config
          pkgs.zstd
          (pkgs.postgresql_18.withPackages (p: [ p.pgvector ]))
          pkgs.nats-server
          pkgs.dbus
          pkgs.nodejs
          pkgs.typescript
          pkgs.esbuild
        ];
        RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
      };
    };
}
