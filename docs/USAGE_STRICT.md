# video-hw 利用ガイド（厳密 I/O 仕様, 現行実装準拠）

この文書は `DecodeSession` / `EncodeSession` の現行APIを、実装準拠で使うためのガイドです。

## 1. 導入

- macOS: `backend-vt`
- Linux/Windows: `backend-nvidia` / `backend-intel` / `backend-vulkan`
- `default = []`

```toml
[target.'cfg(target_os = "macos")'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-vt"] }

[target.'cfg(any(target_os = "linux", target_os = "windows"))'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-nvidia"] } # or ["backend-intel"] / ["backend-vulkan"]

# backend 実装crate（video-hw が内部で利用。直接使う場合のみ追加）
video-hw-backend-nvidia = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3" }
video-hw-backend-intel = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3" }
video-hw-backend-vulkan = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3" }
video-hw-backend-vt = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3" }
```

## 2. Backend 選択

- `Backend::Auto`
- `Backend::VideoToolbox`（macOS + `backend-vt`）
- `Backend::Nvidia`（Linux/Windows + `backend-nvidia`）
- `Backend::Intel`（Linux/Windows + `backend-intel`）
- `Backend::Vulkan`（Linux/Windows + `backend-vulkan`）

`Backend::Auto` は OS 既定 backend を選択します。

`DecodeSession` / `EncodeSession` は static generic 運用を前提にし、
`DecodeSession::<IntelDecoderAdapter>::new(config)` のように型で backend を固定して利用します。
`Backend::Auto` は wrapper 側の backend 選択用であり、セッション生成時には concrete adapter が必要です。
必要な場合は `Backend::resolve_decoder` / `Backend::resolve_encoder`（または `select_decoder_backend` / `select_encoder_backend`）で concrete `BackendKind` を解決してからセッションを組み立てます。
同梱 examples（`decode_annexb` / `encode_synthetic` / `encode_raw_argb` / `encode_streaming_probe` / `camera_record_fmp4`）もこの解決 API に統一済みです。
`camera_record_fmp4` は `shiguredo_video_device` + `shiguredo_mp4` 連携の GUI 例で、左ペイン（折りたたみ可）に操作系、右ペインに自動スケーリングされるプレビューを配置します。左ペイン上部の Start/Stop と status は固定表示で、backend/codec/capture/fragment 設定は独立スクロール領域に配置しています。backend 選択（auto/nvidia/intel/vulkan）と可用性一覧の確認、codec（h264/hevc）切替、capture 解像度/FPS の再設定（Apply Capture）、fragment頻度（frame数）の再設定（Apply Fragment）を行えます。fragment頻度の適用時は I-frame も同周期に合わせて再同期されます。録画中 status には packets/segments/bytes の逐次進捗が表示され、Stop時は `flush_packets`（停止時に追加回収されたpacket数）で逐次取り出し状況を確認できます。録画フレーム投入は recorder worker thread へ非同期キューイングされ、GUI 上で pending queue 件数を確認できます。各fragment書き込み時は flush + sync_data を実行して逐次保存を強化しています。CLI の無操作録画では `--auto-start-recording --duration <seconds>` を使うと起動後に自動で録画開始し、指定秒数で自動停止します。unsupported な backend+codec 組み合わせは auto-start preflight（session init + submit + flush probe）で即時エラー化し、zero-byte MP4 を残さないようにしています。

`video-hw-backend-*` は backend 実装crateです。`video-hw` は feature 有効化時にこれら（nvidia/intel/vulkan/vt）を内部で読み込みます。
直接 `video-hw-backend-*` を使う場合も adapter 型は同じで、`DecodeSession::<Adapter>::new(...)` を利用できます。

### 2.1 NVIDIA backend の前提（重要）

- `backend-nvidia` は `nvidia-video-codec-sdk`（Rust bindings / ラッパー）を通じて NVIDIA SDK を利用する
- SDK 本体（lib/headers）は同梱しない前提で、利用者が NVIDIA から別途取得して配置する
- ビルド時は環境に応じて `NVIDIA_VIDEO_CODEC_SDK_PATH` などの設定が必要になる

### 2.2 Intel backend の前提（重要）

- `backend-intel` は `onevpl-rs`（`intel-onevpl-sys` 経由）で Intel oneVPL を利用する
- 依存宣言は `https://github.com/Sanzentyo/onevpl-rs` を `rev` 固定で参照する（fork 更新時は README の「onevpl fork 更新時の手順」に従う）
- 現行 Intel backend は H.264 / HEVC の encode/decode 実装を持つ（実際に使える codec は oneVPL runtime / ドライバの公開機能に依存）
- `require_hardware=false` は HW優先で初期化し、失敗時に software 実装フォールバックを試行する
- software 実装を明示的に選びたい場合は `IntelDecoderOptions::force_software=true` / `IntelEncoderOptions::force_software=true`（CLI は `--intel-force-software`）を使う。HEVC encode で VPP 経路を明示したい場合は `IntelEncoderOptions::hevc_use_vpp=Some(true)` を指定できる
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
- encode レート制御は `VIDEO_HW_INTEL_RATE_CONTROL`（`cbr|vbr|cqp|avbr|icq|qvbr`）で上書きでき、未指定時は H.264=CBR / HEVC=CQP を使う（`CQP` の既定量子化パラメータは `VIDEO_HW_INTEL_CQP=24`）。encode async depth は `VIDEO_HW_INTEL_ASYNC_DEPTH`（1..=16, default=10）で調整可能。HEVC hardware encode は既定で CPU 側 ARGB→NV12 変換 + `IN_SYSTEM_MEMORY` 投入を優先し、VPP 経路を強制する場合は `VIDEO_HW_INTEL_HEVC_USE_VPP=1`、low-power を無効化する場合は `VIDEO_HW_INTEL_HEVC_LOW_POWER=0` を使う
- HEVC parity を狙う場合は `--equal-raw-input true --raw-input-pix-fmt nv12` を推奨（直近 report 1773584282 は decode/encode とも ±10% 内）。ARGB 入力や VPP 強制は環境依存で encode 差が残ることがある
- 計測揺れが大きい環境では `--settle-ms 300` 前後を併用すると parity 判定が安定しやすい
- `build.rs` で oneVPL を自動取得/自動ビルドする方式は採用しない（依存 `onevpl-sys` の `build.rs` が先に実行されるため）
- oneVPL導入後は再起動が必要な場合がある（インストーラログに reboot 要求が出た場合）

### 2.3 Vulkan backend の前提（重要）

- `backend-vulkan` は `vk-video` + `ash` を通して Vulkan Video API を Rust から直接利用する
- 現行実装は H.264 の decode/encode が主経路。HEVC decode は ash-level submit execution probe（GPU 実行確認）と Annex-B access-unit 数推定を組み合わせる実験段階で、`DecodeOutputMode::Metadata` では access-unit 推定数ぶんの metadata frame、非 metadata（`Nv12` / `Rgb24`）では access-unit 単位の NV12 readback を ARGB へ変換した frame を返す（フルストリーム HEVC decode/encode は `UnsupportedConfig`）
- `require_hardware=true` では Vulkan 実行を必須とする
- `require_hardware=false` でも direct Vulkan backend に software fallback はない（`allow_software_fallback` は現時点では実質未対応）
- Vulkan loader/driver が `VK_KHR_video_queue` + H.264 decode/encode 拡張を提供していること
- HEVC 直対応は着手中。`vk-video 0.2.1` が H.264 API のみのため、`VK_KHR_video_decode_h265` / `VK_KHR_video_encode_h265` を使う ash レベル実装が必要
- HEVC encode submit probe の session parameters は既定で `sample-videos/sample-10s.h265` の VPS/SPS/PPS を使う。切り分け時は `VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SAMPLE_PATH=<Annex-B .h265>` で差し替え可能（missing path は明示エラーで停止、silent fallback しない）。ただし encode probe の parameter-set は Main profile（`profile_idc=1`）のみ受理し、非Main（例: `Rext`）は明示エラーで拒否する。加えて override sample + `VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_MODE=sample` では安全側として `sample-sps-vui-flag-off` に自動切替する（クラッシュ再現が必要な場合のみ `sample-sps-no-vui-flag-on` を明示指定）
- HEVC encode の切り分け mode は `sample-sps-no-vui` / `sample-sps-vui-flag-off` / `sample-sps-no-vui-flag-on` / `sample-sps-level` / `sample-sps-sub-layer-ordering` / `sample-sps-level-ordering` を利用可能。NVENC Main override sample では `sample-sps-no-vui` と `sample-sps-vui-flag-off` が `vkEndCommandBuffer failed`、`sample-sps-no-vui-flag-on` が `0xc0000005` を再現するため、VUI-present flag の影響を切り分けられる
- 追加切り分けとして `VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_IDX_MODE=minus-one|zero`（`StdVideoEncodeH265ReferenceListsInfo.num_ref_idx_l{0,1}_active_minus1`）と `VIDEO_HW_VULKAN_HEVC_ENCODE_CONTROL_MODE=default|ffmpeg|none`（`cmd_control_video_coding_khr` の rate-control 接続方式）を利用できる。現環境ではこの2トグル（+ `none`）でも `vkEndCommandBuffer failed` は不変
- 追加で `VIDEO_HW_VULKAN_HEVC_ENCODE_RPS_MODE=empty-struct|null-pointers`（`pShortTermRefPicSet` / `pLongTermRefPics`）と `VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_LIST_MODE=sentinel|zero|null-pointers`（`pRefLists`）を利用できる。`sample` / `empty-template` の両方で `vkEndCommandBuffer failed` は不変で、現環境ではこれら pointer wiring の有無は root cause ではない
- さらに `VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_REFERENCE_SLOT_MODE=slot-minus-one|none` と `VIDEO_HW_VULKAN_HEVC_ENCODE_SETUP_REFERENCE_SLOT_MODE=slot-zero|none` で begin/setup reference slot を個別に切替できる。8ケース（begin on/off × setup on/off × sample/empty-template）でも `vkEndCommandBuffer failed` は不変だった
- 診断は `coding_scope_probe` / `pre_encode_scope_probe` / `pre_encode_probe` を区別して表示する。現環境は `pre_encode_scope_probe=ok` かつ `pre_encode_probe=failed` のため、失敗点は `cmd_encode_video_khr` を含む段に局在している。追加切り分けの `VIDEO_HW_VULKAN_HEVC_ENCODE_NALU_MODE=single-slice|empty` でも失敗点は変わらなかった
- 追加で `pre_encode_minimal_probe`（`with-encode-minimal`）を導入し、`pRefLists/pShortTermRefPicSet/pLongTermRefPics` を null、`setup_reference_slot` なしでも失敗が再現することを確認。さらに `VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_FLAGS_MODE=default|non-reference`（`is_reference` / `IrapPicFlag` 切替）でも `vkEndCommandBuffer failed` は不変で、現環境では picture flags は主因ではない
- 追加で `VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_INFO_MODE=default|intra-i|inter-p|temporal-1|poc-1` を導入し、`StdVideoEncodeH265PictureInfo.pic_type` / `TemporalId` / `PicOrderCntVal` と `StdVideoEncodeH265SliceSegmentHeader.slice_type` を切り替え可能化。`empty-template + control_mode=none + nalu_mode=empty + picture_flags_mode=non-reference` の各 mode でも失敗は `vkEndCommandBuffer failed` のままで、picture-info の残項目は root cause ではない
- さらに `VIDEO_HW_VULKAN_HEVC_ENCODE_RATE_CONTROL_MODE=auto|disabled|cbr|vbr|none` で rate-control 選択を明示上書きできる。`requested_rate_control_mode` と `rate_control_mode` は diagnostics に出力され、`cbr`/`vbr`/`none` 指定も反映されるが、現環境では5ケースすべて `vkEndCommandBuffer failed` のまま不変
- 追加で `VIDEO_HW_VULKAN_HEVC_ENCODE_MAINTENANCE1_MODE=auto|on|off` で logical device 作成時の `VK_KHR_video_maintenance1` feature 有効化方針を切替できる。diagnostics には bootstrap 側 `maintenance1_mode` と submit 側 `encode_probe_inputs(... maintenance1_mode=..., maintenance1_feature_enabled=...)` が出力される。`sample/empty-template × auto/on/off` の6ケース比較でも失敗点は不変（`vkEndCommandBuffer failed`）で、この環境では `maintenance1_feature_supported=false` のため `on` 指定でも実効 `maintenance1_feature_enabled=false`
- さらに `VIDEO_HW_VULKAN_HEVC_ENCODE_SESSION_DPB_MODE=default|minimal-one` で session create の `max_dpb_slots` / `max_active_reference_pictures` を切替できる。`sample/empty-template × default/minimal-one` の4ケース比較では diagnostics 上の `session_max_dpb_slots` / `session_max_active_refs` は期待どおり `16/15` と `1/1` へ切替される一方、失敗点は全ケース `encode_submit_execution=failed (vkEndCommandBuffer failed...)` のままで不変
- さらに `VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_RESOURCE_EXTENT_MODE=coded|image-aligned` で `VideoPictureResourceInfoKHR.coded_extent` を切替できる。`sample/empty-template × coded/image-aligned` の4ケース比較では diagnostics 上の `picture_resource_coded` / `picture_resource_extent_mode` は期待どおり `640x360` と `640x384` へ切替される一方、失敗点は全ケース `encode_submit_execution=failed (vkEndCommandBuffer failed...)` / `pre_encode_probe=failed` のままで不変
- 追加で `VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_SESSION_PARAMETERS_MODE=with|without` で `cmd_begin_video_coding_khr` の `video_session_parameters` 接続有無を切替できる。`with`（sample/empty-template）は従来どおり `vkEndCommandBuffer failed`、`without` は両ケースとも `0xc0000005`（process crash）となり、`video_session_parameters` を外すと症状が悪化する
- 追加で `VIDEO_HW_VULKAN_HEVC_ENCODE_PRIMARY_MODE=submit|scope-only|pre-encode-scope|pre-encode|pre-encode-minimal` を導入し、primary probe を任意段で停止できるようにした。`empty-template + begin/setup slot=none + control=none + nalu=empty + picture_flags=non-reference` で比較すると、`scope-only` / `pre-encode-scope` は `begin_session_parameters_mode=with|without` とも非クラッシュ、`pre-encode` は `with` で `vkEndCommandBuffer failed`・`without` で `0xc0000005`、`pre-encode-minimal` / `submit` は `without` で `0xc0000005`。よって `without` のクラッシュ境界は `cmd_encode_video_khr` 呼び出し段に局在する
- さらに `VIDEO_HW_VULKAN_HEVC_ENCODE_CODEC_INFO_MODE=with-h265-info|with-h265-info-std-picture-only|with-h265-info-minimal|with-h265-info-no-std-picture|without-h265-info` を用意し、`VideoEncodeH265PictureInfoKHR` の payload 形状を段階切替できるようにした。`primary_mode=pre-encode`・`begin_session_parameters_mode=with` では、`with-h265-info` と `with-h265-info-std-picture-only` が `vkEndCommandBuffer failed`（非クラッシュ）、`without-h265-info` / `with-h265-info-minimal` / `with-h265-info-no-std-picture` が `0xc0000005`。`setup_reference_slot_mode=slot-zero|none` は分岐に影響しない
- 追加で `VIDEO_HW_VULKAN_HEVC_ENCODE_CODEC_INFO_MODE=with-h265-info-minimal` を導入。これは `VideoEncodeH265PictureInfoKHR` の pNext 連結のみを行い、`std_picture_info`/nalu entries を未設定にする最小 mode。`empty-template + begin_session_parameters_mode=with + begin_reference_slot_mode=none + control_mode=none + nalu_mode=empty + picture_flags_mode=non-reference + primary_mode=pre-encode` で `setup_reference_slot_mode=slot-zero|none` を比較すると、`with-h265-info` は両方 `vkEndCommandBuffer failed`、`without-h265-info` と `with-h265-info-minimal` は両方 `0xc0000005`。`setup_reference_slot` はこの分岐の主因ではなく、codec-info pNext 形状が支配的
- `with-h265-info` 対照として `VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_LIST_MODE=sentinel|null-pointers` と `VIDEO_HW_VULKAN_HEVC_ENCODE_RPS_MODE=empty-struct|null-pointers` を比較しても `vkEndCommandBuffer failed` のまま（crash しない）。reference list/RPS pointer null 化単独は主因ではなく、codec-info pNext の payload shape が主要因
- `with-h265-info-std-picture-only` についても `VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_FLAGS_MODE=default|non-reference`、`VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_INFO_MODE=default|intra-i|inter-p|temporal-1|poc-1`、`reference_list_mode=sentinel|null-pointers`、`rps_mode=empty-struct|null-pointers` を matrix 実行したが、全ケース `vkEndCommandBuffer failed`（非クラッシュ）で不変。std-picture 内の現行切替項目単独では crash 側に遷移しない
- `with-h265-info` でも `control_mode=default|none|ffmpeg`、`nalu_mode=single-slice|empty`、`picture_flags_mode=default|non-reference`、`picture_info_mode=default|intra-i|inter-p|temporal-1|poc-1`、`rate_control_mode=auto|disabled|cbr|vbr|none`、`parameter_mode=empty-template|sample|sample-no-add-info` を sweep したが、全ケース `vkEndCommandBuffer failed`（非クラッシュ）で固定
- `with-h265-info-no-std-picture` は `nalu_mode=single-slice|empty` の両方で `0xc0000005`、`with-h265-info-std-picture-only` は `nalu_mode=single-slice|empty` と `parameter_mode=sample` でも `vkEndCommandBuffer failed` のまま。周辺トグルでは crash/non-crash 境界が反転しないことを確認
- `VIDEO_HW_VULKAN_HEVC_ENCODE_CODEC_INFO_MODE=with-h265-info-empty-std-picture`（空の `std_picture_info` + nalu entries）を追加して比較。`nalu_mode=single-slice|empty` × `setup_reference_slot_mode=none|slot-zero` の4ケースはすべて `vkEndCommandBuffer failed`（非クラッシュ）で、crash へは遷移しない
- queue family 判定は `vkGetPhysicalDeviceQueueFamilyProperties2` + `QueueFamilyVideoPropertiesKHR.video_codec_operations` を優先し、`ENCODE_H265` / `DECODE_H265` の codec operation を確認する。driver が codec operation metadata を返さない場合は互換性維持のため `VIDEO_ENCODE_KHR` / `VIDEO_DECODE_KHR` の queue flag 判定へ自動フォールバックする
- HEVC decode は `submit_probe_access_unit_limit` を含む bootstrap 結果を bitstream ごとにキャッシュしており、`Metadata` / `Rgb24` / `Nv12` を連続実行しても Vulkan device/session を毎回再初期化しない。これにより `Initialization of an object has failed` 系の再現を避ける
- unsafe 呼び出しは `vulkan_hevc_decode` モジュールへ閉じ込め、上位の `video-hw` API は safe な probe 結果だけを受け取る責務分割にしている
- HEVC decode probe は拡張列挙だけでなく `VIDEO_DECODE_KHR` queue family と最小 logical-device 初期化まで確認し、失敗理由を診断メッセージへ反映する
- HEVC Annex-B の VPS/SPS/PPS 抽出と SPS 解像度解析は `scuffle-h265` で実装済みで、decode 未実装時の診断メッセージに抽出結果を付与する
- bitstream 付き decode パスでは HEVC profile の capability / output-format query に加えて、報告された output format 候補を順に試しながら `vkCreateVideoSessionKHR` / `vkCreateVideoSessionParametersKHR` の作成 probe まで実行し、SPS 解像度がデバイス許容範囲外なら診断メッセージで明示する
- `vkCreateVideoSessionParametersKHR` probe では抽出済み VPS/SPS/PPS を `StdVideoH265*` 構造体へ変換して `VideoDecodeH265SessionParametersAddInfoKHR` に渡す（ID・解像度・DPB・短期/長期参照セットの基本項目を反映）
- 現在は PPS の `pps_scaling_list_data_present_flag=1` および `pps_extension_present_flag=1` を未対応として明示エラー化しているため、該当ストリームでは診断メッセージに unsupported 理由が出る
- 上記 probe 成功時には decode submit/reap 実装の次段用として DPB slot / reference slot の計画骨組み（decode submit skeleton）を生成し、先頭 VCL slice header（NAL type / PPS id / POC LSB）解析結果と合わせて blocker message に `decode_submit_skeleton=...` を追記する
- submit 実行可否の前段確認として、`vkGetVideoSessionMemoryRequirementsKHR` / `vkBindVideoSessionMemoryKHR` / decode source buffer 準備 / `vkCmdBeginVideoCodingKHR`→`vkCmdDecodeVideoKHR`→`vkCmdEndVideoCodingKHR` の録画・submit・fence wait に加えて decode 出力 image の `vkCmdCopyImageToBuffer` readback と `vkMapMemory` 回収確認まで含む probe を実施し、blocker message に `decode_submit_execution=...` を追記する
- 実験的 DPB 経路は `VIDEO_HW_VULKAN_HEVC_EXPERIMENTAL_DPB`（`off`/`auto`/`on`）で制御する。`auto` は `%TEMP%\\video-hw-vulkan-hevc-dpb-inflight.flag` の残留検出時に自動で無効化され、`decode_submit_execution=ready(...)` では `experimental_dpb_mode` / `experimental_dpb_status`（判定理由）と `readback_bytes` / `readback_planes` / `readback_sample_stride` / `readback_sample_count`（readback handoff 状態）を確認できる
- 非 metadata の HEVC 出力は submit probe の access-unit 上限を stream 長へ拡張して full coverage を要求する。`submitted_access_units` が不足した場合は `UnsupportedConfig` を返す。残課題は DPB/reference-slot を有効化した広範囲ストリームでの安定化

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

- `DecodeSession::<ConcreteDecoder>::new(DecoderConfig)`
- `DecodeSession::from_decoder(DecodeOutputMode, concrete_decoder)`
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

- `EncodeSession::<ConcreteEncoder>::new(EncoderConfig)`
- `EncodeSession::from_encoder(BackendKind, concrete_encoder)`
- `submit(EncodeFrame)`
- `try_reap()`
- `reap_timeout(Duration)`
- `flush()`
- `query_capability(Codec)`
- `request_session_switch(SessionSwitchRequest)`
- `request_session_switch_strict(SessionSwitchRequest)`（`SessionSwitchingEncoderBackend` 実装backendのみ）

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
- Vulkan: `EncodedLayout::AnnexB`

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
cargo test --features backend-vulkan -- --nocapture
cargo check --all-targets --features backend-vulkan
```

## 8. 関連

- `docs/spec/IO_FORMAT_CONTRACT.md`
- `docs/spec/TEST_SPEC_INVENTORY.md`
- `docs/status/STATUS.md`
