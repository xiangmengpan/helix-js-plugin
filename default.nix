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

  # 入源内容**显式列举**,不再对文件调用 fs.gitTracked——
  # fs.gitTracked 的参数必须是**目录**(传文件会直接抛
  # "lib.fileset.gitTracked: Expected the argument ... to be a directory, but it's a file instead",
  # 求值期就红,连构建都进不去)。原先"整树 gitTracked 再做 difference"的写法
  # 一旦要往回转某个文件就必然踩这个坑,所以这里改成白名单式的路径列表。
  #
  # 列表内容 = cargo workspace 成员 + 顶层构建配置 + 被 include_str!/include_bytes!
  # 在**编译期**读入、且位于 crate 目录之外的数据文件。runtime/ 整体不入源
  # (它由下面的 runtimeDir 提供),唯独 runtime/server-manager-builtin.toml 必须单列:
  # helix-term/src/commands/server_manager.rs 用 include_str! 在编译期读它,
  # 缺了就是 "couldn't read .../runtime/server-manager-builtin.toml"。
  # (普通 cargo 构建看不到这个问题——整个 checkout 都在,这也是它只红 Cachix、
  #  不红 Build 的原因。)
  sourcePaths = [
    # Cargo.toml 里的 workspace 成员:每个成员的 Cargo.toml 都得在,
    # cargo 才解得开 workspace;src/ 也必须齐,否则 --workspace 构建直接失败。
    ./helix-core
    ./helix-view
    ./helix-term
    ./helix-tui
    ./helix-lsp-types
    ./helix-lsp
    ./helix-event
    ./helix-js
    ./helix-dap-types
    ./helix-dap
    ./helix-loader
    ./helix-vcs
    ./helix-parsec
    ./helix-stdx
    ./xtask

    # 构建配置:Cargo.lock 钉依赖;.cargo/config.toml 供 `--cfg tokio_unstable` 等 rustflags。
    ./Cargo.toml
    ./Cargo.lock
    ./.cargo/config.toml
    ./rust-toolchain.toml

    # 编译期 include_bytes!/include_str! 的顶层数据文件
    ./languages.toml # helix-loader/src/config.rs, helix-loader/src/lib.rs
    ./theme.toml # helix-view/src/theme.rs
    ./base16_theme.toml # helix-view/src/theme.rs
    ./contrib/server-manager-recipes.toml # helix-term/src/commands/server_manager.rs(测试断言)
    ./runtime/server-manager-builtin.toml # helix-term/src/commands/server_manager.rs

    # 测试语料与插件资产:不参与 `cargo build`,但属源的一部分(xtask / 集成测试用)。
    ./tests
    ./plugins
  ];

  # 上游 nixpkgs 也支持 fs.unions 直接吃普通 path(列表里只有显式路径,没有递归扫描),
  # 所以这里既不需要 gitTracked 也不需要 fileFilter 二次过滤。
  src = fs.toSource {
    root = ./.;
    fileset = fs.unions sourcePaths;
  };

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
    inherit src;

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
