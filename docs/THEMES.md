# Themes

Sendito ships 33 themes and reads your own from TOML files. Every theme with
RGB colors keeps each text color at least 4.5:1 (WCAG AA) on the background,
the bars and the selection.

| Theme | |
|---|---|
| `auto` | `daylight` on a light terminal (from `COLORFGBG`), else `dusk` |
| `dusk` | dark, cool blue accents |
| `daylight` | light, every text color at least 4.5:1 (WCAG AA) |
| `ember` | dark and warm: charcoal with amber and coral |
| `linen` | light and warm: paper with ink blue and teal |
| `contrast` | black and white with yellow focus, heavy borders on the focused pane |
| `ansi` | your terminal's own 16 colors |

These themes follow the palette of a popular editor theme. A color that is
not readable enough is a little lighter (dark themes) or darker (light themes)
than the original.

| Theme | |
|---|---|
| `dracula` | dark: purple, pink and cyan on blue-gray |
| `monokai` | dark: neon green, pink and yellow on olive black |
| `nord` | dark: arctic frost blues on polar night |
| `gruvbox` | dark: retro warm tones on brown-gray |
| `one-dark` | dark: soft blue, green and purple on slate |
| `catppuccin` | dark (Mocha): pastel mauve and blue on deep indigo |
| `tokyo-night` | dark: neon blue and magenta on midnight |
| `rose-pine` | dark: iris, rose and gold on muted plum |
| `night-owl` | dark: bright blue and lime on deep ocean |
| `kanagawa` | dark: ink blues and muted gold |
| `everforest` | dark: soft greens and earth tones on moss gray |
| `github-dark` | dark: blue links and green checks on near black |
| `solarized-dark` | dark: the Solarized accents on base03 |
| `solarized-light` | light: the Solarized accents on base3 |
| `gruvbox-light` | light: retro warm tones on cream |
| `catppuccin-latte` | light: mauve and blue on soft white |
| `github-light` | light: blue links and green checks on white |
| `one-light` | light: blue, purple and green on paper white |

Solid-color themes: a strong background, a selection bar in one solid color
and heavy borders on the focused pane.

| Theme | |
|---|---|
| `turbo` | DOS blue with yellow keys and a cyan selection bar |
| `phosphor` | a green CRT: green on black; errors and warnings keep a hue |
| `amber` | an amber CRT: amber on black; errors keep a red hue |
| `bauhaus` | light: cream paper, black lines, primary blue, red and yellow |
| `crimson` | deep wine with gold |
| `abyss` | deep teal with coral |
| `grove` | forest green with mustard |
| `grape` | deep purple with mint |

Pick one in Settings (`Ctrl+W ,`; on the Theme row ← → change the theme and
Space or Enter opens the list), with `:theme` (the list) or `:theme <name>` in
the command palette (the matching names are listed as you type), or with
`theme = "<name>"` under `[ui]` in the config. The list shows each theme as you
move to it: Enter keeps it, Esc goes back to the theme in use. Each name has its
kind after it (`dark`, `light`, `solid selection`, `yours`); type a kind to
filter by it. A swatch of the theme's colors on its own background ends
each row. `NO_COLOR=1` turns colors off
whatever the theme. Terminals without 24-bit color get the nearest of 256 colors.

## A theme for each environment

An environment file can name a theme, so a request to production does not
look like a request to a local server:

```toml
# .sendito/environments/prod.toml
theme = "crimson"

[variables]
base_url = "https://api.example.com"
```

While the environment is active, the app shows its theme and the top bar shows
the environment as a solid chip. An environment without `theme` shows the
`[ui] theme`. A name that no theme has (for example a theme of yours that a
teammate does not have) falls back to the `[ui] theme` with a warning.
`:theme` and Settings save `[ui] theme` and show it until the environment
changes. The Theme line of the environment editor (`Ctrl+W V`) sets the key.
It works as the Theme row of Settings: ← → change the theme, and Space or
Enter lists the themes. The screen shows the theme of the line while the line
has the focus; the theme in use comes back when it loses the focus.

## Your own themes

A theme is a TOML file in one of these directories:

* `themes/` next to the global config file, usually `~/.config/sendito/themes/`
  (it follows `XDG_CONFIG_HOME`);
* `<collection>/.sendito/themes/`, for one collection (it wins over a global theme
  with the same name).

```toml
# ~/.config/sendito/themes/harbor.toml
name = "harbor"          # optional; the file name otherwise
extends = "dusk"         # optional; a built-in theme or another theme of yours

[colors]
bg = "#0f1720"
accent = "#5ccfe6"
border_focus = "#5ccfe6"
get = "green"            # a color name
border = 240             # an xterm-256 index
```

Only the colors you set change; the rest come from `extends` (`dusk` when it is
left out). Derived colors (the second table below) are recomputed from your
`bg`, `fg` and other base colors unless you set them too, so changing `bg` alone
gives a consistent theme.

Colors are written as `"#rrggbb"`, `"#rgb"`, a name (`black`, `red`, `green`,
`yellow`, `blue`, `magenta`, `cyan`, `gray`, `dark-gray`, `light-red`, …, `white`,
`bright-*` is the same as `light-*`), an xterm-256 index (`0`–`255`) or `"reset"`
(the terminal's default).

To start from a complete file:

```sh
sendito theme print dusk > ~/.config/sendito/themes/mine.toml
sendito theme list --collection .   # every theme, where it comes from and any problem
```

Add `--collection <path>` to either command to also search that collection's
`.sendito/themes/` directory.

While you edit, `:theme reload` in the command palette reads the files again and
applies the change at once.

Problems never stop Sendito from starting: an unknown token, a value that is not a
color or an `extends` that loops is reported (status line, Settings, `sendito
theme list`) and skipped. Text colors that fall below a 4.5:1 contrast with the
background, the bars, the selection or alternate rows are reported too, so you can
fix them, but the theme is still used. A theme cannot reuse a built-in name.

## Color tokens

### Base

Every built-in theme sets these.

| Token | Used for |
|---|---|
| `bg` | Page background. |
| `fg` | Body text. |
| `dim` | Secondary text: labels, hints, placeholders. |
| `accent` | Keys, active tabs and other interactive accents. |
| `border` | Borders of panes without focus. |
| `border_focus` | Border of the focused pane. |
| `sel_bg` | Background of the selected row. |
| `sel_fg` | Text of the selected row. |
| `bar_bg` | Input fields, the top bar and the status line. |
| `ok` | Success: 2xx status, passed assertions, saved. |
| `warn` | Warnings: 4xx status, unsaved changes, a running request. |
| `err` | Errors: 5xx status, failed assertions, delete buttons. |
| `info` | Information: 3xx status, notes. |
| `get` | GET method. |
| `post` | POST method. |
| `put` | PUT method. |
| `patch` | PATCH method. |
| `delete` | DELETE method. |
| `other` | Other HTTP methods. |
| `grpc` | gRPC requests. |
| `json_key` | JSON object keys. |
| `json_str` | JSON strings. |
| `json_num` | JSON numbers. |
| `json_lit` | JSON true, false and null. |
| `var_ok` | A `{{variable}}` that is defined. |
| `var_missing` | A `{{variable}}` that nothing defines. |
| `search_bg` | Background of search matches. |

### Derived

Computed from the base colors unless a theme sets them.

| Token | Used for |
|---|---|
| `surface` | Pane background (derived: bg). |
| `surface_alt` | Alternate table rows and the current line (derived from bg and fg). |
| `overlay_bg` | Dialog background (derived from bg and fg). |
| `backdrop` | Background behind an open dialog (derived from bg). |
| `shadow` | Dialog shadow (derived from bg). |
| `gutter` | Line numbers (derived from dim and bg). |
| `keycap_bg` | Background of key labels in the hint row (derived from bg and fg). |
| `title` | Pane titles (derived: fg). |
| `scroll_thumb` | Scrollbar thumb (derived: dim). |
| `phase_dns` | Timing: DNS lookup (derived: info). |
| `phase_connect` | Timing: TCP connect (derived: accent). |
| `phase_tls` | Timing: TLS handshake (derived: patch). |
| `phase_wait` | Timing: waiting for the first byte (derived: warn). |
| `phase_download` | Timing: download (derived: ok). |

Chip backgrounds (methods and status codes in lists, the soft highlight of an
active tab or button) are a wash of the chip's color over `surface`, computed at
draw time.

## Icons

`icons = "nerd"` under `[ui]` (or Icons in Settings) switches to Nerd Font icons:
folders, the sidebar modes, the environment, Send and status icons, and rounded
chip ends. It needs a [Nerd Font](https://www.nerdfonts.com) in your terminal; the
default `unicode` set works with any font.
