{
  description = "bsdkrun - a Firecracker-style microVM launcher for BSD, Linux, and unikernel guests, on libkrun";

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

    # The Solo5 fork whose hvt tender `bsdkrun solo5` embeds — it adds the
    # macOS/HVF backend upstream does not have.
    #
    # `flake = false` because solo5 is a C project with no flake of its own,
    # and needs none: nix fetches the repo and hands the source tree to the
    # derivation below. This is also why a nix build needs no `?submodules=1`
    # — `library/solo5` is the same commit as a git submodule, for `cargo
    # build` outside nix, but nix pins it here and in flake.lock instead.
    # Bump both together: `git submodule update --remote library/solo5` and
    # `nix flake update solo5`. The e2e-solo5 workflow fails if they drift.
    solo5 = {
      url = "github:tsirysndr/solo5/hvf-macos-aarch64";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, crane, flake-utils, libkrun-pvh, solo5, ... }:
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

        # ---- web UI ---------------------------------------------------------
        #
        # `bsdkrun ui` serves an SPA compiled into the binary by rust-embed, so
        # the bundle has to exist before cargo runs.

        # node_modules for the web SPA. `bun install` needs the network, so
        # dependency resolution lives in a fixed-output derivation.
        #
        # The tree is NOT platform-independent: native optional deps (e.g.
        # @tailwindcss/oxide-*) are installed per-OS/CPU, so each system needs
        # its own hash.
        #
        # Updating: change web/package.json or web/bun.lock, set the current
        # system's hash below to lib.fakeHash, run `nix build`, and copy the
        # hash Nix reports on mismatch back in. Repeat per platform.
        webNodeModules = pkgs.stdenv.mkDerivation {
          pname = "bsdkrun-web-node-modules";
          version = "0.1.0";

          src = lib.fileset.toSource {
            root = ./web;
            fileset = lib.fileset.unions [
              ./web/package.json
              ./web/bun.lock
            ];
          };

          nativeBuildInputs = [ pkgs.bun ];

          dontConfigure = true;

          buildPhase = ''
            runHook preBuild
            export HOME=$(mktemp -d)
            bun install --frozen-lockfile --no-progress
            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            mv node_modules $out
            runHook postInstall
          '';

          dontFixup = true;

          outputHashMode = "recursive";
          outputHashAlgo = "sha256";
          # These genuinely differ per system — the three below are three
          # distinct trees, which is the per-platform caveat above in practice.
          outputHash = {
            aarch64-darwin = "sha256-HBlPd3BIT0bhXsqQtgzf2x9L4p+UHRST+cAuPTvIAdY=";
            x86_64-linux = "sha256-817iL8whjah44OISnq2vwyCeYrAeGVlJxk4j0ri4ACw=";
            aarch64-linux = "sha256-dAAh/6mXbzHSiqqFDjoNlw9rLHB4CWqYMdpiSoLApbY=";
          }.${system};
        };

        # The built SPA. Embedded into the bsdkrun binary at compile time.
        # ---- bsdkrun pack: the Go half ------------------------------------
        # Built as its own derivation and copied into core/src/pack-bin
        # before cargo runs, exactly as webUi is copied into web/dist.
        #
        # core/build.rs would otherwise run `go build` itself, which cannot
        # work here: a nix build has no network, so fetching modules fails
        # and build.rs degrades to shipping a binary with no pack support at
        # all. buildGoModule fetches them in a fixed-output derivation
        # instead, which is the one place a nix build is allowed to.
        #
        # vendorHash covers those modules. If a dependency changes, nix will
        # report the expected value; regenerate it with:
        #   cd pack && go mod vendor -o /tmp/v && nix hash path --sri /tmp/v
        packBin = pkgs.buildGoModule {
          pname = "bsdkrun-pack";
          version = "0.1.0";

          src = ./pack;
          vendorHash = "sha256-c1/3o8pfZ3td2iXE+o3r2aE3i8PuSLe0IEkIisgvR0A=";

          # Matches what core/build.rs builds: a static binary with no libc
          # dependency, which is what makes it safe to embed and exec.
          env.CGO_ENABLED = "0";
          ldflags = [ "-s" "-w" ];

          # The binary has to carry the name rust_embed looks for.
          postInstall = ''
            mv $out/bin/pack $out/bin/bsdkrun-pack 2>/dev/null || true
          '';

          # There are tests, but they shell out to docker for the buildkit
          # ones; the unit tests that matter run in CI.
          doCheck = false;
        };

        # ---- solo5-hvt: the Solo5 tender ----------------------------------
        # `bsdkrun solo5` runs MirageOS unikernels through this, and it is not
        # a libkrun guest: the tender drives Hypervisor.framework (macOS) or
        # KVM (Linux) in its own process. bsdkrun embeds it, so an end user
        # needs no Solo5 install.
        #
        # Its own derivation, for the same reason as packBin: crane's
        # `cleanCargoSource` keeps only Rust and Cargo files, so
        # `library/solo5` never reaches the cargo build. core/build.rs then
        # finds no configure.sh there, leaves `core/src/solo5-bin` untouched
        # (rather than clobbering it), and rust_embed bakes in whatever
        # `preBuild` put there — which is this.
        #
        # The source is the `solo5` flake input, NOT `./library/solo5`: a
        # flake does not fetch git submodules, so a plain `nix build` would
        # otherwise get an empty directory and fail at configure. The
        # submodule and the input are the same commit — see the input's
        # comment for how to bump them together.
        solo5Tender = pkgs.stdenv.mkDerivation {
          pname = "solo5-hvt";
          version = "0.9.0-bsdkrun";

          src = solo5;

          # pkg-config + libseccomp: the hvt tender installs a seccomp filter
          # on Linux (hvt_seccomp_linux.c), and solo5's configure.sh fails
          # outright without the headers. Darwin needs neither — it has no
          # seccomp, which the tender documents as a gap rather than papering
          # over.
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = lib.optionals (!isDarwin) [ pkgs.libseccomp ];

          postPatch = ''
            # scripts/gen_version_h.sh derives the version from `git describe`
            # and dies outside a git tree — which a nix source always is. Its
            # documented fallback is this file, the release-tarball case.
            cat > include/version.h.distrib <<'EOF'
            /* Generated by bsdkrun's flake.nix, do not edit */
            #ifndef __VERSION_H__
            #define __VERSION_H__
            #define SOLO5_VERSION "bsdkrun-nix"
            #endif
            EOF
          '';

          # --disable-toolchain stops before the cross-compiler and bindings,
          # which would want ld.lld and llvm-objcopy to emit ELF on a Mach-O
          # host. Building a unikernel needs those; running one does not, and
          # this only ever runs one. --disable-elftool likewise: bsdkrun reads
          # the manifest note itself (core/src/solo5.rs).
          configurePhase = ''
            runHook preConfigure
            ./configure.sh --disable-toolchain --disable-elftool
            runHook postConfigure
          '';

          buildPhase = ''
            runHook preBuild
            make -j"$NIX_BUILD_CORES"
            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            install -Dm755 tenders/hvt/solo5-hvt $out/bin/solo5-hvt
            runHook postInstall
          '';

          # The tender's own link step signs it with the hypervisor
          # entitlement, but nix's stdenv puts no `codesign` on PATH at all
          # here — so point it at the real one, the same /usr/bin/codesign
          # this flake uses to sign bsdkrun itself.
          preConfigure = lib.optionalString isDarwin ''
            mkdir -p .codesign-shim
            cat > .codesign-shim/codesign <<'EOF'
            #!/bin/sh
            exec /usr/bin/codesign "$@"
            EOF
            chmod +x .codesign-shim/codesign
            export PATH="$PWD/.codesign-shim:$PATH"
          '';

          # ...and sign it again here, which is the signature that actually
          # survives: nix's darwin fixupPhase re-signs every binary in $out
          # ad-hoc, silently dropping the entitlements the link step applied.
          # postFixup runs after it. Without this the tender builds, installs
          # and then dies at run time with
          #   HVF: hv_vm_create() failed: 0xfae94007
          # which names the entitlement but not the step that removed it.
          # (`bsdkrun solo5` re-signs the tender when it extracts it, so this
          # matters most for `nix run .#solo5-hvt` — using the tender directly.)
          postFixup = lib.optionalString isDarwin ''
            printf '%s' ${lib.escapeShellArg entitlements} > entitlements.plist
            /usr/bin/codesign --entitlements entitlements.plist --force \
              --sign - "$out/bin/solo5-hvt"
          '';

          meta = with lib; {
            description = "Solo5 hvt tender (Hypervisor.framework on macOS, KVM on Linux)";
            homepage = "https://github.com/tsirysndr/solo5";
            license = licenses.isc;
            mainProgram = "solo5-hvt";
            platforms = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
          };
        };

        webUi = pkgs.stdenv.mkDerivation {
          pname = "bsdkrun-web";
          version = "0.1.0";

          src = ./web;

          nativeBuildInputs = [ pkgs.bun pkgs.nodejs ];

          configurePhase = ''
            runHook preConfigure
            cp -r ${webNodeModules} node_modules
            chmod -R u+w node_modules
            patchShebangs node_modules
            export HOME=$(mktemp -d)
            runHook postConfigure
          '';

          buildPhase = ''
            runHook preBuild
            bun run build
            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            cp -r dist $out
            runHook postInstall
          '';
        };



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
          version = "0.8.1";
          strictDeps = true;
          # Explicit even though the workspace's `default-members` is this
          # package: the daemon is a member too, and nothing here should build
          # it by accident.
          cargoExtraArgs = "-p bsdkrun";

          # llvm (llvm-config) + libclang for bindgen-based crates.
          nativeBuildInputs = [ pkgs.pkg-config pkgs.llvmPackages.llvm ];
          buildInputs = lib.optionals (!isDarwin) [ libkrun ];

          LIBKRUN_PREFIX = libkrunPrefix;
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          # rust-embed reads web/dist at compile time, and cleanCargoSource
          # strips the whole web directory, so drop the pre-built SPA back in
          # before cargo runs. Without this the binary compiles fine and ships
          # build.rs's "UI not built" placeholder.
          preBuild = ''
            mkdir -p web
            cp -r ${webUi} web/dist
            chmod -R u+w web/dist

            # rust_embed embeds whatever is here. build.rs finds no `go` on
            # PATH and leaves this alone rather than overwriting it, so the
            # binary that lands in bsdkrun is the one nix built.
            mkdir -p core/src/pack-bin
            cp ${packBin}/bin/bsdkrun-pack core/src/pack-bin/bsdkrun-pack
            chmod -R u+w core/src/pack-bin

            # Same deal for the Solo5 tender: build.rs finds no
            # library/solo5 in the cleaned source, so it leaves this alone
            # rather than rebuilding it, and rust_embed bakes in the tender
            # nix built.
            mkdir -p core/src/solo5-bin
            cp ${solo5Tender}/bin/solo5-hvt core/src/solo5-bin/solo5-hvt
            chmod -R u+w core/src/solo5-bin
          '';
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # ---- bsdkrund: the gRPC daemon ------------------------------------
        # A workspace member, not a standalone crate: it depends on
        # `bsdkrun-core` by path, so its source has to be the whole workspace
        # (`../core` has to exist) and `-p bsdkrun-daemon` is what narrows the
        # build back down to it.
        #
        # It links the engine WITHOUT its `boot` feature, so it stays the pure,
        # hypervisor-free package it has always been: booting lives in
        # `bsdkrun-supervisor`, which ships beside it and is the only half that
        # needs libkrun.
        daemonArgs = {
          # NOT `cleanCargoSource`: that keeps only Rust and Cargo files, which
          # drops proto/bsdkrun.proto and leaves the build script failing with
          # "Could not make proto path relative". Keep .proto files too.
          src = lib.cleanSourceWith {
            src = ./.;
            name = "source";
            filter = path: type:
              (builtins.match ".*\\.proto$" path != null)
              || (craneLib.filterCargoSources path type);
          };
          pname = "bsdkrund";
          version = "0.1.0";
          strictDeps = true;
          # The workspace's `default-members` is the CLI, so without this the
          # daemon build would build bsdkrun instead — and want the web bundle.
          cargoExtraArgs = "-p bsdkrun-daemon";

          # libkrun is wired in for `bsdkrun-supervisor`, which overrides these
          # args below. `bsdkrund` itself takes `bsdkrun-core` without its
          # `boot` feature, so it links no hypervisor at all — but the two share
          # this attrset, and an unused LIBKRUN_PREFIX costs nothing.
          nativeBuildInputs = [ pkgs.pkg-config pkgs.llvmPackages.llvm pkgs.protobuf ];
          buildInputs = lib.optionals (!isDarwin) [ libkrun ];
          LIBKRUN_PREFIX = libkrunPrefix;
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          # tonic-prost-build shells out to protoc to compile the .proto.
          # PROTOC_INCLUDE isn't needed today (nothing imports a well-known
          # type), but without it the first `import "google/protobuf/*.proto"`
          # would fail only under Nix, which is a confusing way to find out.
          PROTOC = "${pkgs.protobuf}/bin/protoc";
          PROTOC_INCLUDE = "${pkgs.protobuf}/include";
        };

        daemonArtifacts = craneLib.buildDepsOnly daemonArgs;

        # ---- bsdkrun-supervisor -------------------------------------------
        # The half of the daemon that links libkrun, split out so `bsdkrund`
        # itself does not have to. Same source tree, different package.
        supervisorArgs = daemonArgs // {
          pname = "bsdkrun-supervisor";
          version = "0.8.1";
          cargoExtraArgs = "-p bsdkrun-supervisor";

          # The supervisor carries the embedded Solo5 tender (its core dep
          # turns the `solo5` feature on, so daemon-driven solo5 boots work).
          # Same trick as the bsdkrun package: build.rs finds no library/solo5
          # in the cleaned source and leaves this copy alone.
          preBuild = ''
            mkdir -p core/src/solo5-bin
            cp ${solo5Tender}/bin/solo5-hvt core/src/solo5-bin/solo5-hvt
            chmod -R u+w core/src/solo5-bin
          '';
        };

        supervisorArtifacts = craneLib.buildDepsOnly supervisorArgs;

        bsdkrun-supervisor = craneLib.buildPackage (supervisorArgs // {
          cargoArtifacts = supervisorArtifacts;

          nativeBuildInputs = supervisorArgs.nativeBuildInputs
            ++ lib.optionals (!isDarwin) [ pkgs.makeWrapper ];
          postInstall = lib.optionalString (!isDarwin) ''
            wrapProgram $out/bin/bsdkrun-supervisor \
              --prefix PATH : ${lib.makeBinPath runtimeDeps}
          '';

          meta = with lib; {
            description =
              "Runs one bsdkrun command in its own process, for bsdkrund (not a user-facing tool)";
            homepage = "https://github.com/tsirysndr/bsdkrun";
            license = licenses.mit;
            mainProgram = "bsdkrun-supervisor";
            platforms = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
          };
        });

        bsdkrund = craneLib.buildPackage (daemonArgs // {
          cargoArtifacts = daemonArtifacts;

          # No `bsdkrun` wrapper: the daemon needs no CLI on PATH at all. It
          # links the engine and supervises machines by re-exec'ing itself, so
          # the only thing it still shells out to is the runtime tools below.
          nativeBuildInputs = daemonArgs.nativeBuildInputs
            ++ lib.optionals (!isDarwin) [ pkgs.makeWrapper ];
          postInstall = lib.optionalString (!isDarwin) ''
            wrapProgram $out/bin/bsdkrund \
              --prefix PATH : ${lib.makeBinPath runtimeDeps}
          '';

          meta = with lib; {
            description =
              "Token-authenticated gRPC daemon for running bsdkrun machines on a remote VPS or bare-metal KVM host";
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
            description = "Firecracker-style microVM launcher for BSD, Linux, and unikernel guests on libkrun";
            homepage = "https://github.com/tsirysndr/bsdkrun";
            license = licenses.mit;
            mainProgram = "bsdkrun";
            platforms = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
          };
        });
      in
      {
        checks = {
          inherit bsdkrun bsdkrund bsdkrun-supervisor;

          bsdkrun-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets";
          });

          bsdkrun-fmt = craneLib.cargoFmt { inherit src; };

          bsdkrund-clippy = craneLib.cargoClippy (daemonArgs // {
            cargoArtifacts = daemonArtifacts;
            cargoClippyExtraArgs = "--all-targets -- -D warnings";
          });

        };

        packages.default = bsdkrun;
        packages.bsdkrun = bsdkrun;
        packages.bsdkrund = bsdkrund;
        packages.bsdkrun-supervisor = bsdkrun-supervisor;
        # The SPA on its own, for serving from something other than `bsdkrun ui`.
        packages.web = webUi;
        # The Solo5 tender on its own — useful for running a unikernel with
        # plain `solo5-hvt`, and for checking the tender builds without
        # waiting for the whole Rust build behind it.
        packages.solo5-hvt = solo5Tender;

        apps.default = flake-utils.lib.mkApp { drv = bsdkrun; };
        apps.bsdkrund = flake-utils.lib.mkApp { drv = bsdkrund; };

        # `nix develop` — toolchain + libkrun (Linux) + everything bsdkrun needs,
        # plus zig/cargo-zigbuild for cross-building guest agents.
        devShells.default = craneLib.devShell {
          checks = self.checks.${system};

          LIBKRUN_PREFIX = libkrunPrefix;
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          packages = with pkgs;
            # protobuf so the daemon's build script can run protoc too.
            # bun + node to build the web UI (`make web`).
            # go builds pack/, which core/build.rs compiles and embeds.
            [ pkg-config llvmPackages.llvm cargo-zigbuild zig protobuf bun nodejs go ]
            # core/build.rs builds the Solo5 tender from library/solo5 in a
            # plain `cargo build`, which needs make and (on Linux) libseccomp
            # — without them the build only warns and the tender is silently
            # left out of the binary.
            ++ [ gnumake ]
            ++ lib.optionals (!isDarwin) [ libkrun libseccomp ]
            ++ runtimeDeps;
        };
      });
}
