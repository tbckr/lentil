# lentil

Lentil makes [Beancount](https://github.com/beancount/beancount/) practical for everyday use: a Git repository of standard Beancount files is the single source of truth, and several interchangeable clients work on it following a shared convention.

## Status

Lentil has a first, unreleased draft of v0.1: the `lentil` CLI (`init`, `import`, `status`, `check`) on a Rust core, and an Android triage app.

- [Product requirements](docs/PRD.md): principles, goals, requirements and release plan.
- [Convention spec](docs/SPEC.md): the rules every client follows.
- [Contributing](CONTRIBUTING.md): engineering standards.

## Install

The flake packages the `lentil` CLI for Linux (x86_64 and arm64) and macOS (arm64).

- Install it with `nix profile install github:tbckr/lentil`, or run it without installing with `nix run github:tbckr/lentil -- <command>`.
- On NixOS, add the flake as an input of the system configuration and its package to the system:

  ```nix
  {
    inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    inputs.lentil.url = "github:tbckr/lentil";

    outputs =
      { nixpkgs, lentil, ... }:
      {
        nixosConfigurations.example = nixpkgs.lib.nixosSystem {
          modules = [
            ./configuration.nix
            (
              { pkgs, ... }:
              {
                environment.systemPackages = [ lentil.packages.${pkgs.stdenv.hostPlatform.system}.default ];
              }
            )
          ];
        };
      };
  }
  ```

- With the overlay, the module sets `nixpkgs.overlays = [ lentil.overlays.default ];` and adds `pkgs.lentil` instead.

## Non-goals

Feature requests are filtered through these non-goals; the reasons are in the [PRD](docs/PRD.md#non-goals-for-v1).

- Budgets; investments with lots and cost basis; receipts from a document archive (after v1)
- Matching manual entries with later imports; third-party plugins and importers (after v1)
- Own web UI, desktop app and automatic bank access via FinTS (later)
- iOS; several people or households; end-to-end encrypted remote; commercial bank aggregators (open)
- Hosted service (never)

## License

GPL-3.0-only, see [LICENSE](LICENSE).
