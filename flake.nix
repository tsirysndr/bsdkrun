{
  description = "bsdkrun - a Firecracker-style microVM launcher for BSD and Linux guests, on libkrun";

  # Both bsdkrun's and the libkrun fork's CI push to this cache — declaring it
  # here lets `nix build`/`nix develop` substitute libkrun-pvh (and bsdkrun
  # itself) instead of compiling; nix asks once to trust it.
  nixConfig = {
    extra-substituters = [ "https://bsdkrun.cachix.org" ];
    extra-trusted-public-keys =
      [ "bsdkrun.cachix.org-1:KzvN59TR6k15k7Fl7SxTEhxJnE0MvbxLC2HpxdVlC9Q=" ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    crane.url = "github:ipetkov/crane";

    flake-utils.url = "github:numtide/flake-utils";

    # The PVH-enabled libkrun fork — what boots NetBSD/FreeBSD amd64 on
    # Linux/KVM (stock libkrun only speaks the Linux boot protocol there).
    # Its CI pushes builds to the `bsdkrun` Cachix cache. Deliberately NO
    # `nixpkgs.follows`: rewriting the fork's nixpkgs would change its
    # derivation hash and miss the cache its CI populated — to substitute a
    # producer's build, consume it with the producer's own locked inputs.
    libkrun-pvh.url = "github:tsirysndr/libkrun/feat/pvh-boot";
  };

  outputs = { self, nixpkgs, crane, flake-utils, libkrun-pvh, ... }:
    # Linux (KVM) uses nixpkgs' libkrun. macOS (Hypervisor.framework) has no
    # libkrun in nixpkgs, so it links Homebrew's — an *impure* build:
    #   brew install libkrun && nix build --impure .#bsdkrun
    flake-utils.lib.eachSystem [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
    ] (system:
      let
        pkgs = import nixpkgs { inherit system; };
        inherit (pkgs) lib;
        isDarwin = pkgs.stdenv.isDarwin;

        craneLib = crane.mkLib pkgs;
        src = craneLib.cleanCargoSource ./.;

        # Linux: the PVH libkrun fork's flake (built with BLK=1 NET=1 there).
        # Beyond stock libkrun it adds the PVH direct-boot path that NetBSD's
        # MICROVM and FreeBSD's FIRECRACKER amd64 kernels need. Only defined
        # for Linux systems — never forced on darwin (guarded by !isDarwin).
        libkrun = libkrun-pvh.packages.${system}.default;

        # macOS: Homebrew's libkrun. During an *impure* build (BSDKRUN_IMPURE=1 +
        # `nix build --impure`) we import it into the store so the sandboxed build
        # can link it (a bare /opt/homebrew path isn't visible in the sandbox).
        # During *pure* evaluation — e.g. `flakehub-push`, which inspects every
        # system's drvPath on a Linux runner where /opt/homebrew is absent and
        # absolute-path access is forbidden — `builtins.getEnv` returns "", so we
        # fall back to a plain path string and never touch the forbidden path.
        brewLibkrun = builtins.path {
          path = /opt/homebrew/opt/libkrun;
          name = "libkrun-brew";
        };

        libkrunPrefix =
          if isDarwin then
            (if builtins.getEnv "BSDKRUN_IMPURE" != ""
            then "${brewLibkrun}"
            else "/opt/homebrew/opt/libkrun")
          else "${libkrun}";

        # Tools bsdkrun shells out to at runtime. On macOS the system versions are
        # fine (and losetup/gvproxy don't apply), so we only wrap on Linux.
        runtimeDeps = with pkgs;
          [ curl gnutar gzip xz cpio ] ++ lib.optionals (!isDarwin) [ util-linux gvproxy ];

        # The entitlement libkrun requires on macOS (Hypervisor.framework).
        entitlements = ''
          <?xml version="1.0" encoding="UTF-8"?>
          <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
          <plist version="1.0">
          <dict>
            <key>com.apple.security.hypervisor</key>
            <true/>
            <key>com.apple.security.cs.disable-library-validation</key>
            <true/>
          </dict>
          </plist>
        '';

        commonArgs = {
          inherit src;
          pname = "bsdkrun";
          version = "0.5.2";
          strictDeps = true;

          # llvm (llvm-config) + libclang for bindgen-based crates.
          nativeBuildInputs = [ pkgs.pkg-config pkgs.llvmPackages.llvm ];
          buildInputs = lib.optionals (!isDarwin) [ libkrun ];

          LIBKRUN_PREFIX = libkrunPrefix;
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # ---- bsdkrund: the gRPC daemon ------------------------------------
        # A separate crate with its own workspace and lockfile. It links no
        # libkrun — it drives the `bsdkrun` binary as a subprocess — so unlike
        # bsdkrun itself it builds purely on every system, darwin included, and
        # needs no `--impure` and no Homebrew.
        daemonArgs = {
          # NOT `cleanCargoSource`: that keeps only Rust and Cargo files, which
          # drops proto/bsdkrun.proto and leaves the build script failing with
          # "Could not make proto path relative". Keep .proto files too.
          src = lib.cleanSourceWith {
            src = ./daemon;
            name = "source";
            filter = path: type:
              (builtins.match ".*\\.proto$" path != null)
              || (craneLib.filterCargoSources path type);
          };
          pname = "bsdkrund";
          version = "0.1.0";
          strictDeps = true;

          # tonic-prost-build shells out to protoc to compile the .proto.
          # PROTOC_INCLUDE isn't needed today (nothing imports a well-known
          # type), but without it the first `import "google/protobuf/*.proto"`
          # would fail only under Nix, which is a confusing way to find out.
          nativeBuildInputs = [ pkgs.protobuf ];
          PROTOC = "${pkgs.protobuf}/bin/protoc";
          PROTOC_INCLUDE = "${pkgs.protobuf}/include";
        };

        daemonArtifacts = craneLib.buildDepsOnly daemonArgs;

        bsdkrund = craneLib.buildPackage (daemonArgs // {
          cargoArtifacts = daemonArtifacts;

          # Deliberately NOT wrapped with bsdkrun on PATH. The daemon's contract
          # is that it drives whatever `bsdkrun` the host has installed, and
          # baking one in would drag bsdkrun's libkrun dependency — impure on
          # darwin — into a package that otherwise builds anywhere. Install both
          # into the same profile and the daemon finds the CLI on PATH.
          meta = with lib; {
            description =
              "Token-authenticated gRPC daemon that drives the bsdkrun CLI on a remote VPS or bare-metal KVM host";
            homepage = "https://github.com/tsirysndr/bsdkrun";
            license = licenses.mit;
            mainProgram = "bsdkrund";
            platforms = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
          };
        });

        bsdkrun = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;

          nativeBuildInputs = commonArgs.nativeBuildInputs
            ++ lib.optionals (!isDarwin) [ pkgs.makeWrapper ];

          postInstall = lib.optionalString (!isDarwin) ''
            wrapProgram $out/bin/bsdkrun \
              --suffix PATH : ${lib.makeBinPath runtimeDeps}
          '';

          # Re-sign with the hypervisor entitlement AFTER Nix's own darwin
          # signing (postFixup runs after fixupPhase), so the entitlement sticks.
          postFixup = lib.optionalString isDarwin ''
            printf '%s' ${lib.escapeShellArg entitlements} > entitlements.plist
            /usr/bin/codesign --entitlements entitlements.plist --force \
              --sign - "$out/bin/bsdkrun"
          '';

          meta = with lib; {
            description = "Firecracker-style microVM launcher for BSD and Linux guests on libkrun";
            homepage = "https://github.com/tsirysndr/bsdkrun";
            license = licenses.mit;
            mainProgram = "bsdkrun";
            platforms = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
          };
        });
      in
      {
        checks = {
          inherit bsdkrun bsdkrund;

          bsdkrun-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets";
          });

          bsdkrun-fmt = craneLib.cargoFmt { inherit src; };

          bsdkrund-clippy = craneLib.cargoClippy (daemonArgs // {
            cargoArtifacts = daemonArtifacts;
            cargoClippyExtraArgs = "--all-targets -- -D warnings";
          });

          bsdkrund-fmt = craneLib.cargoFmt {
            inherit (daemonArgs) src;
            pname = "bsdkrund";
          };
        };

        packages.default = bsdkrun;
        packages.bsdkrun = bsdkrun;
        packages.bsdkrund = bsdkrund;

        apps.default = flake-utils.lib.mkApp { drv = bsdkrun; };
        apps.bsdkrund = flake-utils.lib.mkApp { drv = bsdkrund; };

        # `nix develop` — toolchain + libkrun (Linux) + everything bsdkrun needs,
        # plus zig/cargo-zigbuild for cross-building guest agents.
        devShells.default = craneLib.devShell {
          checks = self.checks.${system};

          LIBKRUN_PREFIX = libkrunPrefix;
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          packages = with pkgs;
            # protobuf so `cargo build` inside daemon/ can run protoc too.
            [ pkg-config llvmPackages.llvm cargo-zigbuild zig protobuf ]
            ++ lib.optionals (!isDarwin) [ libkrun ]
            ++ runtimeDeps;
        };
      });
}
