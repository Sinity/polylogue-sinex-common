{
  description = "Crates and development tooling shared by Polylogue and Sinex";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = [ pkgs.cargo pkgs.cargo-nextest pkgs.rustc pkgs.rustfmt pkgs.clippy pkgs.gcc pkgs.pkg-config pkgs.zstd pkgs.postgresql_18 pkgs.nats-server pkgs.dbus pkgs.nodejs pkgs.typescript pkgs.esbuild ];
      };
    };
}
