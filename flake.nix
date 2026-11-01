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
        };
      });

      # town on a mac: the town cli on PATH, and launchd keeping your town open from login.
      darwinModules.default = { config, lib, pkgs, ... }:
        let
          town = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
          home = "/Users/${config.system.primaryUser}";
        in
        {
          options.town.programs = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [ "${home}/.config/town" ];
            description = "your town: Lua files, or folders of them, in order. yours to write.";
          };

          config = {
            environment.systemPackages = [ town ];
            # launchd opens the log before town runs, and an empty town opens: both folders must
            # be there
            system.activationScripts.postActivation.text = ''
              sudo -u ${config.system.primaryUser} mkdir -p ${home}/.cache/town ${home}/.config/town
            '';
            launchd.user.agents.town.serviceConfig = {
              ProgramArguments = [ "${town}/bin/town" ] ++ config.town.programs;
              RunAtLoad = true;
              # restart on a crash, not on a clean exit: town exits 0 when another holds the square
              KeepAlive.SuccessfulExit = false;
              ThrottleInterval = 5;
              StandardOutPath = "${home}/.cache/town/server.log";
              StandardErrorPath = "${home}/.cache/town/server.log";
            };
          };
        };
    };
}
