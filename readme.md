# Helmsman

Helmsman is a local, view-only browser UI for a Kamal project. It shows containers, logs, proxy traffic, the audit log, and `deploy.yml`. It does not deploy, roll back, reboot, or edit the config.

Download the binary from **[here](https://github.com/JijoBose/Helmsman/releases/tag/v1.0.0)** and run the binary from the root of the project you want to manage. For example, for a Rails app that deploys with Kamal:


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

Helmsman only runs these Kamal commands. Each one ends with `--skip-hooks`:

- `kamal app details --skip-hooks`
- `kamal proxy details --skip-hooks`
- `kamal accessory details <name> --skip-hooks`
- `kamal app logs --lines N --skip-hooks`
- `kamal proxy logs --lines N --skip-hooks`
- `kamal audit --skip-hooks`

Kamal 2.12 still runs hooks when that flag is set on a subcommand, so Helmsman also loads a small Ruby file into those processes that makes hook lookup miss. A hook in the project cannot run.

It does not run `kamal deploy`, `kamal rollback`, `kamal server exec`, or any boot, start, stop, reboot, or remove command. Accessory names are checked before they are passed through, and log line counts are capped.

- Commands run from the project directory where you started Helmsman.
- `deploy.yml` is displayed and is not written.
- Older deploy runs, if any, stay in `helmsman.sqlite` in that project directory.
- Helmsman passes through `HOME` and `SSH_AUTH_SOCK` to Kamal.
- Helmsman runs Kamal with the current shell `PATH` first, then appends `/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin`.
- If the project has `.ruby-version` and the matching Ruby exists in RVM, Helmsman adds the matching RVM Ruby and gem paths before running Kamal.
