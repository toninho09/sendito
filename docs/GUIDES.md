# Guides

Short recipes for common tasks. Each recipe shows the complete files, so you
can copy them and change the values. Every field is described in
[FORMAT.md](../FORMAT.md), and every key in [KEYMAP.md](../KEYMAP.md).

You can write the files by hand or make them in the app: the app writes the
same files.

* [Your first collection](#your-first-collection)
* [Log in once and use the token](#log-in-once-and-use-the-token)
* [Run your requests in CI](#run-your-requests-in-ci)
* [Move from Postman, Insomnia or Bruno](#move-from-postman-insomnia-or-bruno)
* [Call a gRPC service](#call-a-grpc-service)

---

## Your first collection

A collection is a directory. Each request is one TOML file in it, and each
subdirectory is a folder. Keep the directory in git.

1. Create the collection:

   ```sh
   sendito init my-api
   cd my-api
   ```

   `sendito init` makes `.sendito/` with an empty `local` environment and adds
   the local files (secrets, history, session, cookies) to `.gitignore`.

2. Set the variables of the `local` environment in
   `.sendito/environments/local.toml`:

   ```toml
   [variables]
   base_url = "http://localhost:8080"
   token = { source = "local" }
   ```

   `base_url` is a plain value: it is in the file and in git. `token` is a
   secret: its value is in `.sendito/secrets.toml`, which git ignores.

3. Put the secret value in `.sendito/secrets.toml`:

   ```toml
   [local]
   token = "my-dev-token"
   ```

   You can also set it in the app: `Ctrl+W V` opens the environment editor.
   Other secret sources are the OS keychain, a file and a `.env` file
   ([Environment files](../FORMAT.md#environment-files)).

4. Create the request `Users/list users.toml`. The file name is the request
   name:

   ```toml
   kind = "http"
   url = "{{base_url}}/users"

   [auth]
   type = "bearer"
   token = "{{token}}"

   [[scripts.assert]]
   from = "status"
   op = "eq"
   value = "200"
   ```

   The assertion checks the status of the response.

5. Send the request. In the app, run `sendito`, select the request and press
   `Ctrl+Enter` or `F5`. From the shell:

   ```sh
   sendito run "Users/list users" --env local
   ```

   The body goes to stdout. The status and the assertion results go to stderr.
   The exit code is `1` when an assertion fails.

Secrets are masked (`••••••`) in the app, in the history and in exports
(`sendito curl`).

---

## Log in once and use the token

A login request stores the token in a variable. The requests after it use the
variable. `sendito run <folder>` sends the requests of a folder in tree order
and keeps the variables that each request extracts.

The tree sorts by name, `2` before `10`. Start the names with a number to set
the order.

```
Orders/
├── folder.toml
├── 1 login.toml
├── 2 create order.toml
└── 3 get order.toml
```

The environment holds the user name and the password:

```toml
# .sendito/environments/local.toml
[variables]
base_url = "http://localhost:8080"
api_user = "ana"
api_password = { source = "local" }
```

`Orders/folder.toml` sets the auth of every request in the folder:

```toml
[auth]
type = "bearer"
token = "{{token}}"
```

`Orders/1 login.toml` turns the folder auth off, sends the credentials and
extracts the token. `secret = true` masks the token everywhere:

```toml
kind = "http"
method = "POST"
url = "{{base_url}}/login"

[auth]
type = "none"

[body]
type = "json"
text = '''
{"user": "{{api_user}}", "password": "{{api_password}}"}
'''

[[scripts.extract]]
var = "token"
from = "jsonpath"
expr = "$.access_token"
secret = true

[[scripts.assert]]
from = "status"
op = "eq"
value = "200"
```

`Orders/2 create order.toml` uses the token from the folder and extracts the
ID of the new order:

```toml
kind = "http"
method = "POST"
url = "{{base_url}}/orders"

[body]
type = "json"
text = '''
{"item": "book", "quantity": 1}
'''

[[scripts.extract]]
var = "order_id"
from = "jsonpath"
expr = "$.id"

[[scripts.assert]]
from = "status"
op = "eq"
value = "201"
```

`Orders/3 get order.toml` uses the ID:

```toml
kind = "http"
url = "{{base_url}}/orders/{{order_id}}"

[[scripts.assert]]
from = "jsonpath"
expr = "$.item"
op = "eq"
value = "book"
```

Run the folder:

```sh
sendito run Orders --env local
```

```
sendito: running 3 request(s) under "Orders"
PASS  Orders/1 login.toml  (200 OK, 41 ms)
PASS  Orders/2 create order.toml  (201 Created, 12 ms)
PASS  Orders/3 get order.toml  (200 OK, 9 ms)
3 passed, 0 failed, 0 error(s)  (3, 0.1 s)
```

In the app, send `1 login` first: the variables it extracts stay available to
the next sends. The app keeps a secret variable until you quit, and the other
variables in the session. Other sources for `extract` are a header, a cookie, a regex
and XPath ([Script sources](../FORMAT.md#script-sources)). For logic that
rows cannot express, use a Lua script ([Lua scripts](../FORMAT.md#lua-scripts)).

---

## Run your requests in CI

A CI job runs `sendito run` and fails when the exit code is not `0`:

| Code | Meaning |
|---|---|
| `0` | Every assertion passed |
| `1` | An assertion or a Lua test failed |
| `2` | A request failed (network error, gRPC status other than `OK`, …) |
| `3` | Usage error or unresolved variables |

A CI host has no OS keychain and no `secrets.toml`. Make an environment for CI
that reads the secrets from the process environment with
`{{$processEnv.NAME}}`:

```toml
# .sendito/environments/ci.toml
[variables]
base_url = "https://staging.example.com"
api_user = "ci-bot"
api_password = "{{$processEnv.API_PASSWORD}}"
```

A value from `$processEnv` is a secret: Sendito masks it as it masks the
other secrets. When the variable is not set, the requests that use it are
not sent and the exit code is `3`.

This GitHub Actions workflow installs a release and runs every request of the
collection. Put it in `.github/workflows/` of the repository that holds the
collection. Set `API_PASSWORD` in the repository secrets, and change the
version to the release you want:

```yaml
name: API checks

on:
  push:
  workflow_dispatch:

jobs:
  api:
    runs-on: ubuntu-24.04
    env:
      SENDITO: sendito-v0.2.0-x86_64-unknown-linux-gnu
    steps:
      - uses: actions/checkout@v4

      - name: Install Sendito
        run: |
          curl -fsSLO "https://github.com/toninho09/sendito/releases/download/v0.2.0/$SENDITO.tar.gz"
          curl -fsSLO "https://github.com/toninho09/sendito/releases/download/v0.2.0/$SENDITO.tar.gz.sha256"
          sha256sum -c "$SENDITO.tar.gz.sha256"
          tar xzf "$SENDITO.tar.gz"
          sudo install "$SENDITO/sendito" /usr/local/bin/

      - name: Run the requests
        env:
          API_PASSWORD: ${{ secrets.API_PASSWORD }}
        run: sendito run . --env ci --no-history --report json > report.json

      - name: Keep the report
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: sendito-report
          path: report.json
```

`sendito run .` runs the whole collection from its root. Give a folder name
to run only that folder. `--report json` writes a summary to stdout
([The JSON report](CLI.md#the-json-report)); the `PASS` and `FAIL` lines stay
in the job log. `--fail-fast` stops at the first failure.

---

## Move from Postman, Insomnia or Bruno

`sendito import` reads a Postman Collection v2.1, an Insomnia v4 or v5
export, a Bruno directory, an OpenAPI 3 or Swagger 2 document, or a `.http`
file. Run it inside a collection:

```sh
sendito init my-api && cd my-api
sendito import ~/Downloads/acme.postman_collection.json --into Acme
```

In the app, `Ctrl+W i` opens the same import.

The import prints what it made and a list of the items that it did not
import or changed, with the reason for each item. In the app, the report
dialog puts the items in three groups (`:report` opens it again):

| Group | What to do |
|---|---|
| Needs action | Do the step that the item names. For example, set a secret value or an OAuth identity |
| Skipped | Sendito cannot convert it. Do it again in Sendito (see below) |
| Imported with changes | Read the change. The request works, but it is not the same as the source |

Some things do not convert:

* **JavaScript scripts** (Postman, Bruno, `.http` response handlers). Write
  them again as script rows (`[[scripts.extract]]`, `[[scripts.assert]]`) or
  as a Lua script.
* **OAuth 2 auth.** Add an `[oauth.<name>]` identity to
  `.sendito/config.toml` and set `[auth] type = "oauth"` in the request or in
  its `folder.toml` ([Project and global config](../FORMAT.md#project-and-global-config)).
* **Secret and private environment values.** Set them again with `Ctrl+W V`.

Then send the requests. Read the changes with `git diff` before you commit
them. Everything that the import reads and skips is in
[FEATURES.md](FEATURES.md#import-and-export) and [DEBT.md](DEBT.md).

---

## Call a gRPC service

When the server has reflection, the request needs only the target, the
service, the method and the message:

```toml
kind = "grpc"
target = "localhost:50051"
service = "acme.users.v1.Users"
method = "GetUser"
message = '''
{"id": "42"}
'''

[[scripts.assert]]
from = "jsonpath"
expr = "$.user.name"
op = "eq"
value = "Ana"
```

In the app, `:newgrpc` (or `n` in the sidebar) makes a gRPC request.
`:grpcmethod` lists the services and methods of the schema, and `:template`
fills the message with a template from the schema.

When the server has no reflection, give the `.proto` files. Sendito compiles
them; you do not need `protoc`:

```toml
[schema]
source = "files"
files = ["protos/acme/users/v1/users.proto"]   # relative to the collection root
import_paths = ["protos"]                       # where the imports start
```

* `message` is proto3 JSON. For a client-streaming or bidi method, a JSON
  array sends one message for each element.
* `https://` before the target, or `[options] tls = true`, connects with TLS.
* `[[metadata]]` rows and `[auth]` are sent as metadata.
* In an assertion, `status` is the gRPC status code (`0` is `OK`) and
  `jsonpath` reads the last message.

The complete format is in [gRPC request](../FORMAT.md#grpc-request).
