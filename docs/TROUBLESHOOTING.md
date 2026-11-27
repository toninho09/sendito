# Troubleshooting

Common problems and their fixes. Each item names the setting or the key. The
settings are in [FORMAT.md](../FORMAT.md#project-and-global-config) and the
keys in [KEYMAP.md](../KEYMAP.md).

## Ctrl+Enter does not send

Your terminal does not report `Ctrl+Enter` as a different key. Use `F5` or
`Ctrl+W e`: they work in every terminal. These terminals report
`Ctrl+Enter`: kitty, Alacritty, WezTerm, foot, Ghostty, and iTerm2 with CSI u
on.

To use a different key, set it in the config file:

```toml
[keymap.all]
send = ["f9", "ctrl+w e"]
```

## "unresolved variables" and exit code 3

A `{{name}}` in the request has no value. `sendito run` does not send the
request and exits with `3`. The app asks before it sends.

1. Give the environment: `sendito run <request> --env local`. Without
   `--env`, only the collection variables are defined.
2. Make sure that the environment, the collection or a `folder.toml` defines
   the variable. The app shows an unresolved variable in red, and the status
   line names it.
3. For a secret, make sure that its value loaded. A keychain entry or a
   `secrets.toml` value that is missing makes the variable unresolved. Set the
   value with `Ctrl+W V` (the environment editor).
4. For `{{$processEnv.NAME}}`, make sure that the variable `NAME` is set in the
   shell that starts Sendito.

`--allow-unresolved` sends the request with the `{{name}}` text as it is.

A variable that only a script sets is red in the editor, because the editor
does not run scripts. The send works.

## The keychain does not work (Linux, CI, SSH)

`source = "keychain"` needs the Secret Service on Linux (GNOME Keyring or
KWallet) or the Keychain on macOS. A CI host or an SSH session often has no
keychain, so the secret does not load.

Use a different source there:

* `source = "local"`: the value is in `.sendito/secrets.toml` (git ignores it).
* `source = "file"`: the value is the contents of a file.
* `source = "dotenv"`: the value is in a `.env` file.
* `"{{$processEnv.NAME}}"`: the value is in a variable of the process
  environment. This is the usual choice in CI
  ([Run your requests in CI](GUIDES.md#run-your-requests-in-ci)).

## Requests through a corporate proxy

Set the proxy in `.sendito/config.toml` or in the global config:

```toml
[http]
proxy = "http://proxy.corp:8080"   # or "env" for HTTPS_PROXY, HTTP_PROXY, NO_PROXY
```

HTTP requests, SSE sessions and MCP servers over HTTP use the proxy. A
WebSocket session uses only an HTTP or HTTPS proxy (not SOCKS, not `"env"`).
gRPC does not use a proxy.

## Certificate errors (private CA, mutual TLS)

When the server certificate comes from a private CA, give the CA:

```toml
[http.tls]
ca = "certs/ca.pem"              # relative to the collection root
client_cert = "certs/client.pem" # mutual TLS: set the pair together
client_key = "certs/client.key"
```

`ca` replaces the system roots. HTTP, gRPC and MCP over HTTP use these files.

To skip the check for one request only, set `verify_tls = false` in its
`[options]`. Do not do this for a server that you do not control.

## The icons show as boxes or question marks

`icons = "nerd"` needs a [Nerd Font](https://www.nerdfonts.com) in your
terminal. Install one, or go back to the icons that work with every font:

```toml
[ui]
icons = "unicode"
```

## The colors are wrong or hard to read

The `auto` theme reads `COLORFGBG`, and many terminals do not set it. Then
`auto` picks the dark theme `dusk`. Set the theme yourself:

```toml
[ui]
theme = "daylight"   # light; see `sendito theme list` for all of them
```

`NO_COLOR=1` turns the colors off. Terminals without 24-bit color get the
nearest 256-color palette. Themes are in [THEMES.md](THEMES.md).

## Copy (`y`) does nothing

Sendito copies through the terminal (OSC 52) and through the local
clipboard program: `pbcopy` on macOS, `wl-copy` on Wayland, `xclip` on X11.
On Linux, install `wl-clipboard` or `xclip` when the copy does not arrive.

Over SSH, only OSC 52 reaches your computer. In tmux, turn it on:

```
set -g set-clipboard on
```

Some terminals ask for permission to write to the clipboard, or turn OSC 52
off. Look for "clipboard" in the settings of your terminal. In iTerm2, turn
on Settings → General → Selection → "Applications in terminal may access
clipboard".

## `:edit` comes back at once with no change

`:edit` (`Ctrl+W O`) waits until the editor exits. An editor that opens a
window and returns at once gives back the text unchanged. Tell it to wait:

```sh
export VISUAL="code -w"      # VS Code
export VISUAL="subl -w"      # Sublime Text
```

Sendito uses `$VISUAL`, then `$EDITOR`, then `vi`; the pager is `$PAGER`, then
`less`.

## The server gets a header that the request does not have

Sendito adds only the headers that HTTP needs (`Host`, `Content-Length`),
the auth, and a `Content-Type` that agrees with the body when the request has
none ([What goes on the wire](../FORMAT.md#what-goes-on-the-wire)). The HTTP
library adds `Accept: */*` when the request has no `Accept`, and sends HTTP/1.1
header names in Title-Case ([DEBT.md](DEBT.md)). To send a different
`Accept`, define it in the request.

## The response body is cut

Bodies larger than `[http] max_body_mb` (100 by default) are cut, and a
warning shows. Make the limit larger:

```toml
[http]
max_body_mb = 500
```

## The build from source fails in mlua or Lua

The Lua engine is compiled with Sendito, so the build needs a C compiler
(`cc`): install `build-essential` on Debian or Ubuntu, or the Xcode Command
Line Tools on macOS (`xcode-select --install`).

## The agent does not see Sendito (`sendito serve`)

The agent starts `sendito serve` and shows no Sendito tools, or it says that
the server stopped.

1. Use the full path of the binary in the agent's configuration
   (`which sendito` prints it). An app that you start from the Dock or the
   menu does not get the `PATH` of your shell.
2. Run the same command in a terminal: `sendito serve /path/to/collection
   --env local`. When it works, it prints `sendito: serving … on stdin/stdout`
   and waits; `Ctrl+D` stops it. Otherwise it prints the error:
   * `no collection at …`: give the path of the collection, or run
     `sendito init` there.
   * `environment "…" does not exist`: give a name from
     `.sendito/environments/`.
3. Start the agent again after you change its configuration.

## The agent's request has an unresolved secret

The agent starts `sendito serve` as a new process, without your shell.

* A `{{$processEnv.NAME}}` value needs `NAME` in the agent's configuration:
  `claude mcp add -e NAME=value …`, or `"env": {"NAME": "value"}` in the JSON
  ([the guide](GUIDES.md#secrets)).
* A keychain value needs a keychain that the process can open. When the agent
  runs over SSH or in a container, use `source = "local"` or
  `source = "file"` (see [The keychain does not work](#the-keychain-does-not-work-linux-ci-ssh)).

No tool shows a secret value. This is by design: tell the agent to use the
`{{name}}` placeholder.

## The agent cannot create or delete requests

The server runs with `--read-only`, which removes the tools that write. Remove
the flag from the agent's configuration and start the agent again.

## Windows

Windows is not a target of this release. The releases are for Linux and macOS.

## Something else

Look in [DEBT.md](DEBT.md) for the known limits. If the problem is not there,
open an issue on [GitHub](https://github.com/toninho09/sendito/issues) with
the Sendito version (`sendito --version`), the terminal and the steps.
