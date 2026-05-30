# ADR 0021: Windows (x64-MSVC) サポート

- **Status**: Accepted
- **Date**: 2026-05-29
- **Spec sections affected**: なし（実装・CI 内部）; ツール: compiler, runtime, CI

## Context

### v0.7 時点の Windows 対応状況

v0.7 時点で Windows は CI の `cargo check` のみを実行しており、
ジョブは `allow-failure: true` で設定されている。
実際のコンパイルおよびランタイムテストは一切 Windows では行われていない。

コードベース内に `#[cfg(target_os = "windows")]` が存在しない。
runtime は以下の POSIX API に依存している:

- `pthread` (スレッド、ミューテックス)
- `clock_gettime` (高精度タイマー)
- `mmap` / `mprotect` (スタック保護)
- `sigaction` (シグナルハンドラ)

これらは Windows の Win32 API と非互換である。

### ユーザーリーチの観点

開発者向け言語処理系において、Windows は依然として大きなシェアを持つプラットフォームである。
`cargo check` のみでは「Windows でビルドが通る」という保証にすらならない。
v0.8 で Windows を first-class サポートすることで、ユーザーリーチ拡大の最大のレバレッジが得られる。

### ABI の選択: MSVC vs MinGW

Windows 向け Rust ビルドには主に2つの ABI がある:

- **x86_64-pc-windows-msvc** (MSVC ABI): `cl.exe` / `clang-cl` コンパイラ、COFF オブジェクト形式、`lld-link` リンカ
- **x86_64-pc-windows-gnu** (GNU ABI): MinGW-w64、ELF ライクな `.a` / `.so`、`ld.lld` リンカ

vcpkg の `x64-windows` triplet は MSVC ABI を前提としており、
bdwgc (Boehm GC) のビルド済みパッケージも MSVC ABI で提供される。
MinGW GNU ABI は MSVC ABI の `.lib` / `.dll` と ABI 非互換であり、
`lld-link` (COFF) と `ld.lld` (ELF/PE) の混在を招く。

**MSVC ABI (x86_64-pc-windows-msvc triplet) を採択する。**

## Decision

### 1. libgc (Boehm GC) の Windows 配布

vcpkg の `x64-windows` triplet を使用して bdwgc をビルドする:

```powershell
vcpkg install bdwgc:x64-windows
```

これにより以下が生成される:

- `gc.lib` — インポートライブラリ (ビルド時のみ使用)
- `gc.dll` — 実行時 DLL (配布物に同梱)

CI では `VCPKG_ROOT` 環境変数を設定し、`build.rs` から vcpkg パスを参照する。

### 2. リンカ: `clang-cl` + `lld-link` (COFF)

Unix 系の `clang` + `ld` とは別に、Windows 向けの `build_link_cmd_windows` 関数を追加する:

```rust
// compiler/crates/tyra-codegen-llvm/src/linker.rs
#[cfg(windows)]
fn build_link_cmd_windows(obj_path: &Path, out_path: &Path, gc_lib_dir: &Path) -> Command {
    let mut cmd = Command::new("lld-link");
    cmd.arg(obj_path)
       .arg(format!("/OUT:{}", out_path.display()))
       .arg(format!("/LIBPATH:{}", gc_lib_dir.display()))
       .arg("gc.lib")
       .arg("msvcrt.lib")
       .arg("kernel32.lib");
    cmd
}
```

LLVM IR からオブジェクトへの変換は `llc` を使用する:

```
llc -filetype=obj -mtriple=x86_64-pc-windows-msvc input.ll -o output.obj
```

Unix 系の `-mtriple=x86_64-unknown-linux-gnu` とは別の triplet を指定する。

### 3. `gc.dll` の自動コピー

`tyra build` コマンドの実行後、出力バイナリと同じディレクトリに `gc.dll` を自動コピーする:

```rust
// compiler/crates/tyra-cli/src/build.rs
#[cfg(windows)]
fn copy_runtime_dlls(out_dir: &Path) -> Result<()> {
    let gc_dll = find_gc_dll()?;
    let dest = out_dir.join("gc.dll");
    std::fs::copy(&gc_dll, &dest)?;
    Ok(())
}
```

Windows のダイナミックリンカは exe と同階層の DLL を自動的に探索するため、
PATH の設定なしで実行できる。
ユーザーは `tyra build` の出力ディレクトリをそのまま配布できる。

### 4. runtime POSIX 抽象化

POSIX 依存コードを `platform/unix.rs` / `platform/windows.rs` に分離する:

```
runtime/src/platform/
  mod.rs          // #[cfg(unix)] / #[cfg(windows)] で切り替え
  unix.rs         // pthread, clock_gettime, mmap, sigaction
  windows.rs      // Win32: CreateThread, QueryPerformanceCounter, VirtualAlloc, SetUnhandledExceptionFilter
```

**対応表**:

| POSIX API | Win32 API |
|-----------|-----------|
| `pthread_create` | `CreateThread` |
| `clock_gettime(CLOCK_MONOTONIC)` | `QueryPerformanceCounter` |
| `mmap(PROT_NONE)` (スタックガード) | `VirtualAlloc(MEM_RESERVE)` |
| `sigaction(SIGSEGV)` | `SetUnhandledExceptionFilter` |

Boehm GC 自体は Windows 対応済みのため、GC 初期化 (`GC_init`, `GC_malloc`) は変更不要。

### 5. llvm-config 探索順序

Windows では `llvm-config` の場所が環境によって異なる。以下の順序で探索する:

1. `LLVM_SYS_<ver>_PREFIX` 環境変数 (例: `LLVM_SYS_190_PREFIX=C:\LLVM`)
2. vcpkg インストールパス: `%VCPKG_ROOT%\installed\x64-windows\`
3. デフォルトインストール先: `C:\Program Files\LLVM\`
4. `where llvm-config` (PATH から探索)

`build.rs` でこの探索ロジックを実装し、見つからない場合は明確なエラーメッセージを出す。

### 6. 配布形式

Windows リリースアーティファクトは ZIP アーカイブとして提供する:

```
tyra-<version>-windows-x86_64.zip
  tyra.exe    // コンパイラ本体
  gc.dll      // Boehm GC ランタイム (同階層必須)
```

`gc.lib` はビルド時のみ必要なため、配布物には含めない。

ユーザーは ZIP を展開してそのまま使用できる (PATH への追加は任意)。

### 7. release-gate-windows

v0.7 まで `allow-failure: true` だった Windows CI ジョブを v0.8 で **required** に昇格する:

```yaml
# .github/workflows/release.yml
jobs:
  build-windows:
    runs-on: windows-latest
    # allow-failure を削除 → required になる
    steps:
      - uses: actions/checkout@v4
      - name: Install vcpkg deps
        run: vcpkg install bdwgc:x64-windows
      - name: Build
        run: cargo build --release --target x86_64-pc-windows-msvc
      - name: Test
        run: cargo test --release --target x86_64-pc-windows-msvc
```

Windows ジョブが失敗するとリリース全体がブロックされる。

## Alternatives considered

### A. MinGW GNU ABI (x86_64-pc-windows-gnu)

Rust の MinGW ターゲット + MinGW-w64 の GCC でビルドする。

**却下**: vcpkg `x64-windows` triplet は MSVC ABI を前提とする。
bdwgc の MinGW ビルドは公式サポートが薄く、`gc.dll` の ABI が MSVC と非互換になる。
`lld-link` (COFF リンカ) と `ld.lld` (ELF リンカ) の混在を避けるため採択しない。

### B. Cygwin / MSYS2 上でのビルド

Unix 互換レイヤー (Cygwin/MSYS2) を使い、POSIX API をそのまま利用する。

**却下**: Cygwin は `cygwin1.dll` への依存が発生し、配布が複雑になる。
native Windows バイナリ (MSVC ABI) を提供することがユーザーリーチの点で優れる。

### C. MSVC ABI (本案)

vcpkg + `clang-cl` + `lld-link` + platform 抽象化。

**採択。**

## Consequences

**Positive**

- Windows ユーザーが `tyra.exe` + `gc.dll` を展開するだけで使用できるようになる
- CI の Windows ジョブが required になり、Windows 対応の退行を防止できる
- `platform/unix.rs` / `platform/windows.rs` の分離により将来の OS 追加が容易になる
- vcpkg + MSVC ABI は Windows エコシステムで最も標準的な構成

**Negative / accepted tradeoffs**

- **native PDB デバッグ情報**: v0.8 では LLVM の `.pdb` 生成は対応しない。v0.9+ で対応予定
- **Windows ARM64**: ARM64 ターゲット (`aarch64-pc-windows-msvc`) は v0.9+ に持ち越す
- **MSVC `link.exe` (native linker)**: v0.8 では `lld-link` を使用する。`link.exe` への対応は v0.9+
- **CI コスト**: Windows CI ジョブが required になるため、PR ごとのビルド時間が増加する
- **vcpkg セットアップ**: CI / 開発環境への vcpkg インストールが必要になる

**実装順序**

1. CI: `windows-latest` ジョブを `allow-failure: false` で追加し、vcpkg セットアップを組み込む
2. `runtime/src/platform/` ディレクトリを作成し、`unix.rs` / `windows.rs` に分離
3. `build.rs` に llvm-config 探索ロジックを追加
4. `linker.rs` に `build_link_cmd_windows` を追加
5. `tyra build` に `copy_runtime_dlls` を追加
6. Windows 上でのランタイムテスト全件通過を確認
7. リリーススクリプトに `tyra-<version>-windows-x86_64.zip` 生成を追加
