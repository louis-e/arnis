{
  inputs = {
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs.url = "nixpkgs/nixos-unstable";
  };

  outputs = { self, fenix, flake-utils, nixpkgs }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        fenixPkgs = (fenix.packages.${system}.stable);
      in
      {
        devShell = pkgs.mkShell
          {
            buildInputs = with pkgs; [
              openssl.dev
              pkg-config
              fenixPkgs.toolchain
            ];
          };
      });
}
