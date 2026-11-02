{
  description = "andy-ide development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        # Libraries the window system and the GL context need at run time.
        # wayland/libxkbcommon for Wayland sessions, the xorg set for X11,
        # libGL for the glow renderer.
        graphicsLibs = with pkgs; [
          libxkbcommon
          wayland
          libGL
          fontconfig
          freetype
        ];

        xorgLibs = with pkgs; [
          libx11
          libxcursor
          libxrandr
          libxi
          libxext
        ];

        # The same program as an installable package, plus the two ways to carry it to another machine.
        #
        # A binary built here cannot simply be copied: its ELF interpreter points into /nix/store, so the
        # kernel of a machine without Nix cannot even start it. Both ways out are here:
        #
        #   nix copy / nix run   — to another machine that has Nix; the whole closure travels with it;
        #   nix bundle           — to a machine with no Nix at all: the closure is packed into a single
        #                          self-extracting file.
        #
        # Either way the window system stays the host's: winit opens libwayland-client, libxkbcommon,
        # libX11 and libGL at run time with `dlopen` rather than linking them, so those libraries have to
        # exist where the program runs. Every Linux desktop has them; the wrapper below hands them over.
        windowed = pkgs.rustPlatform.buildRustPackage {
          pname = "andy-ide";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          # The checks of a windowed program need a window; the devshell is where they run.
          doCheck = false;
        };

        runnable = pkgs.writeShellScriptBin "andy-ide" ''
          export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath (graphicsLibs ++ xorgLibs)}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
          exec ${windowed}/bin/andy-ide "$@"
        '';
      in
      {
        packages.default = windowed;
        packages.run = runnable;
        apps.default = {
          type = "app";
          program = "${runnable}/bin/andy-ide";
        };

        devShells.default = pkgs.mkShell {
          name = "andy-ide-dev";

          nativeBuildInputs = with pkgs; [
            pkg-config
            gcc
            rustc
            cargo
            clippy
            rustfmt
            rust-analyzer
            git
            # Measuring the coverage the technical requirements ask for. `llvm` is heavy, and it
            # is here because its version has to match the LLVM the compiler was built on.
            cargo-llvm-cov
            llvmPackages.llvm
          ];

          buildInputs = graphicsLibs ++ xorgLibs;

          shellHook = ''
            # target/ inside the repository is not writable by every user on
            # this host; keep build artifacts in the user's cache.
            export CARGO_TARGET_DIR="''${CARGO_TARGET_DIR:-$HOME/.cache/andy-ide-target}"
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath (graphicsLibs ++ xorgLibs)}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

            # `cargo-llvm-cov` looks for the `llvm-tools-preview` component of a rustup
            # installation; this toolchain comes from nixpkgs and has no components, so the two
            # programs it needs are named outright. They come from the same nixpkgs as the compiler
            # because the versions have to match.
            export LLVM_COV="${pkgs.llvmPackages.llvm}/bin/llvm-cov"
            export LLVM_PROFDATA="${pkgs.llvmPackages.llvm}/bin/llvm-profdata"
          '';
        };
      });
}
