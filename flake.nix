{
  description = "CloudLedger Rust, Tauri v2, and Android development shell";

  inputs = {
    nixpkgs.url = "tarball+https://channels.nixos.org/nixos-26.05/nixexprs.tar.xz";
    rust-overlay.url = "tarball+https://github.com/oxalica/rust-overlay/archive/refs/heads/master.tar.gz";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      systems = [ "x86_64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
            config = {
              android_sdk.accept_license = true;
              allowUnfree = true;
            };
          };

          androidApiLevel = "36";
          androidBuildToolsVersion = "35.0.0";
          androidMinSdk = "24";

          androidComposition = pkgs.androidenv.composeAndroidPackages {
            platformVersions = [ androidApiLevel ];
            buildToolsVersions = [ androidBuildToolsVersion ];
            includeNDK = true;
            includeEmulator = false;
            includeSystemImages = false;
          };

          androidSdk = androidComposition.androidsdk;
          androidHome = "${androidSdk}/libexec/android-sdk";
          androidNdkHome = "${androidHome}/ndk-bundle";
          androidLlvmBin = "${androidNdkHome}/toolchains/llvm/prebuilt/linux-x86_64/bin";

          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "clippy"
              "rust-src"
              "rustfmt"
            ];
            targets = [
              "aarch64-linux-android"
              "armv7-linux-androideabi"
              "i686-linux-android"
              "x86_64-linux-android"
            ];
          };

          linuxTauriInputs = with pkgs; [
            atk
            cairo
            dbus
            gdk-pixbuf
            glib
            gtk3
            harfbuzz
            libayatana-appindicator
            librsvg
            libsoup_3
            openssl
            pango
            webkitgtk_4_1
            xdotool
          ];
        in
        {
          default = pkgs.mkShell {
            packages =
              [
                androidSdk
                pkgs.cmake
                pkgs.git
                pkgs.gradle
                pkgs.jdk17_headless
                pkgs.just
                pkgs.nodejs_22
                pkgs.pkg-config
                rustToolchain
              ]
              ++ linuxTauriInputs;

            ANDROID_HOME = androidHome;
            ANDROID_SDK_ROOT = androidHome;
            ANDROID_NDK_ROOT = androidNdkHome;
            NDK_HOME = androidNdkHome;
            JAVA_HOME = "${pkgs.jdk17_headless}";

            CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = "${androidLlvmBin}/aarch64-linux-android${androidMinSdk}-clang";
            CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER = "${androidLlvmBin}/armv7a-linux-androideabi${androidMinSdk}-clang";
            CARGO_TARGET_I686_LINUX_ANDROID_LINKER = "${androidLlvmBin}/i686-linux-android${androidMinSdk}-clang";
            CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER = "${androidLlvmBin}/x86_64-linux-android${androidMinSdk}-clang";

            CC_aarch64_linux_android = "${androidLlvmBin}/aarch64-linux-android${androidMinSdk}-clang";
            CC_armv7_linux_androideabi = "${androidLlvmBin}/armv7a-linux-androideabi${androidMinSdk}-clang";
            CC_i686_linux_android = "${androidLlvmBin}/i686-linux-android${androidMinSdk}-clang";
            CC_x86_64_linux_android = "${androidLlvmBin}/x86_64-linux-android${androidMinSdk}-clang";

            AR_aarch64_linux_android = "${androidLlvmBin}/llvm-ar";
            AR_armv7_linux_androideabi = "${androidLlvmBin}/llvm-ar";
            AR_i686_linux_android = "${androidLlvmBin}/llvm-ar";
            AR_x86_64_linux_android = "${androidLlvmBin}/llvm-ar";

            GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${androidHome}/build-tools/${androidBuildToolsVersion}/aapt2";
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath linuxTauriInputs;
            PKG_CONFIG_PATH = pkgs.lib.makeSearchPathOutput "dev" "lib/pkgconfig" linuxTauriInputs;

            shellHook = ''
              export PATH="${androidHome}/platform-tools:${androidHome}/cmdline-tools/latest/bin:${androidLlvmBin}:$PATH"
              export GRADLE_USER_HOME="$PWD/.gradle"
              echo "CloudLedger dev shell: Rust, Tauri v2 Linux deps, and Android SDK are available."
            '';
          };
        }
      );
    };
}
