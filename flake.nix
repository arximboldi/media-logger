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
      packages = forEachSystem (pkgs: rec {
        default = media-logger;

        media-logger = pkgs.rustPlatform.buildRustPackage {
          pname = "media-logger";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.makeWrapper ];

          # The chain tests take the state-directory lock, which needs a HOME.
          # The sandbox has none; the daemon always does under systemd.
          preCheck = "export HOME=$(mktemp -d)";

          # The daemon shells out to pw-dump for sink and volume state.
          # Without this it builds fine and then silently records no device
          # on any host that lacks PipeWire on PATH.
          postInstall = ''
            wrapProgram $out/bin/media-logger \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.pipewire ]}
          '';

          meta = {
            description = "Tamper-evident log of media playback, output device and volume";
            mainProgram = "media-logger";
            platforms = pkgs.lib.platforms.linux;
          };
        };
      });

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
