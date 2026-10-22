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
                  # --web hands the page to the desktop.
                  pkgs.xdg-utils
                ]
              }
          '';

          meta = {
            description = "Tamper-evident log of media playback, output device and volume";
            mainProgram = "media-logger";
            license = pkgs.lib.licenses.mit;
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
            xdg-utils # --web
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

          # `logsRoot` is written in systemd specifiers, which mean nothing to
          # PAM or to a shell. Translate the two that have an environment
          # equivalent; anything else is caught by the assertion below rather
          # than exported as a path that silently does not exist.
          sessionRoot = builtins.replaceStrings [ "%h" "%u" ] [ "$HOME" "$USER" ] cfg.logsRoot;

          # One command line, rendered twice: once for systemd, which expands
          # its own specifiers, and once for the shell, which expands none of
          # them. The activation script below compares the second against
          # what is actually running, so they must not drift.
          command =
            root:
            lib.concatStringsSep " " (
              [
                (lib.getExe cfg.package)
                "--root"
                root
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

                It is deliberately **not** created here, because the daemon
                creates it itself and records having done so as a
                `chain_restart` with reason `CreatedRoot`. Creating it from the
                module would pre-empt that record, leaving a tree that began
                from nothing with no evidence that it did -- a silent fresh
                start, which is the thing actually worth guarding against.

                The daemon refuses only when the path exists and is not a
                directory.

                systemd specifiers are expanded, so `%h` is the user's home.

                This is also exported as `MEDIA_LOGGER_ROOT`, so `media-logger
                show` in a terminal reads the same tree without `--root`. Only
                `%h` and `%u` survive that translation (as `$HOME` and
                `$USER`); any other specifier is an evaluation error.
              '';
            };

            heartbeat = lib.mkOption {
              type = lib.types.ints.positive;
              default = 20;
              description = ''
                Seconds between heartbeats.

                A run of unchanged beats collapses into one line that is
                rewritten in place, so a short interval costs resolution
                rather than storage.
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

            # Likewise the tree itself, so reporting does not need --root.
            # sessionVariables rather than variables: PAM sets it at login, so
            # it reaches launchers and graphical sessions too, and it is merged
            # into environment.variables for shells regardless.
            #
            # The unit below still passes --root explicitly. A user service
            # must not depend on the login environment having been set up, and
            # both render the same `logsRoot`, so they cannot drift.
            environment.sessionVariables.MEDIA_LOGGER_ROOT = sessionRoot;

            assertions = [
              {
                assertion = !(lib.hasInfix "%" sessionRoot);
                message = ''
                  services.media-logger.logsRoot contains a systemd specifier
                  with no environment-variable equivalent:

                      ${cfg.logsRoot}

                  Only %h and %u can be exported to the shell, as $HOME and
                  $USER. Use an absolute path or one of those two, so that
                  `media-logger show` reads the tree the daemon writes.
                '';
              }
            ];

            # Switching a configuration reloads each user's systemd but
            # starts nothing new in it, so a service enabled mid-session
            # first runs at the next login. For this one that is the failure
            # nobody notices: enabled, never started, and a month of reports
            # that can only say no playback was found.
            #
            # Idempotent, as user activation scripts must be: starting a
            # running unit does nothing, and the restart happens only when
            # the running command line is no longer the configured one --
            # restarting on every unrelated switch would punch a hole in the
            # log for nothing.
            system.userActivationScripts.media-logger.text = ''
              unit=media-logger.service
              want="${command sessionRoot}"
              pid=$(${pkgs.systemd}/bin/systemctl --user show -p MainPID --value "$unit" 2>/dev/null || true)
              running=""
              if [ -n "$pid" ] && [ "$pid" != 0 ] && [ -r "/proc/$pid/cmdline" ]; then
                running=$(tr '\0' ' ' < "/proc/$pid/cmdline" | sed 's/ *$//')
              fi
              if [ -z "$running" ]; then
                ${pkgs.systemd}/bin/systemctl --user start "$unit" || true
              elif [ "$running" != "$want" ]; then
                ${pkgs.systemd}/bin/systemctl --user restart "$unit" || true
              fi
            '';

            systemd.user.services.media-logger = {
              description = "Tamper-evident log of media playback";
              documentation = [ "https://github.com/arximboldi/media-logger" ];

              # Retry forever rather than giving up. The daemon refuses to
              # start while the log share is unmounted, and the right
              # behaviour then is to keep trying until it appears -- not to
              # land in `failed` and leave the machine silently unlogged.
              #
              # This belongs to [Unit], not [Service]. Set in serviceConfig it
              # is parsed into the wrong section and systemd drops it with
              # "Unknown key", leaving the guarantee stated and unenforced.
              startLimitIntervalSec = 0;

              wantedBy = [ "default.target" ];
              after = [ "pipewire.service" ];
              wants = [ "pipewire.service" ];

              serviceConfig = {
                ExecStart = command cfg.logsRoot;

                Restart = "always";
                RestartSec = 10;

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
