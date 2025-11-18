{rust-bin}:
rust-bin.stable.latest.default.override {
  extensions = [
    "rustfmt"
    "clippy"
    "rust-src"
  ];
  targets = [
    "aarch64-unknown-none"
  ];
}
