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
      formatter.${system} = pkgs.nixfmt-tree;
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
            # Native checks also cover services and filesystem ACL/xattr syscalls,
            # which the sealed builder cannot supply. Keep the PAX byte tests here.
            cargoNextestExtraArgs = ''
              -E 'not (
                (package(devkit) and (test(/^pg::/) or test(/^nats::/))) or
                (package(hashed-blobs) and binary(pax) and (
                  test(=gnu_tar::gnu_tar_generated_pax_sparse_versions_match_extracted_files) or
                  test(=gnu_tar::pax_members_match_gnu_tar_names_metadata_xattrs_acls_and_sparse_bytes)
                ))
              )'
            '';
            nativeBuildInputs = cargoArgs.nativeBuildInputs ++ [
              pkgs.git
              pkgs.dbus
            ];
            doInstallCargoArtifacts = false;
            TZDIR = "${pkgs.tzdata}/share/zoneinfo";
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
        TZDIR = "${pkgs.tzdata}/share/zoneinfo";
        RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
        LLVM_COV = "${toolchain}/lib/rustlib/${system}/bin/llvm-cov";
        LLVM_PROFDATA = "${toolchain}/lib/rustlib/${system}/bin/llvm-profdata";
        shellHook = ''
          if [ -d .githooks ] && git rev-parse --is-inside-work-tree >/dev/null 2>&1 \
            && ! git config --get core.hooksPath >/dev/null; then
            git config --local core.hooksPath .githooks
          fi
        '';
      };
    };
}
