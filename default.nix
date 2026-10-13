{
  lib,
  rustPlatform,
  callPackage,
  runCommand,
  installShellFiles,
  git,
  gitRev ? null,
  grammarOverlays ? [],
  includeGrammarIf ? _: true,
}: let
  fs = lib.fileset;

  # `./runtime` 整棵被排除(它由 runtimeDir 单独提供),但 helix-term 用 include_str! 在**编译期**
  # 读 runtime/server-manager-builtin.toml ⇒ 必须把这一个文件并回源里,否则 nix 构建必失败
  # ("couldn't read .../runtime/server-manager-builtin.toml")。普通 cargo 构建不受影响——
  # 整个 checkout 都在,这也正是这条只红 Cachix、不红 Build 的原因。
  src = fs.union
    (fs.difference (fs.gitTracked ./.) (fs.unions [
      ./.envrc
      ./rustfmt.toml
      ./screenshot.png
      ./book
      ./docs
      ./runtime
      ./flake.lock
      (fs.fileFilter (file: lib.strings.hasInfix ".git" file.name) ./.)
      (fs.fileFilter (file: file.hasExt "svg") ./.)
      (fs.fileFilter (file: file.hasExt "md") ./.)
      (fs.fileFilter (file: file.hasExt "nix") ./.)
    ]))
    (fs.gitTracked ./runtime/server-manager-builtin.toml);

  # Next we actually need to build the grammars and the runtime directory
  # that they reside in. It is built by calling the derivation in the
  # grammars.nix file, then taking the runtime directory in the git repo
  # and hooking symlinks up to it.
  grammars = callPackage ./grammars.nix {inherit grammarOverlays includeGrammarIf;};
  runtimeDir = runCommand "helix-runtime" {} ''
    mkdir -p $out
    ln -s ${./runtime}/* $out
    rm -r $out/grammars
    ln -s ${grammars} $out/grammars
  '';
in
  rustPlatform.buildRustPackage (self: {
    cargoLock = {
      lockFile = ./Cargo.lock;
      # This is not allowed in nixpkgs but is very convenient here: it allows us to
      # avoid specifying `outputHashes` here for any git dependencies we might take
      # on temporarily.
      allowBuiltinFetchGit = true;
    };

    propagatedBuildInputs = [ runtimeDir ];
    
    nativeBuildInputs = [
      installShellFiles
      git
    ];

    buildType = "release";

    name = with builtins; (fromTOML (readFile ./helix-term/Cargo.toml)).package.name;
    src = fs.toSource {
      root = ./.;
      fileset = src;
    };

    # Helix attempts to reach out to the network and get the grammars. Nix doesn't allow this.
    HELIX_DISABLE_AUTO_GRAMMAR_BUILD = "1";

    # So Helix knows what rev it is.
    HELIX_NIX_BUILD_REV = gitRev;

    doCheck = false;
    strictDeps = true;

    # Sets the Helix runtime dir to the grammars
    env.HELIX_DEFAULT_RUNTIME = "${runtimeDir}";

    # Get all the application stuff in the output directory.
    postInstall = ''
      mkdir -p $out/lib
      installShellCompletion ${./contrib/completion}/hx.{bash,fish,zsh}
      mkdir -p $out/share/{applications,icons/hicolor/{256x256,scalable}/apps}
      cp ${./contrib/Helix.desktop} $out/share/applications/Helix.desktop
      cp ${./logo.svg} $out/share/icons/hicolor/scalable/apps/helix.svg
      cp ${./contrib/helix.png} $out/share/icons/hicolor/256x256/apps/helix.png
    '';

    meta.mainProgram = "hx";
  })
