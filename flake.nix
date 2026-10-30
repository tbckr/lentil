{
  description = "Lentil development shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );
    in
    {
      devShells = forAllSystems (
        pkgs:
        let
          python = pkgs.python3;
        in
        {
          default = pkgs.mkShell {
            packages = [
              # Rust is pinned only in rust-toolchain.toml.
              (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
              pkgs.cargo-nextest
              pkgs.cargo-mutants
              # Its test suite runs against nixpkgs' rustc, whose
              # coverage-instrumented binaries crash on exit. The shell uses the
              # upstream toolchain above, where coverage works.
              (pkgs.cargo-llvm-cov.overrideAttrs { doCheck = false; })
              pkgs.cargo-deny
              pkgs.cargo-machete
              pkgs.osv-scanner
              pkgs.nixfmt
              # Python Beancount comes through uv, pinned in uv.lock.
              python
              pkgs.uv
            ];

            # uv only ever uses this Python and never downloads one.
            UV_PYTHON = "${python}/bin/python3";
            UV_PYTHON_DOWNLOADS = "never";
          };
        }
      );

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
