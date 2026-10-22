# lentil

Lentil makes [Beancount](https://github.com/beancount/beancount/) practical for everyday use: a Git repository of standard Beancount files is the single source of truth, and several interchangeable clients work on it following a shared convention.

## Status

Lentil is in the design phase; there is no code yet.

- [Product requirements](docs/PRD.md): principles, goals, requirements and release plan.
- [Convention spec](docs/SPEC.md): the rules every client follows.
- [Contributing](CONTRIBUTING.md): engineering standards.

## Non-goals

Feature requests are filtered through these non-goals; the reasons are in the [PRD](docs/PRD.md#non-goals-for-v1).

- Budgets; investments with lots and cost basis; receipts from a document archive (after v1)
- Matching manual entries with later imports; third-party plugins and importers (after v1)
- Own web UI, desktop app and automatic bank access via FinTS (later)
- iOS; several people or households; end-to-end encrypted remote; commercial bank aggregators (open)
- Hosted service (never)

## License

GPL-3.0-only, see [LICENSE](LICENSE).
