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
      in
      {
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
