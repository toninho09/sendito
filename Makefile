# Sendito — recurring commands (`make help` lists the targets)
# Lean output by default: targets that call cargo go through
# scripts/quiet.sh, which keeps the full output in target/<target>.log and prints
# a one-line summary on success. On failure it prints the head and the tail of the
# log (QUIET_FULL=1 prints all of it) and exits with an error.

# cargo jobs and test threads (heavy suite; see AGENTS.md)
J ?= 4
TT ?= 2
P :=
F :=
OUT ?= target/critic-bundles  # destination of make bundles

.DEFAULT_GOAL := help
.PHONY: help build check run servers fmt fmt-check lint deny ci test docs corpus corpus-update keymap snap media render bundles cov bench

help: ## List the available targets
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z0-9_-]+:.*?## / {printf "  \033[1m%-14s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

# --- build ---

build: ## Build the workspace (one-line summary)
	@bash scripts/quiet.sh build cargo build -j $(J) -q

check: ## Type-check only, quick feedback (P=<package> for one crate)
	@bash scripts/quiet.sh check cargo check -j $(J) -q $(if $(P),-p $(P),--workspace)

run: ## Run the binary: make run ARGS="req.http"
	cargo run -q -- $(ARGS)

servers: ## Run the echo servers for manual tests (HTTP, gRPC, WS, MCP; Ctrl+C stops)
	cargo run -q -p sendito-testkit --bin echo-servers

# --- CI gates (make ci runs everything in order) ---

fmt: ## Format the code (one-line summary)
	@bash scripts/quiet.sh fmt cargo fmt --all

fmt-check: ## Fail if any code is unformatted (one-line summary)
	@bash scripts/quiet.sh fmt-check cargo fmt --all --check

lint: ## Clippy with -D warnings, one line per diagnostic (P=<package> for one crate)
	@bash scripts/quiet.sh lint cargo clippy -j $(J) -q $(if $(P),-p $(P),--workspace) --all-targets --message-format short -- -D warnings

deny: ## cargo deny (licenses/advisories), one-line summary
	@bash scripts/quiet.sh deny cargo deny check --hide-inclusion-graph

ci: fmt-check lint deny test ## All gates: fmt + clippy + deny + tests

test: ## Full suite, one summary line (P=<package> for one crate, F=<name> filters)
	@bash scripts/quiet.sh test cargo test -j $(J) -q $(if $(P),-p $(P),--workspace) $(if $(F),$(F) )-- --test-threads=$(TT)
	@if [ -n "$(F)" ] && ! awk '/^test result:/ { ran += $$4 + $$8 } END { exit !(ran > 0) }' target/test.log; then \
	  echo "make test: no test matched F=$(F)" >&2; exit 1; fi

docs: ## Check that the paths, symbols and make targets the documentation cites exist
	@bash scripts/quiet.sh docs cargo test -j $(J) -q --test docs

# --- goldens and generated files ---

corpus: ## Run only the corpus test (exact bytes on the wire)
	@bash scripts/quiet.sh corpus cargo test -j $(J) -q --test corpus -- --test-threads=$(TT)

corpus-update: ## Rewrite the corpus goldens and summarize the diff
	@bash scripts/quiet.sh corpus env UPDATE_CORPUS=1 cargo test -j $(J) -q --test corpus -- --test-threads=$(TT)
	@git diff --stat -- tests/corpus/expected

keymap: ## Regenerate KEYMAP.md and report whether it changed
	@tmp=$$(mktemp) && cargo run -q -- --print-keymap > "$$tmp" && mv "$$tmp" KEYMAP.md || { rm -f "$$tmp"; exit 1; }
	@git diff --quiet -- KEYMAP.md && echo "KEYMAP.md: unchanged" || echo "KEYMAP.md: CHANGED — review the diff"

snap: ## Accept the insta snapshots (sendito-ui)
	@bash scripts/quiet.sh snap env INSTA_UPDATE=always cargo test -j $(J) -q -p sendito-ui --test snapshots
	@git status --short crates/ui/tests/snapshots

# --- media, review, coverage, bench (heavy: one at a time) ---

media: ## Generate screenshots/GIFs (--ignored test, single-thread)
	@bash scripts/quiet.sh media cargo test -j $(J) -q -p sendito-ui --test media -- --ignored --test-threads=1

render: ## Render the generated media (docs/media/render.py)
	python3 docs/media/render.py

bundles: ## Build the reviewer bundles (OUT= changes the destination)
	bash scripts/critic-bundles.sh $(OUT)

cov: ## Line coverage (all but the UI, by the whole test suite), 80% gate
	@bash scripts/quiet.sh cov cargo llvm-cov -j $(J) -q --workspace \
	  --ignore-filename-regex 'crates/(ui|testkit)/' \
	  --fail-under-lines 80 --summary-only -- --test-threads=$(TT)

bench: ## Benchmarks (needs hyperfine; see bench/run.sh)
	bash bench/run.sh
