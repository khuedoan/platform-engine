{
  inputs = {
    # TODO switch back to stable channel when Rust 1.88 is available, probably in 25.11
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
  let
    supportedSystems = nixpkgs.lib.genAttrs [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
    ];
  in
  {
    devShells = supportedSystems (system: {
      default = with nixpkgs.legacyPackages.${system}; mkShell {
        packages = [
          bacon
          cargo
          cargo-nextest
          clippy
          nixpacks
          openssl
          pkg-config
          protobuf
          rustc
          rustfmt
        ];
      };
    });
  };
}
