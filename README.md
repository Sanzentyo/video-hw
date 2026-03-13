# video-hw

`video-hw` は、複数のハードウェア backend（VideoToolbox / NVIDIA / Intel oneVPL）を同一 API で扱う workspace 構成のライブラリ群です。

## 主要構成

```text
crates/
  video-hw-core/      # 共通型・エラー・契約（公開core crate）
  video-hw/           # facade + backend 実装（公開crate）
sample-videos/        # E2E/bench 入力素材
scripts/              # 補助スクリプト
```

## feature / platform 切替

- デフォルト: なし（`default = []`）
- macOS は `backend-vt` を有効化
- Linux/Windows は `backend-nvidia` または `backend-intel` を有効化
- 実行時は `Backend` を選択（`Backend::Auto` で OS 既定を自動選択）

### 利用側 Cargo.toml（推奨, git rev 固定）

```toml
[target.'cfg(target_os = "macos")'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-vt"] }

[target.'cfg(any(target_os = "linux", target_os = "windows"))'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-nvidia"] } # or ["backend-intel"]
```

## 現行APIの重要制約

- decode 出力型は `DecodedFrame::{Metadata,Nv12,Rgb24}` を持つ
  - ただし標準 decode 経路の出力は `Metadata` 中心
- encode 入力型は `RawFrameBuffer::{Argb8888,Argb8888Shared,Nv12,Rgb24}` を持つ
  - ただし現行 encode が受理するのは `Argb8888` / `Argb8888Shared` のみ
  - `Nv12` / `Rgb24` は `BackendError::InvalidInput`
- `reap_timeout` は現行実装では `try_reap` と同挙動（実質 non-blocking）

## NVIDIA backend 依存

`backend-nvidia` では次の依存を使用します。

- `nvidia-video-codec-sdk`
  - `git = "https://github.com/Sanzentyo/nvidia-video-codec-sdk"`
  - `rev = "d2d0fec631365106d26adfe462f3ce15b043b879"`
- `cudarc = 0.19.2`

`nvidia-video-codec-sdk` は Rust から NVIDIA Video Codec SDK を扱うためのラッパー層です。
SDK 本体（lib/headers）は同梱しない前提で、利用者側が別途 NVIDIA から取得して配置する必要があります。

### NVIDIA Video Codec SDK ビルド前提（Windows）

```powershell
$env:NVIDIA_VIDEO_CODEC_SDK_PATH = "C:\Path\To\Video_Codec_SDK\Lib\x64"
```

`NVIDIA_VIDEO_CODEC_SDK_PATH` は `nvEncodeAPI.lib` / `nvcuvid.lib` を含むディレクトリを指します。

## Intel backend 依存

`backend-intel` は Intel oneVPL を Rust から扱う `onevpl-rs` を利用します（`intel-onevpl-sys` 経由で oneVPL 公式ヘッダにバインド）。  
現状は H.264 / HEVC の encode/decode をサポートします（`require_hardware=false` の場合は software 実装へフォールバック）。

### oneVPL 導入（CLI / Windows）

管理者 PowerShell で実行してください。

```powershell
# 1) Base Toolkit（既に導入済みならスキップ可）
winget install -e --id Intel.OneAPI.BaseToolkit --accept-package-agreements --accept-source-agreements

# 2) oneVPL standalone package（Intel公式）
# https://www.intel.com/content/www/us/en/developer/articles/tool/oneapi-standalone-components.html#onevpl
# 例: w_oneVPL_p_<version>_offline.exe を取得

# 3) standalone exe から product-id / product-ver を確認
.\w_oneVPL_p_<version>_offline.exe -a --list-products
.\w_oneVPL_p_<version>_offline.exe -a --list-components --product-id <PRODUCT_ID> --product-ver <PRODUCT_VER>

# 4) standalone exe でサイレント導入
.\w_oneVPL_p_<version>_offline.exe -a --silent --eula accept --action install --product-id <PRODUCT_ID> --product-ver <PRODUCT_VER> --components default
```

standalone パッケージを展開済みで `packages` ディレクトリがある場合は、同梱 `installer.exe` で product/component を確認して導入できます。

```powershell
$installer = "C:\Program Files (x86)\Intel\oneAPI\Installer\installer.exe"
$pkg = "C:\path\to\w_oneVPL_p_<version>_offline\packages"

& $installer --package-path $pkg --list-products
# 出力に出た <PRODUCT_ID> / <PRODUCT_VER> を使う
& $installer --package-path $pkg --list-components --product-id <PRODUCT_ID> --product-ver <PRODUCT_VER>
& $installer -s --eula accept --action install --package-path $pkg --product-id <PRODUCT_ID> --product-ver <PRODUCT_VER> --components default
```

導入後は必要に応じて再起動してください（ログに reboot 要求が出る場合があります）。

導入後、次のファイルが存在することを確認します。

```powershell
Get-Item "C:\Program Files (x86)\Intel\oneAPI\vpl\latest\include\vpl\mfx.h"
Get-Item "C:\Program Files (x86)\Intel\oneAPI\vpl\latest\lib\vpl.lib"
```

存在確認後、環境変数を設定します。

```powershell
$env:LIBVPL_INCLUDE_PATH = "C:\Program Files (x86)\Intel\oneAPI\vpl\latest\include\vpl"
$env:LIBVPL_LIBRARY_PATH = "C:\Program Files (x86)\Intel\oneAPI\vpl\latest\lib"
```

必要に応じて `LIBCLANG_PATH` も設定してください（bindgen が `libclang.dll` を見つけられない場合）。

`vpl\latest` が生成されない環境では、公式ソース `intel/libvpl` から oneVPL dispatcher をビルドして補完できます（CLIで再現確認済み）。

```powershell
git clone --depth 1 https://github.com/intel/libvpl.git $env:TEMP\libvpl
cmake -S $env:TEMP\libvpl -B $env:TEMP\libvpl\build -DCMAKE_INSTALL_PREFIX=$env:TEMP\libvpl\install
cmake --build $env:TEMP\libvpl\build --config Release --target install

Get-Item "$env:TEMP\libvpl\install\include\vpl\mfx.h"
Get-Item "$env:TEMP\libvpl\install\lib\vpl.lib"

$env:LIBVPL_INCLUDE_PATH = "$env:TEMP\libvpl\install\include\vpl"
$env:LIBVPL_LIBRARY_PATH = "$env:TEMP\libvpl\install\lib"
$env:Path = "$env:TEMP\libvpl\install\bin;$env:Path"
```

同等手順は Cargo Script でも実行できます（既定 dry-run）。

```bash
cargo +nightly -Zscript scripts/setup_onevpl.rs
cargo +nightly -Zscript scripts/setup_onevpl.rs --apply
```

> `onevpl-sys` 側の `build.rs` が依存解決時に先行実行されるため、本リポジトリでは `build.rs` による oneVPL 自動取得/自動ビルドは行っていません。上記の事前 CLI セットアップを前提にしています。

この fallback 設定後は、次で Intel backend のビルド検証ができます。

```powershell
cargo clippy --workspace --all-targets --features backend-intel
cargo test --workspace --features backend-intel -- --nocapture
```

### Intel backend トラブルシューティング

- `Unable to generate bindings: NotExist(...\\mfx.h)`  
  `LIBVPL_INCLUDE_PATH` が誤っているか、oneVPL ヘッダ未導入です。`mfx.h` の実在パスを再確認してください。
- `LINK : fatal error LNK1181: cannot open input file 'vpl.lib'`  
  `vpl.lib` が未導入、または `LIBVPL_LIBRARY_PATH` が不正です。
- `Loader::new_session: NotFound`  
  oneVPL runtime/ドライバが未導入、または再起動未実施の可能性があります。導入後に再起動して再試行してください。
- `unsupported config: Intel hardware encoder rejected ... (Session::encoder: InvalidVideoParam)`  
  oneVPL runtime 側で要求した encode パラメータ（実装種別/色形式/メモリ種別）が受理されていません。環境によっては HEVC のみ利用可能で、H.264 hardware encode が提供されない場合があります。  
  Intel GPU runtime / ドライバ更新後に再試行し、ベンチでは必要に応じて `--codec hevc` / `--require-hardware false` / `--allow-case-failures` を利用してください。

## ライセンス

- このプロジェクトは `MIT OR Apache-2.0` のデュアルライセンス
- 詳細は `LICENSE-MIT` / `LICENSE-APACHE` / `NOTICE` を参照
- 依存ライセンスと注意事項は `THIRD_PARTY_NOTICES.md` を参照
- NVIDIA SDK の配布運用ルールは `docs/spec/NVIDIA_SDK_DISTRIBUTION_POLICY.md` を参照

## 検証コマンド

```bash
cargo fmt --all -- --check
cargo test --workspace -- --nocapture
cargo clippy --workspace --all-targets
cargo clippy --workspace --all-targets --features backend-nvidia
cargo test --workspace --features backend-nvidia -- --nocapture
cargo clippy --workspace --all-targets --features backend-intel
cargo test --workspace --features backend-intel -- --nocapture
cargo test --workspace --all-features -- --nocapture
cargo bench --package video-hw --features backend-nvidia --bench decode_bench -- --noplot
cargo deny check licenses advisories bans sources
```

## 実行例

```bash
# decode
cargo run --example decode_annexb -- --backend auto --codec h264 --input sample-videos/sample-10s.h264 --chunk-bytes 4096 --require-hardware

# decode (Intel)
cargo run --features backend-intel --example decode_annexb -- --backend intel --codec h264 --input sample-videos/sample-10s.h264 --chunk-bytes 4096 --require-hardware

# encode
cargo run --features backend-nvidia --example encode_synthetic -- --backend nv --codec h264 --fps 30 --frame-count 300 --require-hardware --output output/video-hw-h264.bin

# encode (Intel)
cargo run --features backend-intel --example encode_synthetic -- --backend intel --codec h264 --fps 30 --frame-count 300 --require-hardware --output output/video-hw-intel-h264.bin

# precise benchmark (Intel vs ffmpeg QSV)
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec h264 --release --warmup 2 --repeat 9 --require-hardware true
# 失敗ケースも記録して継続
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec h264 --release --warmup 1 --repeat 3 --require-hardware true --allow-case-failures
```

## ドキュメント

- インデックス: `docs/README.md`
- 利用ガイド: `docs/USAGE_STRICT.md`
- I/O 契約: `docs/spec/IO_FORMAT_CONTRACT.md`
- テスト台帳: `docs/spec/TEST_SPEC_INVENTORY.md`
- 状態: `docs/status/STATUS.md`
- 計画: `docs/plan/ROADMAP.md`
- 次アクション: `docs/plan/NEXT_ACTION_PLAN_2026-02-23.md`
