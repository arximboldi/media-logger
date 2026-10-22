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

          # The daemon shells out to pw-dump for sink and volume state, and
          # to openssl and curl for RFC 3161 timestamping. Without this it
          # builds fine and then silently records no device and no anchors on
          # any host lacking them -- failures shaped like missing data rather
          # than missing dependencies, which is the worst shape for a defect
          # in a record meant to be evidence.
          postInstall = ''
            wrapProgram $out/bin/media-logger \
              --prefix PATH : ${
                pkgs.lib.makeBinPath [
                  pkgs.pipewire
                  pkgs.openssl
                  pkgs.curl
                  # --format pdf renders the HTML report.
                  pkgs.python3Packages.weasyprint
                ]
              }
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
            openssl # RFC 3161 stamping
            curl # sending timestamp requests
            python3Packages.weasyprint # --format pdf
          ];

          RUST_BACKTRACE = "1";
        };
      });

      nixosModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.media-logger;
        in
        {
          options.services.media-logger = {
            enable = lib.mkEnableOption "the media playback logger";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.media-logger;
              defaultText = lib.literalExpression "media-logger";
              description = "Package providing the media-logger binary.";
            };

            logsRoot = lib.mkOption {
              type = lib.types.str;
              example = "%h/sync/media-logs";
              description = ''
                Root of the log tree. The daemon writes only to
                `<logsRoot>/<hostname>/`, so several machines can share one
                replicated directory without contending.

                It is deliberately **not** created here. The daemon refuses to
                start when the root is missing, because an unmounted or
                not-yet-synced share is indistinguishable from a fresh install
                and starting a new chain would orphan the real history.
                Creating it automatically would defeat that guard.

                systemd specifiers are expanded, so `%h` is the user's home.
              '';
            };

            heartbeat = lib.mkOption {
              type = lib.types.ints.positive;
              default = 10;
              description = ''
                Seconds between heartbeats. Unchanged beats are abbreviated,
                so a short interval costs far less than it appears to.
              '';
            };

            host = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              description = "Name to log under. Defaults to the machine's hostname.";
            };

            quietHours = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [ "22:00-06:00" ];
              example = [
                "22:00-06:00"
                "13:00-15:00"
                "sun 00:00-24:00"
              ];
              description = ''
                Ruhezeiten that `media-logger show` shades and counts against.

                The default is Nachtruhe, which has broad statutory backing.
                Mittagsruhe and all-day Sunday rules come from a building's
                Hausordnung and vary, so they are not assumed: shading a
                period as quiet that the house rules do not impose would
                overstate the case.

                Read by the reporting commands, not the daemon, so changing
                it does not disturb the log.
              '';
            };
          };

          config = lib.mkIf cfg.enable {
            # A *user* service: the daemon reads the session bus, which a
            # system service cannot reach.
            environment.systemPackages = [ cfg.package ];

            # So `media-logger show` in a terminal uses the same hours the
            # module declares, without them having to be retyped.
            environment.variables.MEDIA_LOGGER_QUIET_HOURS = lib.concatStringsSep ";" cfg.quietHours;

            systemd.user.services.media-logger = {
              description = "Tamper-evident log of media playback";
              documentation = [ "https://github.com/arximboldi/media-logger" ];

              wantedBy = [ "default.target" ];
              after = [ "pipewire.service" ];
              wants = [ "pipewire.service" ];

              serviceConfig = {
                ExecStart = lib.concatStringsSep " " (
                  [
                    (lib.getExe cfg.package)
                    "--root"
                    cfg.logsRoot
                  ]
                  ++ lib.optionals (cfg.host != null) [
                    "--host"
                    cfg.host
                  ]
                  ++ [
                    "run"
                    "--heartbeat"
                    (toString cfg.heartbeat)
                  ]
                );

                Restart = "always";
                RestartSec = 10;

                # Retry forever rather than giving up. The daemon refuses to
                # start while the log share is unmounted, and the right
                # behaviour then is to keep trying until it appears -- not to
                # land in `failed` and leave the machine silently unlogged.
                StartLimitIntervalSec = 0;

                # Writes only to the log tree and its own state directory.
                StateDirectory = "media-logger";
                PrivateTmp = true;
                ProtectKernelTunables = true;
                ProtectControlGroups = true;
                RestrictSUIDSGID = true;
                NoNewPrivileges = true;
              };
            };
          };
        };

      formatter = forEachSystem (pkgs: pkgs.nixfmt);
    };
}
