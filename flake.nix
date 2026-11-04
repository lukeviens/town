{
  description = "a little VM with one instruction: the word.";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs, ... }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      each = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = each (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "town";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          # town's own residents, its languages and `reload`, for a town to name first
          postInstall = ''
            mkdir -p $out/share/town
            cp -r residents $out/share/town/
          '';
        };
      });

      # town on a mac: the town cli on PATH, and launchd keeping your town open from login.
      darwinModules.default = { config, lib, pkgs, ... }:
        let
          town = config.town.package;
          home = "/Users/${config.system.primaryUser}";
        in
        {
          options.town.residents = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [ "${town}/share/town/residents" "${home}/.config/town" ];
            description = "your town: residents, or folders of them, in order. yours to write.";
          };

          options.town.package = lib.mkOption {
            type = lib.types.package;
            default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
            # its own residents are in share/town/residents
            description = "the town that runs";
          };

          config = {
            environment.systemPackages = [ town ];
            # launchd opens the log before town runs, and an empty town opens: both folders must
            # be there
            system.activationScripts.postActivation.text = ''
              sudo -u ${config.system.primaryUser} mkdir -p ${home}/.town ${home}/.config/town
            '';
            launchd.user.agents.town.serviceConfig = {
              ProgramArguments = [ "${town}/bin/town" ] ++ config.town.residents;
              # the world a town runs in is the system's: its programs are found on its PATH
              EnvironmentVariables.PATH =
                builtins.replaceStrings [ "$HOME" "$USER" ] [ home config.system.primaryUser ]
                  config.environment.systemPath;
              RunAtLoad = true;
              # restart on a crash, not on a clean exit: town exits 0 when another holds the square
              KeepAlive.SuccessfulExit = false;
              ThrottleInterval = 5;
              StandardOutPath = "${home}/.town/server.log";
              StandardErrorPath = "${home}/.town/server.log";
            };
          };
        };
    };
}
