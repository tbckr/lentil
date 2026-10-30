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
          # Matches requires-python in pyproject.toml; bump both together.
          python = pkgs.python314;
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
              # upstream toolchain above, where coverage works. Drop this once
              # cache.nixos.org serves cargo-llvm-cov again.
              (pkgs.cargo-llvm-cov.overrideAttrs { doCheck = false; })
              pkgs.cargo-deny
              pkgs.cargo-machete
              pkgs.osv-scanner
              pkgs.nixfmt
              # Python Beancount comes through uv, pinned in uv.lock.
              pkgs.uv
            ];

            # uv only ever uses this Python and never downloads one. Python
            # stays off PATH, so its setup hook puts no PYTHONPATH in front of
            # uv's virtual environment.
            UV_PYTHON = "${python}/bin/python3";
            UV_PYTHON_DOWNLOADS = "never";
          };
        }
      );

      formatter = forAllSystems (pkgs: pkgs.nixfmt-tree);
    };
}
