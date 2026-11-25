# Sendito key map

Generated from the key table in `crates/ui/src/keymap.rs` (`sendito --print-keymap`).
The in-app help (F1) shows the same table, including your overrides.

Conventional mode is the default. Vim mode is enabled in Settings (`Ctrl+W ,`) or with
`[ui] vim_mode = true`. `Ctrl+W x` means: press Ctrl+W, release, then press x.

Ctrl+Enter needs a terminal that reports it (kitty, Alacritty, WezTerm, foot, Ghostty,
iTerm2 with CSI u). F5 and `Ctrl+W e` work everywhere.

When a key is bound both in the focused pane's section and in Global, the pane's
binding wins: in vim mode `Ctrl+B` pages up in the response viewer and toggles the
sidebar elsewhere (`Ctrl+W b` toggles it everywhere).


## Global

| Action | Conventional | Vim mode | Description |
|---|---|---|---|
| `send` | `Ctrl+Enter`, `F5`, `Ctrl+W e` | `Ctrl+Enter`, `F5`, `Ctrl+W e` | Send the request (or start the gRPC call) |
| `cancel` | `Ctrl+W x` | `Ctrl+W x` | Cancel the running request or stream |
| `end_stream` | `Ctrl+W d` | `Ctrl+W d` | Finish sending on a client/bidi stream (half-close) |
| `watch` | `Ctrl+W m` | `Ctrl+W m` | Re-send the active request on an interval (watch) |
| `save` | `Ctrl+S`, `Ctrl+W s` | `Ctrl+S`, `Ctrl+W s` | Save the request to its file (asks for a name if new) |
| `quit` | `Ctrl+W q`, `Ctrl+Q` | `Ctrl+W q`, `Ctrl+Q` | Quit Sendito |
| `new_request` | `Ctrl+N`, `Ctrl+W t` | `Ctrl+N`, `Ctrl+W t` | Open a new unsaved HTTP request tab |
| `new_grpc_request` | `Ctrl+W g` | `Ctrl+W g` | Open a new unsaved gRPC request tab |
| `new_ws_request` | `Ctrl+W y` | `Ctrl+W y` | Open a new unsaved WebSocket request tab |
| `new_sse_request` | `Ctrl+W E` | `Ctrl+W E` | Open a new unsaved SSE request tab |
| `new_mqtt_request` | `Ctrl+W M` | `Ctrl+W M` | Open a new unsaved MQTT request tab |
| `close_tab` | `Ctrl+W c` | `Ctrl+W c` | Close the current tab |
| `next_tab` | `Ctrl+W n`, `Ctrl+PageDown` | `Ctrl+W n`, `Ctrl+PageDown` | Next open request tab |
| `prev_tab` | `Ctrl+W p`, `Ctrl+PageUp` | `Ctrl+W p`, `Ctrl+PageUp` | Previous open request tab |
| `toggle_pin` | `Ctrl+W P` | `Ctrl+W P` | Pin or unpin the current tab (pinned tabs stay first) |
| `toggle_favorite` | `Ctrl+W F` | `Ctrl+W F` | Add or remove the request from the favorites |
| `focus_left` | `Ctrl+W h`, `Ctrl+W Left` | `Ctrl+W h`, `Ctrl+W Left` | Focus the pane to the left |
| `focus_down` | `Ctrl+W j`, `Ctrl+W Down` | `Ctrl+W j`, `Ctrl+W Down` | Focus the pane below |
| `focus_up` | `Ctrl+W k`, `Ctrl+W Up` | `Ctrl+W k`, `Ctrl+W Up` | Focus the pane above |
| `focus_right` | `Ctrl+W l`, `Ctrl+W Right` | `Ctrl+W l`, `Ctrl+W Right` | Focus the pane to the right |
| `focus_next` | `Tab`, `Ctrl+W w` | `Tab`, `Ctrl+W w` | Move focus to the next field or pane |
| `focus_prev` | `Shift+Tab`, `Ctrl+W W` | `Shift+Tab`, `Ctrl+W W` | Move focus to the previous field or pane |
| `next_subtab` | `Ctrl+W ]` | `Ctrl+W ]` | Next tab of the focused pane (e.g. Params → Headers) |
| `prev_subtab` | `Ctrl+W [` | `Ctrl+W [` | Previous tab of the focused pane |
| `subtab_1` | `Alt+1` | `Alt+1` | Jump to tab 1 of the focused pane |
| `subtab_2` | `Alt+2` | `Alt+2` | Jump to tab 2 of the focused pane |
| `subtab_3` | `Alt+3` | `Alt+3` | Jump to tab 3 of the focused pane |
| `subtab_4` | `Alt+4` | `Alt+4` | Jump to tab 4 of the focused pane |
| `subtab_5` | `Alt+5` | `Alt+5` | Jump to tab 5 of the focused pane |
| `subtab_6` | `Alt+6` | `Alt+6` | Jump to tab 6 of the focused pane |
| `palette` | `Ctrl+P` | `Ctrl+P` | Open the command palette |
| `help` | `F1`, `Ctrl+W ?` | `F1`, `Ctrl+W ?` | Show all keys and actions |
| `settings` | `Ctrl+W ,` | `Ctrl+W ,` | Open settings |
| `select_env` | `F4`, `Ctrl+W v` | `F4`, `Ctrl+W v` | Choose the active environment |
| `manage_envs` | `Ctrl+W V` | `Ctrl+W V` | Edit environments and their variables |
| `manage_mcp_servers` | `Ctrl+W S` | `Ctrl+W S` | Edit the MCP servers in the project config |
| `history` | `F7`, `Ctrl+W r` | `F7`, `Ctrl+W r` | Show request history in the sidebar (toggle) |
| `toggle_sidebar` | `Ctrl+B`, `Ctrl+W b` | `Ctrl+B`, `Ctrl+W b` | Show or hide the sidebar |
| `import` | `Ctrl+W i` | `Ctrl+W i` | Import a collection or a curl command |
| `export_curl` | `Ctrl+W u` | `Ctrl+W u` | Copy the request as curl (secrets stay {{placeholders}}) |
| `format_body` | `Ctrl+W f` | `Ctrl+W f` | Pretty-print the JSON body / message |
| `zoom` | `Ctrl+W z` | `Ctrl+W z` | Maximize or restore the focused pane |
| `grow_sidebar` | `Ctrl+W >` | `Ctrl+W >` | Widen the sidebar |
| `shrink_sidebar` | `Ctrl+W <` | `Ctrl+W <` | Narrow the sidebar |
| `grow_request` | `Ctrl+W +` | `Ctrl+W +` | Give the request editor more height |
| `shrink_request` | `Ctrl+W -` | `Ctrl+W -` | Give the response more height |
| `complete` | `Ctrl+Space` | `Ctrl+Space` | Show completion suggestions at the cursor |

## Sidebar tree

| Action | Conventional | Vim mode | Description |
|---|---|---|---|
| `tree_open` | `Enter` | `Enter` | Open the selected request / toggle the folder |
| `tree_expand` | `Right` | `l`, `Right` | Expand folder |
| `tree_collapse` | `Left` | `h`, `Left` | Collapse folder or go to parent |
| `tree_new_request` | `n` | `n` | New request in the selected folder |
| `tree_new_folder` | `N` | `N` | New folder in the selected folder |
| `tree_rename` | `r`, `F2` | `r`, `F2` | Rename the selected item |
| `tree_folder_settings` | `s` | `s` | Edit the settings of the selected folder (headers, auth, variables and scripts its requests inherit) |
| `tree_duplicate` | `d` | `d` | Duplicate the selected item |
| `tree_delete` | `Delete`, `x` | `Delete`, `x` | Delete the selected item (asks first) |
| `tree_move` | `m` | `m` | Move the selected item to another folder |
| `tree_filter` | `/` | `/` | Filter the tree by name |
| `toggle_favorite` | `f` | `f` | Add or remove the request from the favorites |
| `up` | `Up` | `k`, `Up` | Move up |
| `down` | `Down` | `j`, `Down` | Move down |
| `page_up` | `PageUp` | `PageUp` | Page up |
| `page_down` | `PageDown` | `PageDown` | Page down |
| `top` | `Home` | `g g`, `Home` | Go to the first line |
| `bottom` | `End` | `G`, `End` | Go to the last line |

## Text editing (conventional mode)

| Action | Conventional | Vim mode | Description |
|---|---|---|---|
| `undo` | `Ctrl+Z` | — | Undo |
| `redo` | `Ctrl+Y` | — | Redo |
| `select_all` | `Ctrl+A` | — | Select all text |
| `copy` | `Ctrl+C` | — | Copy selection (whole field if nothing selected) |
| `cut` | `Ctrl+X` | — | Cut selection |
| `paste` | `Ctrl+V` | — | Paste |

## Params / headers / form tables

| Action | Conventional | Vim mode | Description |
|---|---|---|---|
| `next_cell` | `Enter` | `Enter` | Edit the next cell (name → value → next row) |
| `toggle_row` | `Ctrl+T` | `Ctrl+T` | Enable or disable the row |
| `delete_row` | `Ctrl+D` | `Ctrl+D` | Delete the row |

## Response viewer

| Action | Conventional | Vim mode | Description |
|---|---|---|---|
| `up` | `Up` | `k`, `Up` | Move up |
| `down` | `Down` | `j`, `Down` | Move down |
| `page_up` | `PageUp` | `Ctrl+B`, `Ctrl+U`, `PageUp` | Page up |
| `page_down` | `PageDown` | `Ctrl+F`, `Ctrl+D`, `PageDown` | Page down |
| `top` | `Home`, `Ctrl+Home` | `g g`, `Home` | Go to the first line |
| `bottom` | `End`, `Ctrl+End` | `G`, `End` | Go to the last line |
| `scroll_left` | `Left` | `h`, `Left` | Scroll left |
| `scroll_right` | `Right` | `l`, `Right` | Scroll right |
| `view_pretty` | `p` | `p` | Body view: pretty (formatted and highlighted) |
| `view_raw` | `r` | `r` | Body view: raw text |
| `view_hex` | `x` | `x` | Body view: hex dump |
| `search` | `/`, `Ctrl+F` | `/`, `Ctrl+F` | Search in the response |
| `filter` | `j` | `J` | Filter the response body with a jq expression |
| `filter_here` | `f` | `f` | Filter the body to the JSON value at the cursor |
| `copy_path` | `Y` | `Y` | Copy the jq path of the JSON value at the cursor |
| `prev_message` | `[` | `[` | Show the previous message of a stream |
| `next_message` | `]` | `]` | Show the next message of a stream (the last one follows new ones) |
| `search_next` | `n`, `F3` | `n`, `F3` | Next search match |
| `search_prev` | `N`, `Shift+F3` | `N`, `Shift+F3` | Previous search match |
| `fold_toggle` | `Enter`, `Space` | `z a`, `Enter`, `Space` | Fold or unfold the block at the cursor |
| `fold_all` | `-` | `z M`, `-` | Fold all blocks |
| `unfold_all` | `+`, `=` | `z R`, `+`, `=` | Unfold all blocks |
| `toggle_wrap` | `w` | `w` | Wrap long lines on/off |
| `copy_body` | `y`, `Ctrl+C` | `y y`, `Ctrl+C` | Copy the response body to the clipboard |
| `save_body` | `s` | `s` | Save the response body to a file |

## Fixed keys

These are handled by the widgets themselves and cannot be remapped.

| Where | Keys | What |
|---|---|---|
| Text editing | Arrows, Home, End, PageUp, PageDown | Move the cursor |
| Text editing | Shift+Arrows, Shift+Home/End | Extend the selection |
| Text editing | Ctrl+Left / Ctrl+Right | Move by word |
| Text editing | Backspace / Delete | Delete before / after the cursor |
| Text editing | Tab (body editor) | Insert two spaces (Esc first to leave the editor) |
| Text editing | Esc | Leave the editor (focus goes to the tab bar) |
| Text editing | Terminal paste (e.g. Ctrl+Shift+V / Cmd+V) | Paste from the system clipboard |
| Tab bars | Left / Right | Switch tab when a tab bar is focused |
| Tab bars | Enter / Down | Go into the tab's content |
| Method / pickers | Enter or Space | Open the picker; type to filter, Enter to choose, Esc to close |
| Dialogs | Enter / Esc | Input dialogs: Enter confirms from any field (Ctrl+S too); Tab moves between fields; on a choice row Left/Right change the value and Space opens its list. Question dialogs: Enter presses the focused button; the letter in a button (y, s, d) presses it directly |
| History (sidebar) | Enter | Open a copy of the entry in a new tab |
| History (sidebar) | Ctrl+Enter / F5 | Resend the entry (opens a copy and sends it) |
| History (sidebar) | s | Save the entry to the tree |
| History (sidebar) | F7 | Back to the tree |
| Tables | Space (on a ‹choice› cell: multipart Kind, env Source) | Change the value |
| Environment editor | Space (Source cell) | Change where the value lives: plain, secrets (.sendito/secrets.toml), keychain, file |
| Environment editor | Ctrl+S / Ctrl+N / Ctrl+PageUp, Ctrl+PageDown | Save / new environment / previous, next environment |
| Environment editor | Enter / Ctrl+D / Esc | Next cell / delete the row / close |
| Import dialog | F5 or Ctrl+S / Esc | Import the pasted curl command or collection / cancel |
| Settings | Up, Down / Left, Right or Space / Esc | Choose a setting / change its value / close |
| Report dialogs | y / Up, Down, PageUp, PageDown / Esc | Copy / scroll / close |
| MCP server / tool field | F2 | Choose a configured server / discover and choose a tool |
| Command palette | :<command> | Type a command (listed under Commands) or pick from the list |
| Vim mode | i a I A o O | Enter insert mode |
| Vim mode | Esc | Back to normal mode |
| Vim mode | h j k l  w b e  0 ^ $  gg G | Motions (normal and visual) |
| Vim mode | x  dd  D  cc  C  yy  p  P  u  Ctrl+R | Edit, yank, put, undo, redo |
| Vim mode | d c y + a motion (dw, cw, d$, yw, dG …) | Delete / change / yank over the motion |
| Vim mode | v  V | Visual / visual line; then d, y, c |
| Vim mode | : | Command palette in command mode |
| Vim mode | / | Search (response) or filter (tree) |
| Mouse | Click | Focus panes, select rows, switch and close tabs, press buttons |
| Mouse | Double-click | Open a request from the tree |
| Mouse | Drag a tree item onto a folder | Move it |
| Mouse | Drag a pane border | Resize the sidebar or the request/response split |
| Mouse | Wheel | Scroll the pane under the pointer |

## Symbols

| Symbol | Meaning |
|---|---|
| ● | unsaved changes (tab, status line) |
| ⠋ on a tab (turning) | request running |
| Results 2/3 ✗ | on the Results tab: assertions passed / total (✓ all passed, ✗ some failed) |
| … | text cut short |
| → ← ■ + in the gRPC and WebSocket Log tabs | sent, received, final status, seconds since the session started |
| Headers (2) | number of rows in a tab (compact: Headers(2)) |
| Body•  Auth• | the body or the auth is set |
| ▾ | opens a menu or picker |
| ‹value› | a choice: Space (or ← → in dialogs and settings) changes it |
| ▸ / ▾ in the tree | closed / open folder |
| italic request in the tree | open in a tab |
| ✕ | delete this row |
| [x] / [ ] | row enabled / disabled |
| {{name}} in red | variable not defined in the active environment |
| •••••• | a secret value, masked |

## Commands

Typed in the command palette (`Ctrl+P`, or `:` in vim normal mode).

| Command | What |
|---|---|
| `:send` | Send the current request |
| `:cancel` | Cancel the running request |
| `:watch [seconds]` | Repeat the active HTTP request on an interval |
| `:end` | Finish sending on a client/bidi stream |
| `:env <name>` | Switch environment (names listed as you type) |
| `:envs` | Edit environments |
| `:newenv` | Create an environment (asks for the name) |
| `:auth login [name]` | Sign in to an HTTP OAuth identity or an MCP server (lists them without a name) |
| `:auth logout [name]` | Remove the OAuth tokens of an identity or an MCP server (lists them without a name) |
| `:w` | Save the request |
| `:saveas <name>` | Save the request under a new name |
| `:wq` | Save and quit |
| `:q` | Quit (asks when there are unsaved changes) |
| `:q!` | Quit without saving |
| `:new` | New HTTP request (Ctrl+N) |
| `:newgrpc` | New gRPC request (Ctrl+W g) |
| `:newsse` | New SSE request (Ctrl+W E) |
| `:newmqtt` | New MQTT request (Ctrl+W M) |
| `:newws` | New WebSocket request (Ctrl+W y) |
| `:newmcp` | New MCP request |
| `:mcpservers` | Choose an MCP server |
| `:mcpservers edit` | Edit the MCP servers in the project config (Ctrl+W S) |
| `:mcpadd <name>` | Add an MCP server to the project config (asks for the URL or command) |
| `:mcpedit [name]` | Edit the details of an MCP server: env, headers, TLS, OAuth (lists them without a name) |
| `:mcptools` | Discover and choose an MCP tool |
| `:method` | Choose the HTTP method |
| `:bodytype` | Choose the body type of the HTTP request |
| `:authtype` | Choose the auth type of the request |
| `:grpcmethod` | Choose the gRPC service and method |
| `:rescan` | Read the collection from disk again |
| `:saveexample [name]` | Save the response on screen as an example of the request |
| `:examples` | List the request's saved examples and show one (:examples delete <name> deletes) |
| `:folder` | Edit the settings of the active request's folder (headers, auth, variables, scripts) |
| `:import [path]` | Import Postman, Insomnia, OpenAPI/Swagger, a .http file or a Bruno directory, or paste curl |
| `:curl` | Copy the request as curl (secret variables stay {{placeholders}}) |
| `:curl!` | Copy the request as curl including secret values |
| `:httpfile` | Copy the request as an .http file |
| `:httpfile!` | Copy the .http file including secret values |
| `:cookies` | Manage the active environment's cookies: delete one or clear all (values masked) |
| `:cookies clear` | Clear the active environment's cookies |
| `:cookies delete <number>` | Delete one cookie from the active environment |
| `:history` | Show history |
| `:clearhistory` | Delete all history entries |
| `:format` | Pretty-print the JSON body |
| `:jq [expr]` | Filter the response body with a jq expression |
| `:template` | Fill the gRPC message with a template from the schema |
| `:reload` | Reload the gRPC schema |
| `:graphqlschema` | Reload the GraphQL schema for the HTTP request |
| `:settings` | Open settings |
| `:oauth` | Edit the HTTP OAuth identities (the file Settings saves to) |
| `:keymap` | Edit your key bindings (the file Settings saves to) |
| `:theme [name]` | Switch theme (lists them without a name); :theme reload reads your theme files again |
| `:help` | Show keys |
| `:report` | Show the last report (import, curl export) again |
| `:savebody [path]` | Save the response body to a file |

## Customizing

Override any action in `~/.config/sendito/config.toml` or `<collection>/.sendito/config.toml`.
Sections are `[keymap.all]`, `[keymap.conventional]` and `[keymap.vim]`; values are a key or a list of keys.

```toml
[keymap.all]
send = ["f9", "ctrl+w e"]

[keymap.vim]
tree_delete = "D"
```
