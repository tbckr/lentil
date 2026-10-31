{
  description = "Lentil: development shell and CLI package";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Builds the CLI package.
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      crane,
      ...
    }:
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
              config = {
                # The Android SDK is unfree. Accept its license, and allow
                # only the SDK's own packages, not unfree software in general.
                android_sdk.accept_license = true;
                allowUnfreePredicate = pkg: (pkg.meta.homepage or null) == "https://developer.android.com/tools";
              };
            }
          )
        );

      # The Gradle build pins the Android toolchain in its version catalog;
      # the shell reads it from there.
      androidVersions =
        (builtins.fromTOML (builtins.readFile ./android/gradle/libs.versions.toml)).versions;
    in
    {
      devShells = forAllSystems (
        pkgs:
        let
          inherit (pkgs) lib;

          # Matches requires-python in pyproject.toml; bump both together.
          python = pkgs.python314;

          # The Android SDK's build tools exist only for x86_64 Linux among the
          # supported systems, so only that shell builds the app.
          withAndroid = pkgs.stdenv.hostPlatform.system == "x86_64-linux";
          jdk = pkgs.jdk21_headless;
          buildTools = androidVersions.android-buildTools;
          ndk = androidVersions.android-ndk;
          compileSdk = "${androidVersions.android-compileSdk}.${androidVersions.android-compileSdkMinor}";
          composeSdk =
            args:
            (pkgs.androidenv.composeAndroidPackages (
              {
                platformVersions = [ compileSdk ];
                buildToolsVersions = [ buildTools ];
                includeNDK = true;
                ndkVersions = [ ndk ];
                includeCmake = false;
                includeEmulator = false;
                includeSystemImages = false;
                toolsVersion = null;
              }
              // args
            )).androidsdk;
          # The emulator and a system image add more than a gigabyte, so only
          # the emulator shell has them. API 36 is the newest platform with an
          # image without Google services for x86_64.
          emulatorApi = "36";

          mkShell =
            androidSdk:
            let
              sdkRoot = "${androidSdk}/libexec/android-sdk";
            in
            pkgs.mkShell (
              {
                packages = [
                  # Rust is pinned only in rust-toolchain.toml.
                  (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
                  pkgs.cargo-nextest
                  pkgs.cargo-mutants
                  # Its test suite runs against nixpkgs' rustc, whose
                  # coverage-instrumented binaries crash on exit. The shell uses the
                  # upstream toolchain above, where coverage works.
                  # Drop this once cache.nixos.org serves cargo-llvm-cov again.
                  (pkgs.cargo-llvm-cov.overrideAttrs { doCheck = false; })
                  pkgs.cargo-deny
                  pkgs.cargo-machete
                  pkgs.osv-scanner
                  # Reads osv-scanner's report in scripts/scan.sh and cargo's
                  # metadata in scripts/deps.sh.
                  pkgs.jq
                  # Fetches the Maven metadata in scripts/deps.sh.
                  pkgs.curl
                  pkgs.nixfmt
                  # Checks the GitHub workflows.
                  pkgs.actionlint
                  # Python Beancount comes through uv, pinned in uv.lock.
                  pkgs.uv
                ]
                ++ lib.optionals withAndroid [
                  androidSdk
                  jdk
                  pkgs.cargo-ndk
                  # Builds the vendored OpenSSL for Android.
                  pkgs.perl
                  # Only to regenerate the Gradle wrapper; builds run ./gradlew.
                  pkgs.gradle
                  # Kotlin formatting and static analysis.
                  pkgs.ktfmt
                  pkgs.detekt
                ];

                # uv only ever uses this Python and never downloads one. Python
                # stays off PATH, so its setup hook puts no PYTHONPATH in front of
                # uv's virtual environment.
                UV_PYTHON = "${python}/bin/python3";
                UV_PYTHON_DOWNLOADS = "never";
              }
              // lib.optionalAttrs withAndroid {
                ANDROID_HOME = sdkRoot;
                ANDROID_SDK_ROOT = sdkRoot;
                ANDROID_NDK_HOME = "${sdkRoot}/ndk/${ndk}";
                JAVA_HOME = jdk.home;
                # The aapt2 that the Android Gradle Plugin fetches from Maven does
                # not run on NixOS; use the SDK's own.
                GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${sdkRoot}/build-tools/${buildTools}/aapt2";
              }
            );
        in
        {
          default = mkShell (composeSdk { });
        }
        // lib.optionalAttrs withAndroid {
          # For UI tests and manual runs: `nix develop .#emulator`.
          emulator = mkShell (composeSdk {
            platformVersions = [
              compileSdk
              emulatorApi
            ];
            includeEmulator = true;
            includeSystemImages = true;
            systemImageTypes = [ "default" ];
            abiVersions = [ "x86_64" ];
          });
        }
      );

      packages = forAllSystems (
        pkgs:
        let
          inherit (pkgs) lib;
          # The Rust version of rust-toolchain.toml without the development
          # shell's components and Android targets, so installing the package
          # downloads only what the build needs. The shell already holds these
          # components.
          craneLib = (crane.mkLib pkgs).overrideToolchain (
            p:
            p.rust-bin.fromRustupToolchain {
              inherit ((lib.importTOML ./rust-toolchain.toml).toolchain) channel;
              profile = "minimal";
            }
          );
          commonArgs = {
            # Only the workspace, so changes elsewhere do not rebuild the CLI.
            src = lib.fileset.toSource {
              root = ./.;
              fileset = lib.fileset.unions [
                ./Cargo.toml
                ./Cargo.lock
                (craneLib.fileset.commonCargoSources ./crates)
              ];
            };
            pname = "lentil";
            version = (lib.importTOML ./Cargo.toml).workspace.package.version;
            strictDeps = true;
            cargoExtraArgs = "--locked -p lentil-cli";
            # Builds the vendored OpenSSL.
            nativeBuildInputs = [ pkgs.perl ];
            # CI's development shell jobs run the tests with the same toolchain.
            doCheck = false;
          };
          lentil = craneLib.buildPackage (
            commonArgs
            // {
              cargoArtifacts = craneLib.buildDepsOnly commonArgs;
              # Fails the build if the binary does not start.
              nativeInstallCheckInputs = [ pkgs.versionCheckHook ];
              doInstallCheck = true;
              meta = {
                description = "Everyday Beancount on a Git repository";
                homepage = "https://github.com/tbckr/lentil";
                license = lib.licenses.gpl3Only;
                mainProgram = "lentil";
              };
            }
          );
        in
        {
          inherit lentil;
          default = lentil;
        }
      );

      # Adds pkgs.lentil for NixOS configurations. It hands out the package
      # above, built against this flake's nixpkgs from flake.lock.
      overlays.default = final: _prev: {
        lentil = self.packages.${final.stdenv.hostPlatform.system}.default;
      };

      # CI's `nix flake check` builds the package through this.
      checks = forAllSystems (pkgs: {
        inherit (self.packages.${pkgs.stdenv.hostPlatform.system}) lentil;
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt-tree);
    };
}
