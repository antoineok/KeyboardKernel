{
  description = "Baremetal aarch64-none-elf Rust project";

  inputs = {
    naersk.url = "github:nix-community/naersk/master";
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = {
    self,
    nixpkgs,
    naersk,
    rust-overlay,
  }: let
    forAllSystem = function:
      nixpkgs.lib.genAttrs [
        "x86_64-linux"
      ] (system: let
        overlays = [(import rust-overlay)];
        pkgs = import nixpkgs {inherit system overlays;};
      in
        function pkgs);
  in {
    formatter = forAllSystem (pkgs: pkgs.alejandra);

    packages = forAllSystem (
      pkgs: let
        naersk-lib = pkgs.callPackage naersk {
          cargo = self.packages.${pkgs.system}.rustToolchain;
          rustc = self.packages.${pkgs.system}.rustToolchain;
        };
      in {
        rustToolchain = pkgs.callPackage ./nix/rust.nix {};

        aarch64-gdb = pkgs.callPackage ./nix/gdb.nix {};

        openocd-ch347 = pkgs.callPackage ./nix/openocd.nix {};

        gdb-frontend = pkgs.python3.pkgs.callPackage ./nix/gdb-frontend.nix {};
        default = naersk-lib.buildPackage ./.;
      }
    );

    devShells = forAllSystem (pkgs: {
      default = pkgs.mkShell {
        packages = with pkgs;
        with self.packages.${pkgs.system}; [
          # Rust toolchain
          rustToolchain
          rustfmt
          rust-analyzer
          clippy

          # ARM bare-metal toolchain
          pkgsCross.aarch64-embedded.buildPackages.binutils

          # Custom debugging and flashing tools
          aarch64-gdb
          openocd-ch347
          rpiboot
          gdb-frontend

          # Build essentials
          pkg-config

          # Useful utilities for serial communication
          tmux
          python3
          python3Packages.pyserial
          python3Packages.pyusb
          python3Packages.pillow
          bash

          # Simply for convenience, to format the nix files in this project
          alejandra

          qemu
        ];
        env.RUST_GDB = pkgs.lib.getExe' self.packages.${pkgs.system}.aarch64-gdb "aarch64-none-elf-gdb";
        env.TMUX_EXE = pkgs.lib.getExe pkgs.tmux;
      };
    });
  };
}
