{
  python3,
  pkgsCross,
  ...
}:
pkgsCross.aarch64-embedded.buildPackages.gdb.override {
  pythonSupport = true;
  inherit python3;
}
