{
  description = "A jay_tray_v1 tray item client for the Jay compositor";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = import nixpkgs { inherit system; };
        inherit (pkgs) lib;
        waylandLibs = [ pkgs.wayland ];
        version = (lib.importTOML ./Cargo.toml).package.version;
        src = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            ./Cargo.toml
            ./Cargo.lock
            ./build.rs
            ./src
            ./wayland-protocols
          ];
        };
      in {
        formatter = pkgs.nixfmt-rfc-style;

        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "jay-tray-item";
          inherit version src;
          cargoLock.lockFile = ./Cargo.lock;

          buildInputs = waylandLibs;
          nativeBuildInputs = with pkgs; [ pkg-config makeWrapper ];

          # wl-client dlopen()s libwayland-client.so at runtime; wrap the binary
          # so the library is on LD_LIBRARY_PATH outside of a nix shell.
          postInstall = ''
            wrapProgram $out/bin/jay-tray-item \
              --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath waylandLibs}
          '';

          meta = {
            description = "Displays a custom icon in Jay's system tray and runs shell commands on click";
            mainProgram = "jay-tray-item";
          };
        };

        devShells.default = pkgs.mkShell {
          buildInputs = waylandLibs;
          nativeBuildInputs = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            pkg-config
          ];
          LD_LIBRARY_PATH = lib.makeLibraryPath waylandLibs;
        };
      });
}
