# Activate Linux

*Go to Settings to activate Linux.*

A minimal Wayland shell overlay that displays an "Activate Linux" watermark, similar to the classic "Activate Windows" watermark found on unregistered Windows installations.

## Usage

NixOS users can try this program right now:

```sh
nix run github:pgattic/activate-linux
```

Otherwise, you'll first need to install system dependencies:

```sh
# Need Cairo and Wayland C libraries

sudo pacman -S cairo wayland # Arch Linux
sudo dnf install cairo wayland-devel # Fedora
```

Then, use the Rust language toolchain to install this program:

```sh
cargo install --git https://github.com/pgattic/activate-linux
```

Run it with:

```sh
activate-linux
```

Make it say whatever you want using positional arguments:

```sh
activate-linux "First line" "Second line"
```

Customize placement and appearance with flags:

```sh
activate-linux \
  --corner top-left \
  --margin 32 \
  --color '#ffcc00' \
  --opacity 0.5 \
  "Activated Linux" "Everything is configured."
```

Available placement flags:

```sh
--corner top-left|top-right|bottom-left|bottom-right
--margin PX
--margin-top PX
--margin-right PX
--margin-bottom PX
--margin-left PX
```

`--color` accepts `#RGB`, `#RRGGBB`, `RGB`, or `RRGGBB`.
`--opacity` accepts a value from `0.0` to `1.0`.
