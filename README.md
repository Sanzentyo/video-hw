# video-hw

`video-hw` は、複数のハードウェア backend（VideoToolbox / NVIDIA / Intel oneVPL / Vulkan）を同一 API で扱う workspace 構成のライブラリ群です。

## 主要構成

```text
crates/
  video-hw-core/      # 共通型・エラー・契約（公開core crate）
  video-hw/           # facade + backend選択/セッションAPI（公開crate）
  video-hw-backend-nvidia/ # NVIDIA backend 実装crate
  video-hw-backend-intel/  # Intel backend 実装crate
  video-hw-backend-vulkan/ # Vulkan backend 実装crate
  video-hw-backend-vt/     # VideoToolbox backend 実装crate
sample-videos/        # E2E/bench 入力素材
scripts/              # 補助スクリプト
```

## feature / platform 切替

- デフォルト: なし（`default = []`）
- macOS は `backend-vt` を有効化
- Linux/Windows は `backend-nvidia` / `backend-intel` / `backend-vulkan` のいずれかを有効化
- backend 実装は static generic 前提（`DecodeSession::<...>::new(...)` / `EncodeSession::<...>::new(...)`）
- セッション API は static-only。`DecodeSession::<Adapter>::new(...)` / `EncodeSession::<Adapter>::new(...)` を利用する
- `Backend::Auto` は wrapper/example 側で concrete adapter を選ぶ用途に限定し、セッション本体は concrete adapter で生成する
- Auto 相当の選択は `Backend::resolve_decoder` / `Backend::resolve_encoder`（または `select_decoder_backend` / `select_encoder_backend`）で concrete `BackendKind` を取得して実行する

### 利用側 Cargo.toml（推奨, git rev 固定）

```toml
[target.'cfg(target_os = "macos")'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-vt"] }

[target.'cfg(any(target_os = "linux", target_os = "windows"))'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-nvidia"] } # or ["backend-intel"] / ["backend-vulkan"]
```

### 分割backend crate（video-hw が内部で読込）

`video-hw` は feature 有効化時に、対応する backend 実装crate
（`video-hw-backend-nvidia/intel/vulkan/vt`）を依存として読み込みます。  
通常利用では `video-hw` だけ依存追加すれば十分です。

```toml
video-hw-backend-intel = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3" }
video-hw-backend-nvidia = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3" }
video-hw-backend-vulkan = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3" }
video-hw-backend-vt = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3" }
```

上記crateを直接使う場合は adapter 型を受け取り、`DecodeSession::<Adapter>::new(...)` /
`EncodeSession::<Adapter>::new(...)` で静的ディスパッチできます。

## 現行APIの重要制約

- decode 出力型は `DecodedFrame::{Metadata,Nv12,Rgb24}` を持つ
  - NVIDIA decode は NVDEC の mapped surface を CPU NV12 payload として readback し、`Nv12` と `Rgb24` を返せる
  - backend ごとの decode 出力可否は `CapabilityReport::decode_output_modes` で確認する
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
SW を明示的に使う場合は `IntelDecoderOptions::force_software=true` / `IntelEncoderOptions::force_software=true`（CLI では `--intel-force-software`）を利用してください。`IntelEncoderOptions::hevc_use_vpp` を `Some(true)` にすると、HEVC encode で VPP 経路（BGRA/YV12 -> NV12）を明示選択できます。  
Intel encode のレート制御は `VIDEO_HW_INTEL_RATE_CONTROL`（`cbr|vbr|cqp|avbr|icq|qvbr`）で上書きできます。未指定時は H.264=CBR、HEVC=CQP を使います（CQP 値は `VIDEO_HW_INTEL_CQP`, default=24）。encode async depth は `VIDEO_HW_INTEL_ASYNC_DEPTH`（1..=16, default=10）で調整できます。HEVC hardware encode は既定で CPU 側 ARGB→NV12 変換 + `IN_SYSTEM_MEMORY` 投入を優先し、旧来の VPP 経路を強制したい場合は `VIDEO_HW_INTEL_HEVC_USE_VPP=1`、low-power を無効化したい場合は `VIDEO_HW_INTEL_HEVC_LOW_POWER=0` を設定してください。HEVC parity を厳密比較する場合は `--equal-raw-input true --raw-input-pix-fmt nv12` を推奨します（ARGB 入力は環境依存の揺らぎで ±10% を外れることがあります）。

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
現行実装は **H.264 の decode/encode** と **HEVC decode** に対応します。HEVC decode は ash-level の `VK_KHR_video_decode_h265` submit path を使い、DPB 参照付きの full-stream readback を既定で有効にします。`DecodeOutputMode::Metadata` では access-unit 推定数に対応するメタデータ frame を返し、非 metadata モード（`Nv12` / `Rgb24`）では submit probe の NV12 readback を access-unit 単位で回収して ARGB frame を返します。HEVC encode は NVIDIA Vulkan adapter 上で実験的な IDR-only production path を有効化していますが、FFmpeg `hevc_vulkan` で生成した同サイズの parameter/header sample が必要で、現状は参照フレーム/GOP encode と長寿命encoder sessionが未実装のため性能 parity は未達です。

- `require_hardware=true` では Vulkan 実行を必須とし、利用不可時は `UnsupportedConfig` を返します。
- `require_hardware=false` でも direct Vulkan backend に software fallback はありません（`Vulkan*Options::allow_software_fallback` は現時点では実質未対応）。
- Vulkan loader/driver が `VK_KHR_video_queue` と、使う codec に対応した decode/encode 拡張を提供している必要があります。
- HEVC decode は `VK_KHR_video_decode_h265` を直接使う ash-level path です。HEVC encode は `VK_KHR_video_encode_h265` を直接使う実験的 IDR-only path です。1回の `flush` 内では video session / session parameters を複数frameで再利用しますが、参照フレーム/GOP encodeや長寿命encoder sessionは未実装で、性能比較では FFmpeg `hevc_vulkan` より大きく遅れます。
- HEVC encode submit probe の session parameters は既定で `sample-videos/sample-10s.h265` の VPS/SPS/PPS を使用します。切り分け時は `VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SAMPLE_PATH=<Annex-B .h265>` で差し替え可能です（存在しない path は明示エラー、silent fallback なし）。なお encode probe の parameter-set は現状 Main profile（`profile_idc=1`）のみ受理し、非Main（例: `Rext`）は明示エラーで拒否します。`VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_VUI_SAFETY=auto|preserve|force-off` で VUI の扱いを切替できます。`auto` は override sample が VUI を含み `VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_MODE=sample` の場合に実効 mode を `sample-sps-vui-flag-off` へ自動切替して access violation を回避します。FFmpeg `hevc_vulkan` 生成 320x180 parameter sample では `preserve` が必要で、FFmpeg 風 probe 条件では `cmdEncodeVideoKHR` submit が `Ready(bytes_written=47)` まで進むことを確認しています。
- production HEVC encode では、同じ coded size の FFmpeg `hevc_vulkan` Annex-B output から先頭の non-VCL NAL（VPS/SPS/PPS/prefix SEI）を取り出して、driver が返す slice bitstream の前に付与します。統合benchmark runnerは HEVC Vulkan encode時にこの parameter sample をFFmpegで自動生成し、`video-hw` adapter番号とFFmpeg Vulkan adapter番号が異なるhybrid GPU環境でも名前/vendor/device idで対応付けます。
- HEVC encode probe の切り分け mode（`VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_MODE`）は `sample-sps-no-vui` / `sample-sps-vui-flag-off` / `sample-sps-no-vui-flag-on` / `sample-sps-level` / `sample-sps-sub-layer-ordering` / `sample-sps-level-ordering` などを利用可能です。
- 追加切り分けとして `VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_IDX_MODE=minus-one|zero`（`StdVideoEncodeH265ReferenceListsInfo.num_ref_idx_l{0,1}_active_minus1`）と `VIDEO_HW_VULKAN_HEVC_ENCODE_CONTROL_MODE=default|ffmpeg|none`（`cmd_control_video_coding_khr` の rate-control 接続方式）を利用できます。現環境ではいずれも `vkEndCommandBuffer failed` からの改善は確認できていません。
- さらに `VIDEO_HW_VULKAN_HEVC_ENCODE_RPS_MODE=empty-struct|null-pointers`（`pShortTermRefPicSet` / `pLongTermRefPics`）と `VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_LIST_MODE=sentinel|zero|null-pointers`（`pRefLists`）を追加しました。`sample` / `empty-template` のいずれでも `vkEndCommandBuffer failed` は不変で、今回環境ではこれらポインタ有無は主因ではありませんでした。
- 追加で `VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_REFERENCE_SLOT_MODE=slot-minus-one|slot-minus-one-no-codec-info|none` と `VIDEO_HW_VULKAN_HEVC_ENCODE_SETUP_REFERENCE_SLOT_MODE=slot-zero|none` により begin/setup reference slot を個別に切り替えできます。`slot-minus-one-no-codec-info` は FFmpeg 風に begin reference slot の codec-info pNext を外す診断 mode です。`sample` / `empty-template` の8ケースおよび FFmpeg 風 begin slot 追試でも `vkEndCommandBuffer failed` は不変でした。
- FFmpeg parity 追試として、source `VideoPictureResourceInfoKHR.coded_extent` は input image の aligned extent を使うようにし、`VIDEO_HW_VULKAN_HEVC_ENCODE_DST_PREFIX_BYTES=<bytes>` で FFmpeg の sequence header / filler 後 `dstBufferOffset` を模した prefix offset を指定できます。1920x1080 sample probe では `src_picture_resource=1920x1088`、`DST_PREFIX_BYTES=256`、FFmpeg 風 begin slot の単独/併用いずれでも `vkEndCommandBuffer failed` は不変でした。
- FFmpeg と同じく encode image view には identity `VkSamplerYcbcrConversion` を接続します。また encode image memory は `vkGetImageMemoryRequirements2` の dedicated allocation 要求を反映し、`VIDEO_HW_VULKAN_HEVC_ENCODE_DPB_BARRIER_MODE=with|none` で DPB image の明示 barrier を切替可能です。現環境では dedicated allocation は `src:false|dpb:false`、FFmpeg 風 `none` と identity YCbCr view を使っても NVIDIA Vulkan HEVC encode submit はまだ `vkEndCommandBuffer failed` です。
- HEVC encode probe の source image は staging buffer から NV12 plane（Y=16 / UV=128）を `vkCmdCopyBufferToImage` で投入します。以前の clear-only 経路より FFmpeg の `hwupload` 後に実入力 image を encode する形に近い診断です。
- HEVC encode session parameters は device の `VideoEncodeH265CapabilitiesKHR.std_syntax_flags` を diagnostics に出し、SPS の SAO を capability に合わせ、`sps_temporal_mvp_enabled_flag` は FFmpeg と同じく false に寄せます。現環境では `parameter_set_sao=true` / `parameter_set_temporal_mvp=false` まで反映されますが、submit failure は不変です。
- `VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_SLOT_POINTER_MODE=empty-slice|ffmpeg` で、`VkVideoEncodeInfoKHR.referenceSlotCount=0` 時の `pReferenceSlots` を Rust の空 slice pointer と FFmpeg 風 non-null pointer で切替できます。現環境では `ffmpeg` でも `vkEndCommandBuffer failed` は不変でした。
- HEVC encode probe の disabled rate-control fixed QP は FFmpeg の未指定時既定に合わせて 18 です。`VIDEO_HW_VULKAN_HEVC_ENCODE_CONSTANT_QP=<0..51>` で probe 時だけ上書きできます（CBR/VBR では FFmpeg と同じく slice `constant_qp=0`）。
- HEVC encode probe の quality level 既定は FFmpeg の `hevc_vulkan` と同じ 0 です。`VIDEO_HW_VULKAN_HEVC_ENCODE_QUALITY_LEVEL=<n>` で probe 時だけ上書きできます。
- `pre_encode_scope_probe`（resource/barrier + begin/control/end）と `pre_encode_probe`（+ `cmd_encode_video_khr`）の診断を分離しました。現環境では `pre_encode_scope_probe=ok` かつ `pre_encode_probe=failed` となり、失敗点は `cmd_encode_video_khr` を含む recording 内容に局在しています。補助切り分けとして `VIDEO_HW_VULKAN_HEVC_ENCODE_NALU_MODE=single-slice|empty` を追加しましたが、どちらも `vkEndCommandBuffer failed` は不変でした。
- さらに `pre_encode_minimal_probe`（`with-encode-minimal`）を追加し、`pRefLists/pShortTermRefPicSet/pLongTermRefPics` を null + `setup_reference_slot` なしでも `vkEndCommandBuffer failed` が再現することを確認しました。`VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_FLAGS_MODE=default|non-reference`（`is_reference` / `IrapPicFlag` 切替）でも失敗点は変わらず、現環境では picture flags も主因ではありません。
- 追加で `VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_INFO_MODE=default|intra-i|inter-p|temporal-1|poc-1` を導入し、`StdVideoEncodeH265PictureInfo.pic_type` / `TemporalId` / `PicOrderCntVal` と slice header `slice_type` を A/B 可能化しました。`empty-template + control_mode=none + nalu_mode=empty + picture_flags_mode=non-reference` の5ケースでも `encode_submit_execution=failed (vkEndCommandBuffer failed...)` は不変で、現環境ではこれら picture-info 残項目も主因ではありません。
- さらに `VIDEO_HW_VULKAN_HEVC_ENCODE_RATE_CONTROL_MODE=auto|disabled|cbr|vbr|none` を追加し、rate-control 選択を probe 上で強制できるようにしました。`empty-template + control_mode=none + nalu_mode=empty + picture_flags_mode=non-reference + picture_info_mode=default` で5ケース比較した結果、`requested_rate_control_mode` と `rate_control_mode` は期待どおり切り替わる一方、失敗点は全ケースで `encode_submit_execution=failed (vkEndCommandBuffer failed...)` のままで不変でした。
- 追加で `VIDEO_HW_VULKAN_HEVC_ENCODE_MAINTENANCE1_MODE=auto|on|off` を導入し、logical device 作成時の `VK_KHR_video_maintenance1` feature 有効化方針を明示切替できるようにしました。diagnostics には bootstrap 側 `maintenance1_mode` と submit 側 `encode_probe_inputs(... maintenance1_mode=..., maintenance1_feature_enabled=...)` を追加しています。`sample/empty-template × auto/on/off` の6ケース比較でも `encode_submit_execution=failed (vkEndCommandBuffer failed...)` は不変で、現環境では `maintenance1_feature_supported=false` のため `on` 指定でも `maintenance1_feature_enabled=false` のままでした。
- さらに `VIDEO_HW_VULKAN_HEVC_ENCODE_SESSION_DPB_MODE=default|minimal-one` を導入し、`vkCreateVideoSessionKHR` の `max_dpb_slots` / `max_active_reference_pictures` を capability 値（default）と最小値1（minimal-one）で切替可能化しました。`sample/empty-template × default/minimal-one` の4ケース比較で diagnostics 上は `session_max_dpb_slots=16→1` / `session_max_active_refs=15→1` へ切替が反映される一方、失敗点は全ケース `encode_submit_execution=failed (vkEndCommandBuffer failed...)` のままで不変でした。
- `VIDEO_HW_VULKAN_HEVC_ENCODE_SESSION_H265_CREATE_INFO_MODE=with-max-level|without` で `VkVideoSessionCreateInfoKHR` に H.265-specific create info を付ける/付けないを切替できます。FFmpeg は H.265 create info を付けないため `without` が FFmpeg 風ですが、この環境では `without` でも submit failure は不変でした。
- さらに `VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_RESOURCE_EXTENT_MODE=coded|image-aligned` を導入し、`VideoPictureResourceInfoKHR.coded_extent` を coded サイズ（640x360）と image align 後サイズ（640x384）で切替可能化しました。`sample/empty-template × coded/image-aligned` の4ケース比較では diagnostics 上の `picture_resource_coded` / `picture_resource_extent_mode` は期待どおり切替される一方、`encode_submit_execution=failed (vkEndCommandBuffer failed...)` と `pre_encode_probe=failed` は全ケースで不変でした。
- source 側は `VIDEO_HW_VULKAN_HEVC_ENCODE_SOURCE_PICTURE_RESOURCE_EXTENT_MODE=coded|image-aligned` で個別に切替できます。320x180 probe では `src_picture_resource=320x180` と `320x192` のどちらでも `vkEndCommandBuffer failed` は不変でした。
- 追加で `VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_SESSION_PARAMETERS_MODE=with|without` を導入し、`cmd_begin_video_coding_khr` に `video_session_parameters` を渡す/渡さないを切替可能化しました。`with` ケース（sample/empty-template）は従来どおり `vkEndCommandBuffer failed`、`without` ケースは実行時に `0xc0000005 (STATUS_ACCESS_VIOLATION)` へ悪化し、`begin_session_parameters_mode=without` がドライバクラッシュ側を強く誘発することを確認しました。
- `VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_PNEXT_MODE=none|rate-control`（`ffmpeg` alias 可）で `cmd_begin_video_coding_khr` の pNext に rate-control info を付ける/付けないを切替できます。FFmpeg 風 `rate-control` でも `vkEndCommandBuffer failed` は不変でした。
- `VIDEO_HW_VULKAN_HEVC_ENCODE_DST_RANGE_MODE=full|ffmpeg-reserve-align`（`ffmpeg` alias 可）で `VkVideoEncodeInfoKHR.dstBufferRange` を full aligned range のまま使うか、FFmpeg と同じく末尾 alignment 分を予約するかを切替できます。320x180 の FFmpeg 風 probe では `dst_range` が `1048576→1048320` へ切り替わりますが、失敗点は `vkEndCommandBuffer failed` のままでした。
- `VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SIZE_MODE=sample|coded`（`ffmpeg` alias 可）で sample-derived StdVideo SPS の picture size を probe coded size へ上書きできます。`parameter_mode=sample + parameter_size_mode=coded` では 320x180 probe の `parameter_set_coded_match=true` まで揃いますが、`vkEndCommandBuffer failed` は不変でした。
- `VIDEO_HW_VULKAN_HEVC_ENCODE_IMAGE_VIEW_MODE=ycbcr-conversion|no-ycbcr` で encode image view の `VkSamplerYcbcrConversionInfo` pNext を付ける/付けないを切替できます。FFmpeg 生成 320x180 SPS/PPS を使った probe では `no-ycbcr` でも image view 作成は通りますが、`vkEndCommandBuffer failed` は不変でした。
- `VIDEO_HW_VULKAN_HEVC_ENCODE_DST_PREFIX_MODE=none|zero|parameter-sample`（`ffmpeg` alias 可）で `dst_prefix` 領域を実際に初期化できます。FFmpeg 生成 320x180 stream の先頭 256 bytes を prefix へ書き、`parameter_vui_safety=preserve` を併用した FFmpeg 風条件では NVIDIA Vulkan HEVC encode submit が `Ready(bytes_written=47)` まで進みます。`VIDEO_HW_VULKAN_HEVC_ENCODE_OUTPUT_PATH=<path>` を指定すると `dstBufferOffset + feedback_offset` から実出力sliceだけをdumpできます。FFmpeg生成sampleのheader NALを前置した検証では、FFmpeg decodeが通り、probe入力の平坦NV12に対して MSE=0 / PSNR=inf を確認しています。ただし production encoder としてはまだ有効化していません。
- 追加で `VIDEO_HW_VULKAN_HEVC_ENCODE_PRIMARY_MODE=submit|scope-only|pre-encode-scope|pre-encode|pre-encode-minimal` を導入し、primary probe の録画段を固定できるようにしました。`empty-template + begin/setup slot=none + control=none + nalu=empty + picture_flags=non-reference` 条件で比較すると、`scope-only` / `pre-encode-scope` は `begin_session_parameters_mode=with|without` どちらでも非クラッシュで通過し、`pre-encode` は `with` で `vkEndCommandBuffer failed`・`without` で `0xc0000005`、`pre-encode-minimal` / `submit` は `without` で `0xc0000005` を再現しました。`without` のクラッシュ境界は `cmd_encode_video_khr` 呼び出し段に局在することを確認しています。
- さらに `VIDEO_HW_VULKAN_HEVC_ENCODE_CODEC_INFO_MODE=with-h265-info|with-h265-info-std-picture-only|with-h265-info-minimal|with-h265-info-no-std-picture|without-h265-info` を追加し、`VideoEncodeInfoKHR` に渡す `VideoEncodeH265PictureInfoKHR` の形を段階的に切り替えられるようにしました。`primary_mode=pre-encode`・`begin_session_parameters_mode=with` の比較では、`with-h265-info` と `with-h265-info-std-picture-only` は `vkEndCommandBuffer failed`（非クラッシュ）、`without-h265-info` / `with-h265-info-minimal` / `with-h265-info-no-std-picture` は `0xc0000005` を再現しました。`setup_reference_slot_mode=slot-zero|none` はこの分岐に影響しません。
- `with-h265-info-std-picture-only` の内部でも `picture_flags_mode=default|non-reference`、`picture_info_mode=default|intra-i|inter-p|temporal-1|poc-1`、`reference_list_mode=sentinel|null-pointers`、`rps_mode=empty-struct|null-pointers` を切り替える matrix を追加実行しました。いずれも failure class は `vkEndCommandBuffer failed`（非クラッシュ）で不変でした。現時点で crash 側へ遷移する境界は「`std_picture_info` と nalu entries が両方欠ける/片方だけ欠ける codec-info pNext 形状」に強く局在しています。
- `codec_info_mode` の追加切り分けとして `with-h265-info-minimal`（`VideoEncodeH265PictureInfoKHR` は連結するが `std_picture_info` / nalu entries は未設定）を導入しました。`empty-template + begin_session_parameters_mode=with + begin_reference_slot_mode=none + control_mode=none + nalu_mode=empty + picture_flags_mode=non-reference + primary_mode=pre-encode` で `setup_reference_slot_mode=slot-zero|none` を比較すると、`with-h265-info` は両方 `vkEndCommandBuffer failed`、`without-h265-info` は両方 `0xc0000005`、`with-h265-info-minimal` も両方 `0xc0000005` でした。現環境では `setup_reference_slot` の有無より codec-info pNext の形（特に minimal pNext）が failure class を左右する傾向があります。
- 追加比較として、`with-h265-info` のまま `reference_list_mode=sentinel|null-pointers` と `rps_mode=empty-struct|null-pointers` を切り替えても `vkEndCommandBuffer failed`（非クラッシュ）のままでした。したがって pRefLists/RPS pointer の null 化単独は crash 誘因ではなく、codec-info pNext 自体の形（特に `std_picture_info` も nalu も欠く/片方だけ欠く形）が crash 側へ寄せる有力因子です。
- 追試として `with-h265-info` 側の広い組み合わせ（`control_mode=default|none|ffmpeg`、`nalu_mode=single-slice|empty`、`picture_flags_mode=default|non-reference`、`picture_info_mode=default|intra-i|inter-p|temporal-1|poc-1`、`rate_control_mode=auto|disabled|cbr|vbr|none`、`parameter_mode=empty-template|sample|sample-no-add-info`）を再実行しましたが、全ケース `vkEndCommandBuffer failed` のままでした。
- 追加確認として `with-h265-info-no-std-picture` は `nalu_mode=single-slice|empty` の両方で `0xc0000005`、`with-h265-info-std-picture-only` は `nalu_mode=single-slice|empty` と `parameter_mode=sample` でも `vkEndCommandBuffer failed`（非クラッシュ）を維持しました。よって現時点の crash/non-crash 境界は「codec-info pNext 形状」に強く固定され、周辺の control/rate/nalu/picture 設定では反転しません。
- さらに `with-h265-info-empty-std-picture`（`std_picture_info` は空テンプレートを接続し、nalu entries は接続）を追加して比較しました。`nalu_mode=single-slice|empty` × `setup_reference_slot_mode=none|slot-zero` の4ケースすべて `vkEndCommandBuffer failed`（非クラッシュ）で、crash には遷移しませんでした。現時点の実測では「codec-info pNext が非nullで、かつ `std_picture_info` が接続されている限り非クラッシュ側に留まる」傾向です。
- queue family 選択は `vkGetPhysicalDeviceQueueFamilyProperties2` + `QueueFamilyVideoPropertiesKHR.video_codec_operations` を優先して `ENCODE_H265` / `DECODE_H265` を確認するよう更新しました。driver が codec operation metadata を返さない環境では互換性維持のため従来どおり `VIDEO_ENCODE_KHR` / `VIDEO_DECODE_KHR` flag 判定へフォールバックします。
- 追加知見（NVENC Main override sample で確認）: `vui_parameters_present_flag` が crash/non-crash を強く分岐します。`sample`（実効 `sample-sps-vui-flag-off`）や `sample-sps-no-vui` は `vkEndCommandBuffer failed` 側で停止し、`sample-sps-no-vui-flag-on` は `0xc0000005 (STATUS_ACCESS_VIOLATION)` を再現します。
- FFmpeg parity 追従として SPS/VPS のゼロHRD payload pointerを保持し、session作成ではdevice max coded extentを使い、HEVC encode profile pNext順をFFmpeg同様のH.265 profile→usage順に揃え、`rate_control_mode=cbr|vbr` では slice `constant_qp=0` / `slice_qp_delta=-26`（PPS init QP 0時）へ切り替えます。ただし現環境の `rate_control_mode=cbr + control_mode=ffmpeg` 追試でも `vkEndCommandBuffer failed` は不変で、`empty-template` の1920x1088整列ケースも失敗します。
- `output\\ffmpeg-vulkan-hevc-probe-1f.h265`（FFmpeg `hevc_vulkan` 生成、320x180）をparameter overrideに使い、probeも320x180へ合わせた場合でも `ERROR_OUT_OF_HOST_MEMORY` feedback と `vkEndCommandBuffer failed` は不変です。
- 再現時は `VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SAMPLE_PATH=output\\ffmpeg-hevc-nvenc-main-640x384.h265` を併用し、`encode_synthetic --backend vulkan --codec hevc --require-hardware` の blocker message に出る `parameter_mode=...` で実効 mode を確認してください。
- HEVC decode 着手として、unsafe な Vulkan FFI 呼び出しを `vulkan_hevc_decode` モジュールに隔離し、上位 API からは安全な probe 結果（enum）だけを扱う境界にしています。
- 上記 probe は拡張有無だけでなく、`VIDEO_DECODE_KHR` queue family と最小 logical-device 初期化まで検証し、失敗理由を `UnsupportedConfig` へ反映します。
- HEVC Annex-B の VPS/SPS/PPS 抽出と SPS 由来の解像度解析は `scuffle-h265` で実装済みで、decode 未実装時の診断メッセージにパラメータセット状態を反映します。
- さらに bitstream が与えられた decode パスでは、HEVC profile の capability / output-format query に加えて、報告された output format 候補を順に試しながら `vkCreateVideoSessionKHR` / `vkCreateVideoSessionParametersKHR` の作成 probe を実行し、SPS 解像度チェック結果と合わせて blocker message に追記します。
- `vkCreateVideoSessionParametersKHR` probe では抽出した VPS/SPS/PPS を `StdVideoH265VideoParameterSet` / `StdVideoH265SequenceParameterSet` / `StdVideoH265PictureParameterSet` へ変換して `VideoDecodeH265SessionParametersAddInfoKHR` に投入します（ID・解像度・DPB・短期/長期参照セットなどの基本項目を反映）。
- 現時点では PPS の `pps_scaling_list_data_present_flag=1` と `pps_extension_present_flag=1` を未対応として明示エラーにし、失敗理由が診断メッセージで分かるようにしています。
- 上記 probe が成功した場合は decode submit/reap の次段実装向けに DPB slot / reference slot の計画骨組み（decode submit skeleton）も生成し、先頭 VCL slice header（NAL type / PPS id / POC LSB）解析結果と合わせて blocker message に `decode_submit_skeleton=...` として出力します。
- さらに submit 実行前提として、`vkGetVideoSessionMemoryRequirementsKHR` / `vkBindVideoSessionMemoryKHR` / decode source buffer 準備 / `vkCmdBeginVideoCodingKHR`→`vkCmdDecodeVideoKHR`→`vkCmdEndVideoCodingKHR` の録画・submit・fence wait に加え、decode 出力 image を `vkCmdCopyImageToBuffer` で readback buffer へコピーし `vkMapMemory` で回収確認する probe を追加し、`decode_submit_execution=...` で可否を診断します。
- 同一 bitstream に対する HEVC bootstrap 結果は `submit_probe_access_unit_limit` と組み合わせてキャッシュされるため、`Metadata` → `Rgb24` → `Nv12` のような連続実行で Vulkan device/session を毎回再初期化せず、`Initialization of an object has failed` 系の再現を避けます。
- 回帰テストとして `e2e_vulkan_decode_hevc_sequential_non_metadata_modes` を追加し、非 metadata モードを連続実行して 303 frame 返ることを確認しています。
- HEVC decode の DPB 経路は既定で有効です。診断時のみ `VIDEO_HW_VULKAN_HEVC_EXPERIMENTAL_DPB=off|auto|on` で切り替えられ、`off` は参照品質が大きく落ちるため通常利用では使いません。`auto` は `%TEMP%\\video-hw-vulkan-hevc-dpb-inflight.flag` 残留時に安全側へ自動抑止します。
- 非 metadata の HEVC 出力では submit probe の access-unit 上限を stream 長へ拡張して full coverage を要求します。`decode_submit_execution=ready(...)` の `submitted_access_units` が足りない場合は `UnsupportedConfig` を返します。最新の品質確認では Vulkan HEVC decode は FFmpeg software decode 参照に対して PSNR PASS しています。

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

`--backend auto` は各 example 内で `Backend::resolve_decoder` / `Backend::resolve_encoder` を使って concrete `BackendKind` に解決されます。

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

# camera preview + fragmented MP4 recorder (list devices)
cargo run --example camera_record_fmp4 -- --list-devices
# camera preview + fragmented MP4 recorder (toggle Start/Stop in GUI)
cargo run --features backend-intel --example camera_record_fmp4 -- --backend intel --codec h264 --resolution 1280x720 --fps 30 --fragment-frames 15 --require-hardware --output-dir output/camera-fmp4
# camera preview + fragmented MP4 recorder (non-interactive timed run)
cargo run --features backend-intel --example camera_record_fmp4 -- --backend intel --codec h264 --resolution 1280x720 --fps 30 --fragment-frames 15 --require-hardware --auto-start-recording --duration 3 --output-dir output/camera-fmp4
# GUI は左ペイン（折りたたみ可）に操作系、右ペインに自動スケーリング表示のプレビューを配置
# 左ペインの Start/Stop + status は固定表示、その他の設定群は独立スクロール領域に配置
# backend 選択（auto/nvidia/intel/vulkan）と backend availability の確認、codec (h264/hevc)・capture 解像度/FPS・fragment頻度(frame数)を変更し、Apply Capture/Apply Fragment で反映可能（fragment頻度に合わせてI-frameを揃える）
# 録画中 status の packets/segments/bytes は逐次更新。Stop時は flush_packets を表示（小さいほど録画中に取り出せている）
# 録画フレーム投入は recorder worker thread へ非同期キューイングし、GUI 側には pending queue 件数を表示
# 各 fragment 書き込み時に flush + sync_data を実行して逐次保存を強化
# unsupported な backend+codec 組み合わせは auto-start preflight で即時エラー終了（zero-byte MP4を残さない）

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
