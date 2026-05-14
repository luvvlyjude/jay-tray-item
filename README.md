# custom-jay-tray-item

A Wayland tray item for the [Jay compositor](https://github.com/mahkoh/jay) using the `jay_tray_v1` protocol. Displays a custom icon in Jay's system tray and executes configurable commands on click.

## Usage

```
jay-tray-item [OPTIONS]

Options:
      --icon <icon>              PNG or SVG file path, or a freedesktop icon name
      --left-click <CMD>         Shell command to run on left click
      --right-click <CMD>        Shell command to run on right click
      --middle-click <CMD>       Shell command to run on middle click
  -h, --help                     Print help
  -V, --version                  Print version
```

All options are optional. If no icon is provided, a dark gray square is shown as a fallback.

### Examples

```sh
# Simple launcher icon
jay-tray-item --icon firefox --left-click "firefox"

# Toggle script with a custom icon
jay-tray-item --icon /home/user/.config/icons/toggle.png \
              --left-click "~/.local/bin/toggle-thing.sh"

# Different actions per button
jay-tray-item --icon network-manager \
              --left-click "nm-applet" \
              --right-click "kitty -e nmtui"
```

Commands are passed to `sh -c` and spawned detached — the tray item remains running.

## Installation

### Arch Linux

```sh
cargo install --path .
```

Or with the provided PKGBUILD (if available):

```sh
makepkg -si
```

### NixOS

Add the flake to your inputs and include the package:

```nix
# flake.nix
inputs.custom-jay-tray-item.url = "github:youruser/custom-jay-tray-item";

# In your system or home-manager config:
environment.systemPackages = [ inputs.custom-jay-tray-item.packages.${system}.default ];
```

Or run directly without installing:

```sh
nix run github:youruser/custom-jay-tray-item -- --icon my-icon --left-click "my-command"
```

## Icon resolution

When `--icon` is a file path that exists, it is loaded directly (PNG and SVG supported). Otherwise the value is treated as a freedesktop icon name and searched in:

- `$XDG_DATA_HOME/icons/hicolor/<size>x<size>/<category>/<name>.png`
- `$XDG_DATA_DIRS/icons/hicolor/<size>x<size>/...`
- `$XDG_DATA_HOME/icons/hicolor/scalable/<category>/<name>.svg`
- `$XDG_DATA_DIRS/icons/hicolor/scalable/...`
- `$XDG_DATA_DIRS/pixmaps/<name>.png`

## Dependencies

Runtime: `libwayland-client.so` (provided by `wayland` package).

Build: Rust toolchain, wayland protocol XML files (provided by `wayland` and `wayland-protocols` packages).

## Protocol

Uses the `jay_tray_v1` Wayland extension protocol, which is only supported by the Jay compositor. The protocol XML is included in `wayland-protocols/jay-tray-v1.xml`.
