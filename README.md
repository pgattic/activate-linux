# Activate Linux

A tiny Rust Wayland overlay that recreates the QuickShell `Activate Linux`
example without Qt, QML, or a GUI toolkit.

It creates one wlroots layer-shell overlay surface per monitor, anchors each
surface to the bottom-right corner, renders semi-transparent white text with
Cairo, and sets an empty input region so pointer events pass through.

Run it with:

```sh
nix run
```

Override the displayed text with one or two positional arguments:

```sh
nix run -- "Activate Linux" "Go to Settings to activate Linux"
```

Build it with:

```sh
nix build
```

This requires a Wayland compositor that supports `zwlr_layer_shell_v1`, such as
Sway, Hyprland, River, Wayfire, or wlroots-based compositors.
