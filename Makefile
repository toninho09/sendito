# Sendito — comandos de uso recorrente (`make help` lista os alvos)
# Saída enxuta por padrão: os alvos que chamam cargo passam por
# scripts/quiet.sh, que guarda a saída em target/<alvo>.log e imprime uma linha
# de resumo no sucesso (em falha, despeja o log e sai com erro).

# jobs do cargo e threads dos testes (suite pesada; ver AGENTS.md)
J ?= 4
TT ?= 2
OUT ?= target/critic-bundles  # destino de make bundles

.DEFAULT_GOAL := help
.PHONY: help build check run fmt fmt-check lint deny ci test corpus corpus-update keymap snap media render bundles cov bench

help: ## Lista os alvos disponíveis
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z0-9_-]+:.*?## / {printf "  \033[1m%-14s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

# --- build ---

build: ## Compila o workspace (resumo de 1 linha)
	@bash scripts/quiet.sh build cargo build -j $(J) -q

check: ## Só checa tipos, sem gerar binário (feedback rápido)
	@bash scripts/quiet.sh check cargo check -j $(J) -q --workspace

run: ## Roda o binário: make run ARGS="req.http"
	cargo run -q -- $(ARGS)

# --- gates do CI (make ci roda tudo em sequência) ---

fmt: ## Formata o código (resumo de 1 linha)
	@bash scripts/quiet.sh fmt cargo fmt --all

fmt-check: ## Falha se houver código não formatado (resumo de 1 linha)
	@bash scripts/quiet.sh fmt-check cargo fmt --all --check

lint: ## Clippy com -D warnings (1 linha por diagnóstico)
	@bash scripts/quiet.sh lint cargo clippy -j $(J) -q --workspace --all-targets --message-format short -- -D warnings

deny: ## cargo deny (licenças/advisories), resumo de 1 linha
	@bash scripts/quiet.sh deny cargo deny check --hide-inclusion-graph

ci: fmt-check lint deny test ## Todos os gates: fmt + clippy + deny + testes

test: ## Suite completa (uma linha de resumo; falha mostra o log todo)
	@bash scripts/quiet.sh test cargo test -j $(J) -q --workspace -- --test-threads=$(TT)

# --- goldens e arquivos gerados ---

corpus: ## Roda só o teste de corpus (bytes exatos no wire)
	@bash scripts/quiet.sh corpus cargo test -j $(J) -q --test corpus -- --test-threads=$(TT)

corpus-update: ## Regrava os goldens do corpus e resume o diff
	@bash scripts/quiet.sh corpus env UPDATE_CORPUS=1 cargo test -j $(J) -q --test corpus -- --test-threads=$(TT)
	@git diff --stat -- tests/corpus/expected

keymap: ## Regenera KEYMAP.md e avisa se mudou
	@tmp=$$(mktemp) && cargo run -q -- --print-keymap > "$$tmp" && mv "$$tmp" KEYMAP.md || { rm -f "$$tmp"; exit 1; }
	@git diff --quiet -- KEYMAP.md && echo "KEYMAP.md: inalterado" || echo "KEYMAP.md: MUDOU — revise o diff"

snap: ## Aceita os snapshots do insta (sendito-ui)
	@bash scripts/quiet.sh snap env INSTA_UPDATE=always cargo test -j $(J) -q -p sendito-ui --test snapshots
	@git status --short crates/ui/tests/snapshots

# --- mídia, revisão, cobertura, bench (pesados: um por vez) ---

media: ## Gera screenshots/GIFs (teste --ignored, single-thread)
	@bash scripts/quiet.sh media cargo test -j $(J) -q -p sendito-ui --test media -- --ignored --test-threads=1

render: ## Renderiza a mídia gerada (docs/media/render.py)
	python3 docs/media/render.py

bundles: ## Monta pacotes p/ revisores (OUT= muda o destino)
	bash scripts/critic-bundles.sh $(OUT)

cov: ## Cobertura de linhas (crates non-UI), gate de 80%
	@bash scripts/quiet.sh cov cargo llvm-cov -j $(J) -q -p sendito-core -p sendito-http -p sendito-grpc \
	  -p sendito-store -p sendito-import -p sendito-mcp \
	  --fail-under-lines 80 --summary-only -- --test-threads=$(TT)

bench: ## Benchmarks (precisa de hyperfine; ver bench/run.sh)
	bash bench/run.sh
