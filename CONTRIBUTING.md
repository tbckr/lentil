# Contributing

Every change to Lentil follows these standards. What Lentil does is defined in the [PRD](docs/PRD.md), and how clients share a repository in the [convention spec](docs/SPEC.md).

| Area | Requirement |
| --- | --- |
| Testing | Finance logic is covered by unit tests, differential tests against the reference implementation and mutation tests; the mutation tests are the actual quality gate, and at least 80 % line coverage for `lentil-core` is a floor. Multi-writer sync tests simulate several clients pushing to one repository and fail on lost intents, duplicates or non-linear history; property-based tests cover replay. All run before every merge, alongside agent reviews. Test dependencies are dev-only and never ship. |
| Static analysis | Rust: clippy with warnings as errors and selected pedantic lints, rustfmt. Kotlin: Android Lint plus detekt or ktlint. A frontend linter only if a web UI is built. |
| Architecture | Traits only at I/O boundaries (Git, file system, clock, randomness, import sources); business logic such as replay, ID generation and validation stays in pure functions. |
| Dependencies | Every new direct dependency is justified in its pull request. cargo-deny checks licenses, duplicate versions and advisories. rustledger is pinned to an exact version and upgraded deliberately, guarded by the differential tests. Direct dependencies are upgraded on the project's own schedule; transitive ones change only when a direct dependency requires it or to fix a vulnerability that affects Lentil. |
| Vulnerability scanning | Daily osv-scanner over `Cargo.lock` and the Gradle lockfiles, on `main` and on the latest release; Grype on container images once there are any. A weekly CI run tests against the newest direct dependencies. No update bots; Dependabot only bumps the SHA-pinned GitHub Actions. |
| CI and hosting | GitHub, with GitHub Actions. Every action is pinned by commit SHA. |
| Releases | SemVer for releases. Compatibility between clients is governed by `spec_version`, not by the app version. Android additionally uses an increasing `versionCode`. Release artifacts are built with Rust's native tooling, cargo for each target in GitHub Actions. Checksums, SBOMs, signatures and the GitHub release are separate workflow steps. |
| Supply chain | An SBOM for every release artifact. CLI binaries are signed with cosign keyless signing through GitHub OIDC, so no long-lived signing key exists for them; the same applies to container images once there are any. The Android APK is built reproducibly, so F-Droid can publish the developer-signed APK. |
