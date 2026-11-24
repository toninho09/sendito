# Media

The screenshots and GIFs used in [README](../README.md) and
[FEATURES](../FEATURES.md) are rendered from the application's **own styled
frames**, not captured from a terminal. They therefore always match the real UI
and can be regenerated deterministically.

## Pipeline

```text
crates/ui/tests/media.rs        drives the app with synthetic key/mouse events
  → target/media/frames/*.json  per-cell screen dumps + captions + durations
  → docs/media/render.py        rasterizes them with real fonts
  → docs/media/screenshots/     <WxH>-<theme>[-<variant>]-<caption>.png
    docs/media/recordings/      NN-<flow>.gif (and NN-<flow>/*.png key frames)
```

```sh
make media    # cargo test -p sendito-ui --test media -- --ignored (single-threaded)
make render   # python3 docs/media/render.py
```

`make media` starts the HTTP and gRPC echo servers in-process, so no external
server is needed. It is a heavy job: do not run it alongside the workspace tests,
benchmarks or coverage.

## Requirements

* **Python 3 + Pillow** (`from PIL import Image, ImageDraw, ImageFont`).
* **DejaVu Sans Mono** (regular, bold, oblique, bold-oblique).
* Optional **Nerd Font Mono** (`SymbolsNerdFontMono-Regular.ttf`,
  `MesloLGSNerdFontMono-Regular.ttf` or any `*NerdFontMono-Regular.ttf`) for the
  private-use icons in the `dusk-nerd` screenshots; without it the mono font is
  used.

`render.py` searches the usual font directories on Linux and macOS
(`/usr/share/fonts/…`, `/Library/Fonts`, `/System/Library/Fonts`,
`~/.local/share/fonts`). Box-drawing characters (U+2500–257F), block elements
(U+2580–259F) and braille (U+2800–28FF) are drawn as geometry, so borders join
without gaps whatever the font's line height.

## Naming

| Pattern | Meaning |
|---|---|
| `screenshots/<WxH>-<theme>-<caption>.png` | e.g. `200x60-dusk-palette.png` |
| `screenshots/<WxH>-<theme>-<variant>-<caption>.png` | e.g. `120x40-dusk-nerd-palette.png` |
| `recordings/NN-<flow>.gif` | e.g. `09-history-resend.gif` |
| `recordings/NN-<flow>/<NN>.png` | key frames for reviewers that cannot play GIFs (gitignored) |

The tested sizes are **120×40** and **200×60**. Themes are `dusk`, `daylight`,
`ember`, `linen`, `contrast`, `ansi`, plus the `dusk-nerd` variant. Captions come
from the `shot` calls in `media.rs`: `welcome`, `request-body-and-response`,
`response-focused`, `timing`, `grpc-unary`, `palette`, `help`, `environments`.

`hero.png` is the only hand-made asset; it is not produced by this pipeline.

## Adding or changing a screenshot / recording

1. Edit `crates/ui/tests/media.rs`: `Recorder::new("<stem>")` then `shot(...)`
   for each frame (`ms` is how long it shows in the GIF). Use `type_slow` to emit
   a frame every few keystrokes.
   * A stem starting with `screens-` produces still screenshots; anything else
     produces a GIF plus its key frames.
2. `make media && make render`.
3. Reference the file from the docs with a relative path and meaningful alt text.
4. GIF timing: each frame shows for at least 400 ms and the last frame for at
   least 2500 ms (set in `render.py`). Light themes (`daylight`, `linen`) get a
   light background and a blue caption strip; dark themes a yellow one.

## Real-terminal recordings (optional)

`docs/vhs/*.tape` are [VHS](https://github.com/charmbracelet/vhs) scripts for
capturing the real terminal. They are a separate, optional path and the
committed GIFs do **not** come from them; see [vhs/README.md](../vhs/README.md).

## Reviewer bundles

`make bundles` assembles the screenshots and the recording key frames into
`target/critic-bundles/ux/` for the independent UX review described in
[docs/reports/ux-critic.md](../reports/ux-critic.md). That is why the
`recordings/*/*.png` frames exist even though they are gitignored.
