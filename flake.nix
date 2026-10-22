{
  description = "Tamper-evident log of media playback, output device and volume";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEachSystem = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forEachSystem (pkgs: {
        default = pkgs.mkShell {
          name = "media-logger";

          packages = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer

            # Shelled out to at runtime.
            pipewire # pw-dump, for sink and volume state
            openssl # RFC 3161 stamping, from phase 3 on
          ];

          RUST_BACKTRACE = "1";
        };
      });

      formatter = forEachSystem (pkgs: pkgs.nixfmt-rfc-style);
    };
}
