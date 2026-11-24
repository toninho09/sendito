# Documentation

Start with the [README](../README.md) for what Sendito is, how to install it and
a gallery of the main flows. Everything else is here.

| Document | What it answers | Read it when |
|---|---|---|
| [README.md](../README.md) | What it is, install, quick start, GIF gallery | First contact |
| [FORMAT.md](../FORMAT.md) | Request, environment and config file schema; the exact wire rules | You write or debug a request file |
| [KEYMAP.md](../KEYMAP.md) | Every key and `:command` (generated) | You want to drive the UI |
| [CLI.md](CLI.md) | Every subcommand, flag, exit code and the JSON report | You script Sendito |
| [FEATURES.md](FEATURES.md) | Every feature, grouped by area | You want the complete picture |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Crates, the send pipeline, tests, changing behaviour | You work on the code |
| [MAP.md](MAP.md) | Feature → file and symbol index, tests per feature | You need to find where a feature lives |
| [THEMES.md](THEMES.md) | Theme format, tokens, icons | You want your own theme |
| [DEBT.md](DEBT.md) | Known limitations and how to remove them | Something is missing |
| [media/README.md](media/README.md) | How screenshots and GIFs are produced | You add or refresh media |
| [vhs/README.md](vhs/README.md) | Optional real-terminal recordings | You want tty captures |
| [reports/](reports/) | Coverage, performance, UX and correctness reviews | You want the evidence |
| [PLAN.md](PLAN.md) | The original specification (historical) | You want the project's origin |
| [CHANGELOG.md](../CHANGELOG.md) | What changed in each release | You upgrade |
| [AGENTS.md](../AGENTS.md) | Rules and commands for automation contributors | You use an AI agent |

## Generated files

Some documents are produced from the code. Do not hand-edit them; regenerate:

| File | Generator |
|---|---|
| [KEYMAP.md](../KEYMAP.md) | `make keymap` (`sendito --print-keymap`); a unit test checks it is current |
| `docs/media/screenshots/*.png`, `docs/media/recordings/*.gif` | `make media && make render` |
| `crates/ui/tests/snapshots/*.snap` | `make snap` |
| `tests/corpus/expected/*.wire` | `make corpus-update` |

## Reading order

* **New user**: README → FEATURES → KEYMAP.
* **Integrating in CI**: README (quick start) → CLI → FORMAT (`[http]`,
  `[oauth]`, `[mcp]`).
* **Contributor**: ARCHITECTURE → MAP → AGENTS.md → FORMAT → DEBT.
