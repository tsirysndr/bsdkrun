{
  description = "bsdkrun - a Firecracker-style microVM launcher for BSD and Linux guests, on libkrun (KVM)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    crane.url = "github:ipetkov/crane";

    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, crane, flake-utils, ... }:
    # libkrun in nixpkgs is Linux-only (KVM); macOS uses Homebrew + codesigning,
    # so the Nix package targets Linux. Both CPU arches are supported.
    flake-utils.lib.eachSystem [
      "x86_64-linux"
      "aarch64-linux"
    ] (system:
      let
        pkgs = import nixpkgs { inherit system; };
        inherit (pkgs) lib;

        craneLib = crane.mkLib pkgs;
        src = craneLib.cleanCargoSource ./.;

        # Tools bsdkrun shells out to at runtime (image/agent download, disk
        # prep, and gvproxy for user-mode networking).
        runtimeDeps = with pkgs; [ curl gnutar gzip xz cpio util-linux gvproxy ];

        commonArgs = {
          inherit src;
          pname = "bsdkrun";
          version = "0.1.0";
          strictDeps = true;

          # llvm (llvm-config) + libclang are needed by bindgen-based crates.
          nativeBuildInputs = [ pkgs.pkg-config pkgs.llvmPackages.llvm ];
          buildInputs = [ pkgs.libkrun ];

          # build.rs links libkrun from here (skips brew/pkg-config probing).
          LIBKRUN_PREFIX = "${pkgs.libkrun}";
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        };

        # Build dependencies separately so CI can cache them (standard crane layout).
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        bsdkrun = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;

          nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.makeWrapper ];
          # Put the runtime tools on PATH as a fallback (--suffix keeps any the
          # user already has first).
          postInstall = ''
            wrapProgram $out/bin/bsdkrun \
              --suffix PATH : ${lib.makeBinPath runtimeDeps}
          '';

          meta = with lib; {
            description = "Firecracker-style microVM launcher for BSD and Linux guests on libkrun (KVM)";
            homepage = "https://github.com/tsirysndr/bsdkrun";
            license = licenses.mit;
            mainProgram = "bsdkrun";
            platforms = [ "x86_64-linux" "aarch64-linux" ];
          };
        });
      in
      {
        checks = {
          inherit bsdkrun;

          bsdkrun-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets";
          });

          bsdkrun-fmt = craneLib.cargoFmt { inherit src; };
        };

        packages.default = bsdkrun;
        packages.bsdkrun = bsdkrun;

        apps.default = flake-utils.lib.mkApp { drv = bsdkrun; };

        # `nix develop` — toolchain + libkrun + everything bsdkrun needs at build
        # and run time, plus zig/cargo-zigbuild for cross-building guest agents.
        devShells.default = craneLib.devShell {
          checks = self.checks.${system};

          LIBKRUN_PREFIX = "${pkgs.libkrun}";
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          packages = with pkgs; [
            pkg-config
            llvmPackages.llvm
            libkrun
            cargo-zigbuild
            zig
          ] ++ runtimeDeps;
        };
      });
}
