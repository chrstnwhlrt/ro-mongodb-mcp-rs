{
  description = "ro-mongodb-mcp-rs — read-only MongoDB MCP server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    crane.url = "github:ipetkov/crane";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, crane, rust-overlay, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        # Latest stable Rust (edition 2024 requires >= 1.85)
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"      # For rust-analyzer
            "rust-analyzer" # LSP
            "clippy"        # Linter
            "rustfmt"       # Formatter
          ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Common arguments shared between build and checks
        commonArgs = {
          src = craneLib.cleanCargoSource ./.;
          strictDeps = true;

          # Both mongodb and kube use rustls (pure Rust TLS) — no native TLS deps needed.
          # Only system CA certificates are needed at runtime (via rustls-native-certs).
          buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
            pkgs.libiconv
            pkgs.darwin.apple_sdk.frameworks.Security
            pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
          ];
        };

        # Build only the cargo dependencies (for caching between builds)
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # The actual package
        crate = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
        });

      in
      {
        # `nix build`
        packages = {
          default = crate;
          ro-mongodb-mcp-rs = crate;
        };

        # `nix flake check`
        checks = {
          inherit crate;

          clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });

          fmt = craneLib.cargoFmt {
            src = craneLib.cleanCargoSource ./.;
          };

          tests = craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
          });
        };

        # `nix develop`
        devShells.default = craneLib.devShell {
          checks = self.checks.${system};

          packages = [
            pkgs.cargo-machete  # Unused dependency detection
            pkgs.cargo-outdated # Outdated dependency detection
            pkgs.cargo-nextest  # Test runner
          ];

          # Ensure rustls-native-certs can find system CA certificates
          env.SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
        };

        # `nix run`
        apps.default = flake-utils.lib.mkApp {
          drv = crate;
        };
      }
    );
}
