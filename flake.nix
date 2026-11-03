{
  description = "Keylux keyboard RGB control and NixOS device integration";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      packageFor =
        pkgs:
        pkgs.callPackage (
          {
            lib,
            rustPlatform,
            pkg-config,
            wrapGAppsHook3,
            makeWrapper,
            makeDesktopItem,
            gtk3,
            xdotool,
            udev,
            libusb1,
            libGL,
            libxkbcommon,
            wayland,
            libX11,
            libXcursor,
            libXi,
            libXrandr,
            libayatana-appindicator,
          }:

          rustPlatform.buildRustPackage rec {
            pname = "keylux";
            version = (lib.importTOML ./Cargo.toml).workspace.package.version;
            src = lib.fileset.toSource {
              root = ./.;
              fileset = lib.fileset.unions [
                ./Cargo.toml
                ./Cargo.lock
                ./crates
                ./effects
                ./packaging
                ./reference
                ./LICENSE-MIT
                ./LICENSE-APACHE
              ];
            };
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [
              pkg-config
              wrapGAppsHook3
              makeWrapper
            ];
            buildInputs = [
              gtk3
              xdotool
              udev
              libusb1
            ];

            # Check the whole workspace, including CLI integration tests. Hardware and
            # display-dependent tests are already ignored by the upstream test suite.
            cargoTestFlags = [ "--workspace" ];

            desktopItems = [
              (makeDesktopItem {
                name = "keylux";
                desktopName = "Keylux";
                comment = "Keyboard RGB lighting and effects";
                exec = "keylux";
                categories = [
                  "Utility"
                  "Settings"
                ];
                terminal = false;
              })
            ];

            postInstall = ''
              mkdir -p "$out/share/keylux" "$out/lib/udev/rules.d" "$out/share/applications"
              cp -r effects "$out/share/keylux/effects"
              # uaccess must be set before systemd's 73-seat-late.rules applies ACLs.
              cp packaging/99-aula.rules "$out/lib/udev/rules.d/70-keylux.rules"
              cp ${lib.head desktopItems}/share/applications/* "$out/share/applications/"
            '';

            preFixup = ''
              gappsWrapperArgs+=(
                --set-default KEYLUX_EFFECTS_DIR "$out/share/keylux/effects"
                --prefix LD_LIBRARY_PATH : ${
                  lib.makeLibraryPath [
                    libGL
                    libxkbcommon
                    wayland
                    libX11
                    libXcursor
                    libXi
                    libXrandr
                    libayatana-appindicator
                  ]
                }
              )
            '';

            doInstallCheck = true;
            installCheckPhase = ''
              runHook preInstallCheck
              # Exercise the wrapped binary away from the source tree so missing
              # runtime libraries or bundled effects cannot be hidden by the cwd.
              cd "$TMPDIR"
              "$out/bin/keylux" --version
              "$out/bin/keylux" list > effects-list.txt 2> effects-errors.txt
              test ! -s effects-errors.txt
              for script in "$out/share/keylux/effects/"*.rhai; do
                grep -F "[script: $(basename "$script")]" effects-list.txt
              done
              runHook postInstallCheck
            '';

            meta = {
              description = "Keyboard RGB control with native AULA and OpenRGB backends";
              homepage = "https://github.com/SibteProf/keylux";
              license = with lib.licenses; [
                mit
                asl20
              ];
              platforms = lib.platforms.linux;
              mainProgram = "keylux";
            };
          }

        ) { };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        rec {
          keylux = packageFor pkgs;
          default = keylux;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.keylux}/bin/keylux";
        };
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.keylux ];
            packages = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              nixfmt
              nodejs_22
            ];
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
              with pkgs;
              [
                libGL
                libxkbcommon
                wayland
                libX11
                libXcursor
                libXi
                libXrandr
                libayatana-appindicator
              ]
            );
          };
        }
      );

      nixosModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.programs.keylux;
        in
        {
          options.programs.keylux = {
            enable = lib.mkEnableOption "Keylux keyboard RGB control and AULA HID access";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.keylux;
              defaultText = lib.literalExpression "inputs.keylux.packages.\${pkgs.stdenv.hostPlatform.system}.keylux";
              description = "Keylux package to install, including its udev rules.";
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];
            services.udev.packages = [ cfg.package ];
          };
        };
      nixosModules.keylux = self.nixosModules.default;

      checks = forAllSystems (system: {
        inherit (self.packages.${system}) keylux;
      });

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
