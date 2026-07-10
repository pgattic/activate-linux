# Activate Linux

*Go to Settings to activate Linux.*

A minimal Wayland shell overlay that displays an "Activate Linux" watermark, similar to the classic "Activate Windows" watermark found on unregistered Windows installations.

## Usage

NixOS users can try this program right now with `nix run github:pgattic/activate-linux`.

Otherwise, install the Rust language toolchain, then compile and run this program with:

```sh
cargo run --release
```

Make it say whatever you want using positional arguments:

```sh
cargo run --release -- "First line" "Second line"
```