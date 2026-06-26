# Helmsman

Helmsman is a local browser UI for managing Kamal deploys from a project directory.

## Build The Binary

From the Helmsman source directory:

```sh
cargo build --release
```

The compiled binary will be created at:

```sh
target/release/helmsman
```

You can run it directly from that path, or copy it somewhere on your `PATH`.

Optional local install:

```sh
mkdir -p ~/.local/bin
cp target/release/helmsman ~/.local/bin/helmsman
```

Make sure `~/.local/bin` is on your shell `PATH` if you use the optional install.

## Run Helmsman For A Kamal Project

Run the binary from the root of the project you want to manage. For example, for a Rails app that deploys with Kamal:

```sh
cd /path/to/my-rails-app
/path/to/helmsman/target/release/helmsman
```

If you copied the binary to your `PATH`:

```sh
cd /path/to/my-rails-app
helmsman
```

Helmsman uses the current working directory as the active project. On startup it searches that project for:

```sh
deploy.yml
```

It checks the project root first, then the conventional Kamal path:

```sh
config/deploy.yml
```

If neither exists, it searches subdirectories for the first `deploy.yml` it can find. Kamal commands still run from the project root where you started Helmsman.

Then open:

```text
http://127.0.0.1:3000
```

## Runtime Notes

- `kamal deploy` and `kamal app details` are run from the project directory where you started Helmsman.
- The config editor reads and writes the discovered `deploy.yml`.
- Deploy history is stored in `helmsman.sqlite` in that project directory.
- Helmsman passes through `HOME` and `SSH_AUTH_SOCK` to Kamal.
- Helmsman runs Kamal with the current shell `PATH` first, then appends `/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin`.
- If the project has `.ruby-version` and the matching Ruby exists in RVM, Helmsman adds the matching RVM Ruby and gem paths before running Kamal.
