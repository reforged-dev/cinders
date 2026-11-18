{
  description = "Cinders, a static site generator";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";
    # nixpkgs' rustc trails stable, and oxc needs a current one.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      inherit (nixpkgs) lib;
      # Upgraded together with oxc.
      rustVersion = "1.100.0";
      systems = [
        "aarch64-darwin"
        "x86_64-linux"
        "aarch64-linux"
      ];
      # Linux uses the static musl package set, so its binaries run on any distribution.
      forEachSystem =
        f:
        lib.genAttrs systems (
          system:
          let
            pkgs = import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            };
          in
          f (if pkgs.stdenv.hostPlatform.isLinux then pkgs.pkgsStatic else pkgs)
        );

      target = pkgs: pkgs.stdenv.hostPlatform.rust.rustcTarget;
      toolchain =
        pkgs: extra:
        pkgs.pkgsBuildHost.rust-bin.stable.${rustVersion}.minimal.override {
          extensions = [
            "clippy"
            "rustfmt"
          ]
          ++ extra;
          targets = [ (target pkgs) ];
        };
      # libavif-sys builds libaom from source with cmake, which runs Perl and, on x86, nasm.
      nativeTools =
        pkgs:
        [
          pkgs.pkgsBuildHost.cmake
          pkgs.pkgsBuildHost.perl
        ]
        ++ lib.optional pkgs.stdenv.hostPlatform.isx86 pkgs.pkgsBuildHost.nasm;

      package =
        pkgs:
        let
          rust = toolchain pkgs [ ];
        in
        (pkgs.makeRustPlatform {
          cargo = rust;
          rustc = rust;
        }).buildRustPackage
          {
            pname = "cinders";
            version = (lib.importTOML ./Cargo.toml).package.version;
            src = lib.fileset.toSource {
              root = ./.;
              fileset = lib.fileset.unions [
                ./Cargo.toml
                ./Cargo.lock
                ./.cargo
                ./src
              ];
            };
            cargoLock.lockFile = ./Cargo.lock;
            buildType = "dist";
            # nixpkgs turns off the profile's `strip` and keeps the symbol table by default. Its
            # default flags for stripping everything fail on a Mach-O binary.
            stripAllList = [ "bin" ];
            stripAllFlags = [ "--strip-all" ];
            nativeBuildInputs = nativeTools pkgs;
            # cmake is here for libaom's build script, not to configure this package.
            dontUseCmakeConfigure = true;
            # scripts/check.sh runs the tests.
            doCheck = false;
            meta.license = lib.licenses.mit;
          };
    in
    {
      packages = forEachSystem (pkgs: {
        default = package pkgs;
      });

      devShells = forEachSystem (
        pkgs:
        let
          cc = "${pkgs.stdenv.cc.targetPrefix}cc";
          var = lib.replaceStrings [ "-" ] [ "_" ] (target pkgs);
        in
        {
          default = pkgs.mkShell (
            {
              # An editor runs the language server, which reads the standard library's source.
              packages = [
                (toolchain pkgs [
                  "rust-src"
                  "rust-analyzer"
                ])
              ]
              ++ nativeTools pkgs;
            }
            // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              # Build scripts link with the build platform's compiler, the crate with musl's.
              depsBuildBuild = [ pkgs.pkgsBuildHost.stdenv.cc ];
              CARGO_BUILD_TARGET = target pkgs;
              "CARGO_TARGET_${lib.toUpper var}_LINKER" = cc;
              "CC_${var}" = cc;
              "CXX_${var}" = "${pkgs.stdenv.cc.targetPrefix}c++";
            }
          );
        }
      );

      formatter = forEachSystem (pkgs: pkgs.pkgsBuildHost.nixfmt);
    };
}
