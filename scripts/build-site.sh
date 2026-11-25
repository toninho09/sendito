#!/bin/sh
# Build the documentation site. Copies the public documents into book/src,
# redirects the links to repository-only files to the repository, and runs mdbook.
# Run "make site" or "make site-serve" from the repository root.

set -eu

public_docs="
README.md
FORMAT.md
KEYMAP.md
CHANGELOG.md
docs/README.md
docs/GUIDES.md
docs/FEATURES.md
docs/CLI.md
docs/THEMES.md
docs/TROUBLESHOOTING.md
docs/ARCHITECTURE.md
docs/MAP.md
docs/DEBT.md
"

blob='https://github.com/toninho09/sendito/blob/main'

root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/book/src"

mkdir -p "$src/docs/media/screenshots" "$src/docs/media/recordings"
cp "$root/docs/media/"*.png "$src/docs/media/"
cp "$root/docs/media/screenshots/"*.png "$src/docs/media/screenshots/"
cp "$root/docs/media/recordings/"*.gif "$src/docs/media/recordings/"

# Documents that stay out of the site link to the repository.
# The README rule runs last: mdbook serves each README.md as index.html.
for file in $public_docs; do
  sed -E \
    -e "s#\]\((\.\./)?AGENTS\.md#]($blob/AGENTS.md#g" \
    -e "s#\]\((LICENSE-[^)#]+)#]($blob/\\1#g" \
    -e "s#\]\((docs/)?PLAN\.md#]($blob/docs/PLAN.md#g" \
    -e "s#\]\((docs/)?reports/#]($blob/docs/reports/#g" \
    -e "s#\]\((docs/)?(media|vhs)/README\.md#]($blob/docs/\\2/README.md#g" \
    -e "s#\]\((\.\./)?README\.md#](\\1index.md#g" \
    -e "s#\]\((docs/)README\.md#](\\1index.md#g" \
    "$root/$file" > "$src/$file"
done

mdbook build "$root/book"
