# video-hw 利用ガイド（厳密 I/O 仕様, 現行実装準拠）

この文書は `DecodeSession` / `EncodeSession` の現行APIを、実装準拠で使うためのガイドです。

## 1. 導入

- macOS: `backend-vt`
- Linux/Windows: `backend-nvidia` or `backend-intel`
- `default = []`

```toml
[target.'cfg(target_os = "macos")'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-vt"] }

[target.'cfg(any(target_os = "linux", target_os = "windows"))'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-nvidia"] } # or ["backend-intel"]
```

## 2. Backend 選択

- `Backend::Auto`
- `Backend::VideoToolbox`（macOS + `backend-vt`）
- `Backend::Nvidia`（Linux/Windows + `backend-nvidia`）
- `Backend::Intel`（Linux/Windows + `backend-intel`）

`Backend::Auto` は OS 既定 backend を選択します。

### 2.1 NVIDIA backend の前提（重要）

- `backend-nvidia` は `nvidia-video-codec-sdk`（Rust bindings / ラッパー）を通じて NVIDIA SDK を利用する
- SDK 本体（lib/headers）は同梱しない前提で、利用者が NVIDIA から別途取得して配置する
- ビルド時は環境に応じて `NVIDIA_VIDEO_CODEC_SDK_PATH` などの設定が必要になる

### 2.2 Intel backend の前提（重要）

- `backend-intel` は `onevpl-rs`（`intel-onevpl-sys` 経由）で Intel oneVPL を利用する
- 依存宣言は `https://github.com/Sanzentyo/onevpl-rs` を `rev` 固定で参照する（fork 更新時は README の「onevpl fork 更新時の手順」に従う）
- 現行 Intel backend は H.264 / HEVC の encode/decode 実装を持つ（実際に使える codec は oneVPL runtime / ドライバの公開機能に依存）
- `require_hardware=false` は HW優先で初期化し、失敗時に software 実装フォールバックを試行する
- software 実装を明示的に選びたい場合は `IntelDecoderOptions::force_software=true` / `IntelEncoderOptions::force_software=true`（CLI は `--intel-force-software`）を使う
- oneVPL 本体（runtime/headers）は同梱しない前提で、利用者が Intel oneAPI から別途取得して配置する
- Base Toolkit 単体インストールでは oneVPL が未導入な場合があるため、必要なら `oneapi-standalone-components` の oneVPL を追加導入する
- CLI 導入時は管理者 PowerShell で `w_oneVPL_p_<version>_offline.exe -a --list-products` → `--list-components` で `product-id` / `product-ver` を確認し、`-a --silent --eula accept --action install` を実行する
- もしくは `installer.exe --package-path <...\\packages> --list-products` / `--list-components` で `product-id` と `product-ver` を確認し、`--action install` で導入する
- `intel-onevpl-sys` は `mfx.h` 未検出時に pregenerated bindings へフォールバックする（bindgen 再生成したい場合のみ `LIBVPL_INCLUDE_PATH` と必要なら `LIBCLANG_PATH` を設定）
- runtime 解決のため `libvpl.dll` が `PATH` から見えることを確認する
- `vpl\latest` が作られない場合は `intel/libvpl` を clone し、`cmake --build ... --target install` で `mfx.h` / DLL を生成して補完できる
- 同等手順は `cargo +nightly -Zscript scripts/setup_onevpl.rs --apply` でも実行できる（`--apply` なしは dry-run）
- Intel/ffmpeg 比較ベンチは `cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec h264 --release --warmup 2 --repeat 9 --require-hardware true`（HEVC は `--codec hevc`）
- software 比較は `--require-hardware false --intel-force-software` を付ける（video-hw Intel SW と ffmpeg SW を比較）
- `--equal-raw-input` を使う場合、`--raw-input-pix-fmt argb|nv12` で video-hw/ffmpeg 双方の raw 入力 pix_fmt を揃えられる
- decode 側の計測窓を広げる場合は `--decode-loops <N>` を指定する（入力 Annex-B を N 回連結したファイルを使って decode 比較）
- NV12 raw encode を使う場合は `backend-intel` に加えて `unstable-raw-inputs` feature を有効化する
- runtime 依存で一部ケースが失敗する環境では `--allow-case-failures` を付けると失敗ケースを記録してレポート生成を継続できる
- decode チューニングが必要な場合は `VIDEO_HW_INTEL_DECODE_ASYNC_DEPTH`（1..=16, default=10）で oneVPL decode async depth を調整できる
- benchmark script からは `--intel-decode-async-depth <N>`（1..=16）で同設定を video-hw decode ケースに注入できる
- Intel precise benchmark の既定値は `--warmup 2` / `--decode-loops 10`（揺れが大きい環境での判定安定化のため）
- encode レート制御は `VIDEO_HW_INTEL_RATE_CONTROL`（`cbr|vbr|cqp|avbr|icq|qvbr`）で上書きでき、未指定時は H.264=CBR / HEVC=CQP を使う（`CQP` の既定量子化パラメータは `VIDEO_HW_INTEL_CQP=24`）
- 計測揺れが大きい環境では `--settle-ms 300` 前後を併用すると parity 判定が安定しやすい
- `build.rs` で oneVPL を自動取得/自動ビルドする方式は採用しない（依存 `onevpl-sys` の `build.rs` が先に実行されるため）
- oneVPL導入後は再起動が必要な場合がある（インストーラログに reboot 要求が出た場合）

#### Intel backend トラブルシュート（Windows）

- `Unable to generate bindings: NotExist(...\\mfx.h)`  
  通常は pregenerated bindings へフォールバックする。bindgen 再生成を使いたい場合のみ `LIBVPL_INCLUDE_PATH` とヘッダ実体を確認する
- `Loader::new_session: NotFound`  
  oneVPL runtime/ドライバ未導入、または再起動未実施
- `unsupported config: Intel hardware encoder rejected ... (Session::encoder: InvalidVideoParam)`  
  oneVPL runtime 側が要求 encode パラメータを受理できていない。  
  H.264 では `FrameInfo.PicStruct` 未設定でも同エラーが発生し得るため、現行 backend は `PicStruct::Progressive` を明示して初期化する。  
  それでも失敗する場合は Intel GPU runtime / ドライバ更新後に再試行し、ベンチでは `--codec hevc` / `--require-hardware false` / `--allow-case-failures` を併用する
- `Intel ... cannot use require_hardware=true together with ...force_software=true`  
  `--require-hardware` と `--intel-force-software` は同時指定できない。片方だけを指定する

## 3. Decode API

- `DecodeSession::new(Backend, DecoderConfig)`
- `submit(BitstreamInput)`
- `try_reap()`
- `reap_timeout(Duration)`
- `flush()`
- `summary()`
- `query_capability(Codec)`

### 3.1 Decode 入力

- `BitstreamInput::AnnexBChunk`
- `BitstreamInput::AccessUnitRawNal`
- `BitstreamInput::LengthPrefixedSample`

### 3.2 Decode 出力（重要）

`DecoderConfig.output_mode` で decode 出力モードを指定できます。

- `DecodeOutputMode::Metadata`（既定）
- `DecodeOutputMode::Nv12`
- `DecodeOutputMode::Rgb24`

`DecodedFrame` は次の variant を持ちます。

- `Metadata`
- `Nv12`
- `Rgb24`

`DecodeOutputMode::Metadata` は常用サポートです。  
`DecodeOutputMode::Nv12` / `Rgb24` は backend が ARGB payload を返す場合のみ変換出力できます。  
ARGB payload が未提供の場合は `BackendError::UnsupportedConfig` を返します。

## 4. Encode API

- `EncodeSession::new(Backend, EncoderConfig)`
- `submit(EncodeFrame)`
- `try_reap()`
- `reap_timeout(Duration)`
- `flush()`
- `query_capability(Codec)`
- `request_session_switch(SessionSwitchRequest)`

### 4.1 Encode 入力（重要）

`RawFrameBuffer` は次を持ちます。

- `Argb8888(Vec<u8>)`
- `Argb8888Shared(Arc<[u8]>)`
- `Nv12 { .. }`（`unstable-raw-inputs` feature 有効時のみ）
- `Rgb24(Vec<u8>)`（`unstable-raw-inputs` feature 有効時のみ）

現行 encode が受理するのは `Argb8888` / `Argb8888Shared` のみです。

- `Nv12` / `Rgb24` は `BackendError::InvalidInput`
- ARGB 長さは厳密に `width * height * 4`

### 4.2 Encode 出力 layout

- VT + H264: `EncodedLayout::Avcc`
- VT + HEVC: `EncodedLayout::Hvcc`
- NV: `EncodedLayout::AnnexB`
- Intel: `EncodedLayout::AnnexB`

## 5. submit / reap / flush の意味

- `submit`: 入力投入
- `try_reap`: non-blocking 回収
- `reap_timeout`: timeout 上限まで待機して回収（内部キュー + backend poll）
- `flush`: EOS/遅延分回収

推奨ループは「`submit` -> `try_reap` 回収 -> 最後に `flush`」です。

## 6. 失敗時の見方

- `UnsupportedConfig`: backend/環境依存で利用不可
- `InvalidInput`: 入力不正（未対応 buffer, payload size mismatch）
- `InvalidBitstream`: bitstream 形式不正
- `TemporaryBackpressure`: 一時飽和
- `DeviceLost`: デバイスロスト
- `Backend`: backend 内部エラー

補足:
- `BackendError::kind()` でエラー種別を取得できる
- `BackendError::is_runtime_unavailable()` は `UnsupportedConfig` / `DeviceLost` を runtime unavailable として判定する

## 7. 最小検証コマンド

```bash
cargo test -- --nocapture
cargo test --features backend-nvidia -- --nocapture
cargo check --all-targets --features backend-nvidia
```

## 8. 関連

- `docs/spec/IO_FORMAT_CONTRACT.md`
- `docs/spec/TEST_SPEC_INVENTORY.md`
- `docs/status/STATUS.md`
