# Sendito — recurring commands (`make help` lists the targets)
# Lean output by default: targets that call cargo go through
# scripts/quiet.sh, which keeps the full output in target/<target>.log and prints
# a one-line summary on success (on failure, it dumps the log and exits with an error).

# cargo jobs and test threads (heavy suite; see AGENTS.md)
J ?= 4
TT ?= 2
OUT ?= target/critic-bundles  # destination of make bundles

.DEFAULT_GOAL := help
.PHONY: help build check run fmt fmt-check lint deny ci test corpus corpus-update keymap snap media render bundles cov bench

help: ## List the available targets
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z0-9_-]+:.*?## / {printf "  \033[1m%-14s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

# --- build ---

build: ## Build the workspace (one-line summary)
	@bash scripts/quiet.sh build cargo build -j $(J) -q

check: ## Only check types, without producing a binary (quick feedback)
	@bash scripts/quiet.sh check cargo check -j $(J) -q --workspace

run: ## Run the binary: make run ARGS="req.http"
	cargo run -q -- $(ARGS)

# --- CI gates (make ci runs everything in order) ---

fmt: ## Format the code (one-line summary)
	@bash scripts/quiet.sh fmt cargo fmt --all

fmt-check: ## Fail if any code is unformatted (one-line summary)
	@bash scripts/quiet.sh fmt-check cargo fmt --all --check

lint: ## Clippy with -D warnings (one line per diagnostic)
	@bash scripts/quiet.sh lint cargo clippy -j $(J) -q --workspace --all-targets --message-format short -- -D warnings

deny: ## cargo deny (licenses/advisories), one-line summary
	@bash scripts/quiet.sh deny cargo deny check --hide-inclusion-graph

ci: fmt-check lint deny test ## All gates: fmt + clippy + deny + tests

test: ## Full suite (one summary line; failure prints the whole log)
	@bash scripts/quiet.sh test cargo test -j $(J) -q --workspace -- --test-threads=$(TT)

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

cov: ## Line coverage (non-UI crates), 80% gate
	@bash scripts/quiet.sh cov cargo llvm-cov -j $(J) -q -p sendito-core -p sendito-http -p sendito-grpc \
	  -p sendito-store -p sendito-import -p sendito-mcp \
	  --fail-under-lines 80 --summary-only -- --test-threads=$(TT)

bench: ## Benchmarks (needs hyperfine; see bench/run.sh)
	bash bench/run.sh
