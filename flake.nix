{
  description = "A jay_tray_v1 tray item client for the Jay compositor";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    systems.url = "github:nix-systems/default-linux";
  };

  outputs =
    { nixpkgs, systems, ... }:
    let
      forAllSystems =
        f: nixpkgs.lib.genAttrs (import systems) (system: f nixpkgs.legacyPackages.${system});

      package =
        {
          lib,
          makeWrapper,
          rustPlatform,
          wayland,
        }:
        rustPlatform.buildRustPackage {
          pname = "jay-tray-item";
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
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ makeWrapper ];

          # wl-client dlopen()s libwayland-client.so at runtime; wrap the binary
          # so the library is on LD_LIBRARY_PATH outside of a nix shell.
          postInstall = ''
            wrapProgram $out/bin/jay-tray-item \
              --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath [ wayland ]}
          '';

          meta = {
            description = "Displays a custom icon in Jay's system tray and runs shell commands on click";
            homepage = "https://github.com/luvvlyjude/jay-tray-item";
            license = lib.licenses.mit;
            mainProgram = "jay-tray-item";
            platforms = lib.platforms.linux;
          };
        };
    in
    {
      overlays.default = final: _prev: {
        jay-tray-item = final.callPackage package { };
      };

      formatter = forAllSystems (pkgs: pkgs.nixfmt);

      packages = forAllSystems (pkgs: {
        default = pkgs.callPackage package { };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
          ];
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [ pkgs.wayland ];
        };
      });
    };
}
