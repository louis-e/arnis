{
  inputs = {
    nixpkgs.url = "nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }: {

    packages = nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed (system:
      let
        pkgs = import nixpkgs { inherit system; };
        lib = pkgs.lib;
        toml = lib.importTOML ./Cargo.toml;
      in
      {
        default = self.packages.${system}.arnis;
        arnis = pkgs.rustPlatform.buildRustPackage {
          pname = "arnis";
          version = toml.package.version;

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "bedrockrs_core-0.1.0" = "sha256-0HP6p2x6sulZ2u8FzEfAiNAeyaUjQQWgGyK/kPo0PuQ=";
              "nbtx-0.1.0" = "sha256-JoNSL1vrUbxX6hKWB4i/DX02+hsQemANJhQaEELlT2o=";
            };
          };

          # Checks use internet connection, so we disable them in nix sandboxed environment
          doCheck = false;

          buildInputs = with pkgs; [
            openssl.dev
            libsoup_3.dev
            webkitgtk_4_1.dev
          ];
          nativeBuildInputs = with pkgs; [
            gtk3
            pango
            gdk-pixbuf
            glib
            wayland
            pkg-config
          ];

          meta = {
            description = "Generate any location from the real world in Minecraft Java Edition with a high level of detail.";
            homepage = toml.package.homepage;
            license = lib.licenses.asl20;
            maintainers = [ ];
            mainProgram = "arnis";
          };
        };
      });
    apps = nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed (system: {
      default = self.apps.${system}.arnis;
      arnis = {
        type = "app";
        program = "${self.packages.${system}.arnis}/bin/arnis";
      };
    });
  };
}
