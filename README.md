# jay-tray-item

A [`jay_tray_v1`](https://github.com/mahkoh/jay) tray item client for the Jay compositor. Displays a custom icon in Jay's system tray and executes configurable shell commands on click.

## Usage

```
jay-tray-item [OPTIONS]

Options:
      --icon <ICON>         PNG or SVG file path, or a freedesktop icon name
      --output <NAME>       Only show on this output, by connector name (see `jay randr`), e.g. DP-1
      --left-click <CMD>    Shell command to run on left click
      --right-click <CMD>   Shell command to run on right click
      --middle-click <CMD>  Shell command to run on middle click
  -h, --help                Print help
  -V, --version             Print version
```

All options are optional. If no icon is provided, a dark gray square is shown as a fallback.

The icon appears on every monitor unless `--output` is set. Each monitor it skips is reported on stderr, so a mistyped name is visible. Monitors plugged in later are picked up automatically, even if none is connected at startup.

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

Commands are passed to `sh -c` and run in their own process group with stdin from `/dev/null`. The tray item keeps running, and Ctrl+C in its terminal does not stop them.

## Installation

Requires `cargo` and `rustc`.

```sh
git clone https://github.com/luvvlyjude/jay-tray-item
cd jay-tray-item
cargo install --path .
```

A `flake.nix` is included for NixOS users.

## Icons

`--icon` is first tried as a file path (PNG or SVG; files without a `.svg` extension are read as PNG). If that fails and the value has no `/`, it is treated as a freedesktop icon name. Each step below checks `$XDG_DATA_HOME`, then each entry of `$XDG_DATA_DIRS`, before moving to the next:

1. `icons/hicolor/<size>x<size>/<category>/<name>.png` at the exact size
2. `icons/hicolor/scalable/<category>/<name>.svg`
3. `icons/hicolor/<size>x<size>/<category>/<name>.png` at a common size from 16 to 1024, larger sizes first
4. `pixmaps/<name>.png`

Only the `hicolor` theme is searched, in the categories `apps`, `status`, `devices`, `actions`, `categories` and `mimetypes`. Non-square icons are centered and keep their aspect ratio. If an icon can't be loaded, a message goes to stderr and the gray square is shown.

SVG `<text>` is not drawn, so convert text to paths. An SVG that draws nothing counts as a failed load.

## Dependencies

Runtime: `wayland`

Build: `rust` 1.96 or newer (the protocol XML files are in this repo)

## Protocol

Uses the `jay_tray_v1` protocol, which is specific to the Jay compositor. The protocol XML is included in `wayland-protocols/jay-tray-v1.xml`.
