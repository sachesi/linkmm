{
  description = "LinkMM development environment and build recipes";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
        };
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "linkmm";
          version = "0.1.0";
          src = ./.;

          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = with pkgs; [
            pkg-config
            cmake
            wrapGAppsHook4
          ];

          buildInputs = with pkgs; [
            gtk4
            libadwaita
            glib
            pango
            gdk-pixbuf
            cairo
            fuse3
            openssl
            dbus
            zlib
          ];

          meta = {
            description = "Link Mod Manager - A mod manager for Bethesda games";
            homepage = "https://github.com/sachesi/linkmm";
            license = pkgs.lib.licenses.gpl3Plus;
            mainProgram = "linkmm";
          };
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            pkg-config
            cargo
            rustc
            rust-analyzer
            rustfmt
            clippy
            cmake
            wrapGAppsHook4
          ];

          buildInputs = with pkgs; [
            gtk4
            libadwaita
            glib
            pango
            gdk-pixbuf
            cairo
            fuse3
            openssl
            dbus
            zlib
          ];

          shellHook = ''
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath (with pkgs; [
              gtk4
              libadwaita
              glib
              pango
              gdk-pixbuf
              cairo
              fuse3
              openssl
              zlib
            ])}:$LD_LIBRARY_PATH"
          '';
        };
      }
    );
}
