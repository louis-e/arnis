{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs.url = "nixpkgs/nixos-unstable";
  };

  outputs =
    {
      flake-utils,
      nixpkgs,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        stdenv = if pkgs.stdenv.isLinux then pkgs.stdenvAdapters.useMoldLinker pkgs.stdenv else pkgs.stdenv;
      in
      {
        devShell = pkgs.mkShell.override { inherit stdenv; } {
          buildInputs = with pkgs; [
            openssl.dev
            pkg-config
            wayland
            glib
            gdk-pixbuf
            pango
            gtk3
            libsoup_3.dev
            webkitgtk_4_1.dev
          ];
        };
      }
    );
}
