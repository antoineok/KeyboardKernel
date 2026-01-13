{rust-bin}:
rust-bin.nightly.latest.default.override {
  extensions = [
    "rustfmt"
    "clippy"
    "rust-src"
  ];
  targets = [
    "aarch64-unknown-none"
  ];
}
