{
  description = "Crates and development tooling shared by Polylogue and Sinex";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.rust-overlay = {
    url = "github:oxalica/rust-overlay/4748ec2f5ed4a881474ed4c98aa71a5308cdac8d";
    inputs.nixpkgs.follows = "nixpkgs";
  };
  inputs.crane.url = "github:ipetkov/crane/eb35abda9f232cc6610b1d1e3200d15c49b7ac54";
  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      crane,
    }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
        config.allowUnfreePredicate = pkg: nixpkgs.lib.getName pkg == "timescaledb";
      };
      toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
      # Keep fixture data and browser assets alongside Rust sources.
      src = pkgs.lib.fileset.toSource {
        root = ./.;
        fileset = pkgs.lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./.cargo
          ./.config
          ./crates
          ./xtask
        ];
      };
      cargoArgs = {
        inherit src;
        pname = "polylogue-sinex-common";
        version = "0.1.0";
        CARGO_PROFILE = "dev";
        cargoExtraArgs = "--locked --workspace --all-features";
        nativeBuildInputs = [ pkgs.pkg-config ];
        buildInputs = [ pkgs.zstd ];
        strictDeps = true;
      };
      cargoArtifacts = craneLib.buildDepsOnly cargoArgs;
      postgres = pkgs.postgresql_18.withPackages (p: [
        p.timescaledb
        p.pgvector
      ]);
    in
    {
      formatter.${system} = pkgs.nixfmt;
      checks.${system} = {
        fmt = craneLib.cargoFmt {
          inherit (cargoArgs) src pname version;
          cargoExtraArgs = "--all";
        };
        clippy = craneLib.cargoClippy (
          cargoArgs
          // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- -D warnings";
            doInstallCargoArtifacts = false;
          }
        );
        nextest = craneLib.cargoNextest (
          cargoArgs
          // {
            inherit cargoArtifacts;
            # These groups start database services; the native xtask runs them.
            cargoNextestExtraArgs = "-E 'not (package(devkit) and (test(/^pg::/) or test(/^nats::/)))'";
            nativeBuildInputs = cargoArgs.nativeBuildInputs ++ [
              pkgs.git
              pkgs.dbus
            ];
            doInstallCargoArtifacts = false;
            preCheck = ''
              export XDG_STATE_HOME="$TMPDIR/state"
            '';
          }
        );
        web-shell =
          pkgs.runCommand "web-shell-check"
            {
              nativeBuildInputs = [
                pkgs.nodejs
                pkgs.typescript
                pkgs.esbuild
              ];
            }
            ''
              node ${src}/crates/web-shell/build.mjs --test
              touch "$out"
            '';
      };
      devShells.${system}.default = craneLib.devShell {
        checks = self.checks.${system};
        packages = [
          toolchain
          pkgs.git
          pkgs.cargo-nextest
          pkgs.cargo-llvm-cov
          pkgs.cargo-fuzz
          pkgs.cargo-deny
          pkgs.mold
          pkgs.sqlite
          pkgs.jq
          pkgs.gcc
          pkgs.pkg-config
          pkgs.zstd
          postgres
          pkgs.nats-server
          pkgs.dbus
          pkgs.nodejs
          pkgs.typescript
          pkgs.esbuild
        ];
        CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS = "-C link-arg=-fuse-ld=mold";
        RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
        LLVM_COV = "${toolchain}/lib/rustlib/${system}/bin/llvm-cov";
        LLVM_PROFDATA = "${toolchain}/lib/rustlib/${system}/bin/llvm-profdata";
      };
    };
}
