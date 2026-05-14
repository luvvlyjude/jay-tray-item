{
  description = "jay_tray_v1 tray item client for the Jay compositor";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        waylandLibs = with pkgs; [ wayland ];
      in {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "jay-tray-item";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          buildInputs = waylandLibs;
          nativeBuildInputs = with pkgs; [ pkg-config makeWrapper ];

          # wl-client loads libwayland-client.so at runtime via dlopen;
          # wrap the binary so the library is findable outside of nix-shell.
          postInstall = ''
            wrapProgram $out/bin/jay-tray-item \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath waylandLibs}
          '';
        };

        devShells.default = pkgs.mkShell {
          buildInputs = waylandLibs;
          nativeBuildInputs = with pkgs; [ cargo rustc pkg-config ];
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath waylandLibs;
        };
      });
}
