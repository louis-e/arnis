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
      in
      {
        devShell = pkgs.mkShell {
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
