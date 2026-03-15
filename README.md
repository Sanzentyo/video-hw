# video-hw

`video-hw` は、複数のハードウェア backend（VideoToolbox / NVIDIA / Intel oneVPL / Vulkan）を同一 API で扱う workspace 構成のライブラリ群です。

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
- Linux/Windows は `backend-nvidia` / `backend-intel` / `backend-vulkan` のいずれかを有効化
- 実行時は `Backend` を選択（`Backend::Auto` で OS 既定を自動選択）

### 利用側 Cargo.toml（推奨, git rev 固定）

```toml
[target.'cfg(target_os = "macos")'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-vt"] }

[target.'cfg(any(target_os = "linux", target_os = "windows"))'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-nvidia"] } # or ["backend-intel"] / ["backend-vulkan"]
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
依存宣言は `https://github.com/Sanzentyo/onevpl-rs` を参照し、`rev` 固定で利用しています。  
現状は H.264 / HEVC の encode/decode をサポートします。`require_hardware=false` は「HW優先で初期化し、失敗時にSWへフォールバック」です。  
SW を明示的に使う場合は `IntelDecoderOptions::force_software=true` / `IntelEncoderOptions::force_software=true`（CLI では `--intel-force-software`）を利用してください。
Intel encode のレート制御は `VIDEO_HW_INTEL_RATE_CONTROL`（`cbr|vbr|cqp|avbr|icq|qvbr`）で上書きできます。未指定時は H.264=CBR、HEVC=CQP を使います（CQP 値は `VIDEO_HW_INTEL_CQP`, default=24）。

#### onevpl fork 更新時の手順

1. `https://github.com/Sanzentyo/onevpl-rs` に `third_party/onevpl-rs` 相当の変更を反映する  
2. 反映した commit SHA を `crates/video-hw/Cargo.toml` の `onevpl` 依存へ `rev = "<sha>"` として固定する  
3. `cargo update -p onevpl && cargo update -p intel-onevpl-sys` で lockfile を更新する  
4. `cargo fmt --check && cargo clippy --workspace --all-targets --all-features && cargo test --workspace --all-features && cargo bench --package video-hw --features backend-nvidia --bench decode_bench -- --noplot` を再実行する

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
Get-Item "C:\Program Files (x86)\Intel\oneAPI\vpl\latest\bin\libvpl.dll"
```

存在確認後、環境変数を設定します。

```powershell
$env:LIBVPL_INCLUDE_PATH = "C:\Program Files (x86)\Intel\oneAPI\vpl\latest\include\vpl"
$env:Path = "C:\Program Files (x86)\Intel\oneAPI\vpl\latest\bin;$env:Path"
```

必要に応じて `LIBCLANG_PATH` も設定してください（bindgen が `libclang.dll` を見つけられない場合）。

`vpl\latest` が生成されない環境では、公式ソース `intel/libvpl` から oneVPL dispatcher をビルドして補完できます（CLIで再現確認済み）。

```powershell
git clone --depth 1 https://github.com/intel/libvpl.git $env:TEMP\libvpl
cmake -S $env:TEMP\libvpl -B $env:TEMP\libvpl\build -DCMAKE_INSTALL_PREFIX=$env:TEMP\libvpl\install
cmake --build $env:TEMP\libvpl\build --config Release --target install

Get-Item "$env:TEMP\libvpl\install\include\vpl\mfx.h"
Get-Item "$env:TEMP\libvpl\install\bin\libvpl.dll"

$env:LIBVPL_INCLUDE_PATH = "$env:TEMP\libvpl\install\include\vpl"
$env:Path = "$env:TEMP\libvpl\install\bin;$env:Path"
```

同等手順は Cargo Script でも実行できます（既定 dry-run）。

```bash
cargo +nightly -Zscript scripts/setup_onevpl.rs
cargo +nightly -Zscript scripts/setup_onevpl.rs --apply
```

> `intel-onevpl-sys` の `build.rs` は `mfx.h` が見つからない場合、同梱の pregenerated bindings へ自動フォールバックします。`LIBVPL_INCLUDE_PATH` は「bindgen で再生成したい場合」に設定してください。

この fallback 設定後は、次で Intel backend のビルド検証ができます。

```powershell
cargo clippy --workspace --all-targets --features backend-intel
cargo test --workspace --features backend-intel -- --nocapture
```

### Intel backend トラブルシューティング

- `Unable to generate bindings: NotExist(...\\mfx.h)`  
  通常は pregenerated bindings へフォールバックします。bindgen 再生成を使いたい場合のみ `LIBVPL_INCLUDE_PATH` と `mfx.h` 実体を確認してください。
- `Loader::new_session: NotFound`  
  oneVPL runtime/ドライバが未導入、または再起動未実施の可能性があります。導入後に再起動して再試行してください。
- `unsupported config: Intel hardware encoder rejected ... (Session::encoder: InvalidVideoParam)`  
  oneVPL runtime 側で要求した encode パラメータ（実装種別/色形式/メモリ種別）が受理されていません。  
  H.264 の場合は `FrameInfo.PicStruct` 未設定でも同エラーになり得るため、現行 backend は `PicStruct::Progressive` を明示して初期化します。  
  それでも失敗する場合は Intel GPU runtime / ドライバ更新後に再試行し、ベンチでは必要に応じて `--codec hevc` / `--require-hardware false` / `--allow-case-failures` を利用してください。

## Vulkan backend 依存

`backend-vulkan` は Rust から Vulkan Video API を直接利用します（`vk-video` + `ash`）。  
現行実装は **H.264 の decode/encode が主経路** で、HEVC decode は ash-level の submit execution probe（GPU 実行確認）と Annex-B access-unit 数の推定を組み合わせる実験段階です。`DecodeOutputMode::Metadata` では access-unit 推定数に対応するメタデータ frame を返し、非 metadata モード（`Nv12` / `Rgb24`）では submit probe の NV12 readback を access-unit 単位で回収して ARGB frame を返します（フルストリーム HEVC decode/encode は `UnsupportedConfig`）。

- `require_hardware=true` では Vulkan 実行を必須とし、利用不可時は `UnsupportedConfig` を返します。
- `require_hardware=false` でも direct Vulkan backend に software fallback はありません（`Vulkan*Options::allow_software_fallback` は現時点では実質未対応）。
- Vulkan loader/driver が `VK_KHR_video_queue` + H.264 decode/encode 拡張を提供している必要があります。
- HEVC 直対応は調査中です。現在採用している `vk-video 0.2.1` が H.264 API のみ公開しているため、HEVC 実装には `VK_KHR_video_decode_h265` / `VK_KHR_video_encode_h265` を使う ash レベルの新規パスが必要です。
- HEVC decode 着手として、unsafe な Vulkan FFI 呼び出しを `vulkan_hevc_decode` モジュールに隔離し、上位 API からは安全な probe 結果（enum）だけを扱う境界にしています。
- 上記 probe は拡張有無だけでなく、`VIDEO_DECODE_KHR` queue family と最小 logical-device 初期化まで検証し、失敗理由を `UnsupportedConfig` へ反映します。
- HEVC Annex-B の VPS/SPS/PPS 抽出と SPS 由来の解像度解析は `scuffle-h265` で実装済みで、decode 未実装時の診断メッセージにパラメータセット状態を反映します。
- さらに bitstream が与えられた decode パスでは、HEVC profile の capability / output-format query に加えて、報告された output format 候補を順に試しながら `vkCreateVideoSessionKHR` / `vkCreateVideoSessionParametersKHR` の作成 probe を実行し、SPS 解像度チェック結果と合わせて blocker message に追記します。
- `vkCreateVideoSessionParametersKHR` probe では抽出した VPS/SPS/PPS を `StdVideoH265VideoParameterSet` / `StdVideoH265SequenceParameterSet` / `StdVideoH265PictureParameterSet` へ変換して `VideoDecodeH265SessionParametersAddInfoKHR` に投入します（ID・解像度・DPB・短期/長期参照セットなどの基本項目を反映）。
- 現時点では PPS の `pps_scaling_list_data_present_flag=1` と `pps_extension_present_flag=1` を未対応として明示エラーにし、失敗理由が診断メッセージで分かるようにしています。
- 上記 probe が成功した場合は decode submit/reap の次段実装向けに DPB slot / reference slot の計画骨組み（decode submit skeleton）も生成し、先頭 VCL slice header（NAL type / PPS id / POC LSB）解析結果と合わせて blocker message に `decode_submit_skeleton=...` として出力します。
- さらに submit 実行前提として、`vkGetVideoSessionMemoryRequirementsKHR` / `vkBindVideoSessionMemoryKHR` / decode source buffer 準備 / `vkCmdBeginVideoCodingKHR`→`vkCmdDecodeVideoKHR`→`vkCmdEndVideoCodingKHR` の録画・submit・fence wait に加え、decode 出力 image を `vkCmdCopyImageToBuffer` で readback buffer へコピーし `vkMapMemory` で回収確認する probe を追加し、`decode_submit_execution=...` で可否を診断します。
- 実験的 DPB 経路は `VIDEO_HW_VULKAN_HEVC_EXPERIMENTAL_DPB`（`off`/`auto`/`on`）で制御され、`auto` では `%TEMP%\\video-hw-vulkan-hevc-dpb-inflight.flag` 残留時に安全側へ自動抑止します。blocker message の `decode_submit_execution=ready(...)` には `experimental_dpb_mode` / `experimental_dpb_status` に加えて `readback_bytes` / `readback_planes` / `readback_sample_stride` / `readback_sample_count` も含め、DPB 判定理由と readback handoff 状態を追跡できるようにしています。
- 非 metadata の HEVC 出力では submit probe の access-unit 上限を stream 長へ拡張して full coverage を要求します。`decode_submit_execution=ready(...)` の `submitted_access_units` が足りない場合は `UnsupportedConfig` を返します。残課題は DPB/reference-slot を有効化した広範囲ストリームでの安定化です。

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
cargo clippy --workspace --all-targets --features backend-vulkan
cargo test --workspace --features backend-vulkan -- --nocapture
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

# decode (Intel software)
cargo run --features backend-intel --example decode_annexb -- --backend intel --codec h264 --input sample-videos/sample-10s.h264 --chunk-bytes 4096 --intel-force-software

# decode (Vulkan)
cargo run --features backend-vulkan --example decode_annexb -- --backend vulkan --codec h264 --input sample-videos/sample-10s.h264 --chunk-bytes 4096 --require-hardware

# encode
cargo run --features backend-nvidia --example encode_synthetic -- --backend nv --codec h264 --fps 30 --frame-count 300 --require-hardware --output output/video-hw-h264.bin

# encode (Intel)
cargo run --features backend-intel --example encode_synthetic -- --backend intel --codec h264 --fps 30 --frame-count 300 --require-hardware --output output/video-hw-intel-h264.bin

# encode (Intel software)
cargo run --features backend-intel --example encode_synthetic -- --backend intel --codec h264 --fps 30 --frame-count 300 --intel-force-software --output output/video-hw-intel-sw-h264.bin

# encode (Vulkan)
cargo run --features backend-vulkan --example encode_synthetic -- --backend vulkan --codec h264 --fps 30 --frame-count 300 --require-hardware --output output/video-hw-vulkan-h264.bin

# encode raw (Intel NV12 input, unstable-raw-inputs)
cargo run --features "backend-intel unstable-raw-inputs" --example encode_raw_argb -- --backend intel --codec hevc --fps 30 --frame-count 300 --width 640 --height 360 --input-raw output/benchmark-input-nv12-640x360-300f.raw --input-pix-fmt nv12 --require-hardware --output output/video-hw-intel-hevc-nv12.bin

# precise benchmark (Intel vs ffmpeg QSV)
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec h264 --release --warmup 2 --repeat 9 --require-hardware true
# precise benchmark (Intel software vs ffmpeg software)
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec h264 --release --warmup 2 --repeat 9 --require-hardware false --intel-force-software
# precise benchmark (equal raw NV12 input)
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec hevc --release --warmup 2 --repeat 9 --require-hardware true --equal-raw-input --raw-input-pix-fmt nv12
# precise benchmark (decode計測窓を拡張)
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec hevc --release --warmup 2 --repeat 9 --require-hardware true --equal-raw-input --raw-input-pix-fmt nv12 --decode-loops 3
# precise benchmark (揺れを抑える推奨設定: settle + decode async depth)
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec hevc --release --warmup 1 --repeat 3 --require-hardware true --equal-raw-input --raw-input-pix-fmt nv12 --decode-loops 10 --settle-ms 300 --intel-decode-async-depth 8
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
