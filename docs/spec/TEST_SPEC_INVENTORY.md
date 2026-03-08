# Test Spec Inventory（現存テスト仕様の棚卸し）

更新日: 2026-02-23

## 1. 目的

この文書は、現時点でリポジトリに存在するテストの実行対象・前提条件・検証内容を整理した台帳である。  
実装変更時に「何を壊してはいけないか」を判断する基準として使う。

## 2. 実行サーフェスの整理

### 2.1 workspace canonical crate

- パッケージ: `crates/video-hw`（`video-hw` facade crate）
- 通常実行: workspace root で `cargo test`（default-members）
- 実行対象:
  - unit tests（`crates/video-hw/src/*.rs` 内 `#[cfg(test)]`）
  - integration tests（`crates/video-hw/tests/e2e_video_hw.rs`）

### 2.2 core crate

- パッケージ: `crates/video-hw-core`
- 現状は型/契約提供が中心で、単体テストは未定義

## 3. `crates/video-hw` unit tests 仕様

### 3.1 `crates/video-hw/src/bitstream.rs`

- `chunked_parse_converges`
  - H.264 Annex-B を小チャンク分割投入しても AU 組み立て結果が収束すること
  - 期待: AU 2件、先頭 keyframe / 次 non-keyframe
- `extracts_required_parameter_sets`
  - ParameterSet cache が H.264 の SPS/PPS を抽出できること
  - 期待: `required_for_codec(H264)` が 2件

### 3.2 `crates/video-hw/src/pipeline.rs`

- `queue_stats_track_depth_and_peak`
  - bounded queue の depth / peak_depth が正しく更新されること
- `inflight_credits_work`
  - credit 上限、release 後の再獲得が正しく機能すること

### 3.3 `crates/video-hw/src/transform.rs`

- `nv12_to_rgb_returns_expected_size`
  - NV12->RGB24 変換後の寸法/バイト数整合
- `dispatcher_runs_transform_job`
  - worker dispatcher がジョブを処理し結果を返すこと
- `keep_native_fast_path_bypasses_transform`
  - `ColorRequest::KeepNative` + resize 無しで enqueue 不要判定になること

### 3.4 `crates/video-hw/src/backend_transform_adapter.rs`

- `keep_native_fast_path_returns_input`（`backend-nvidia` + Linux/Windows）
  - NVIDIA adapter の KeepNative fast-path が入力をそのまま返すこと
- `nv12_rgb_request_runs_worker`（`backend-nvidia` + Linux/Windows）
  - NVIDIA adapter で NV12->RGB 要求時に即時 or 非同期 reaping のどちらでも完了すること
- `vt_keep_native_fast_path_returns_input`
  - VT adapter の KeepNative fast-path が入力をそのまま返すこと

### 3.5 `crates/video-hw/src/pipeline_scheduler.rs`（`backend-nvidia` + Linux/Windows）

- `keep_native_frame_passes_through_scheduler`
  - scheduler 経由で Metadata frame が保持されること
- `rgb_request_reaps_async_result`
  - scheduler が非同期 RGB 変換結果を回収できること
- `stale_generation_is_dropped`
  - stale generation の入力が `TemporaryBackpressure` として扱われること

### 3.6 `crates/video-hw/src/vt_backend.rs`（`target_os=macos` + `backend-vt`）

- `detect_h264_keyframe_from_length_prefixed_payload`
- `detect_h264_non_keyframe_from_length_prefixed_payload`
- `detect_hevc_keyframe_from_length_prefixed_payload`
- `vt_switch_immediate_updates_generation_hint`
- `vt_switch_on_next_keyframe_stays_pending_when_frames_are_buffered`
- `vt_pending_switch_generation_syncs_to_pipeline_scheduler`

### 3.7 `crates/video-hw/src/nv_backend.rs`（`backend-nvidia` + Linux/Windows）

- `switch_on_next_keyframe_stays_pending_when_frames_are_buffered`
- `switch_immediate_updates_config_even_without_active_session`
- `pending_switch_generation_syncs_to_pipeline_scheduler`
- `push_frame_succeeds_with_integrated_pipeline_scheduler`

### 3.8 `crates/video-hw/src/lib.rs`

- `backend_default_is_auto`（backend が1つ以上有効な構成）
- `unpack_length_prefixed_sample_to_annexb_converts_nals`
- `encoded_layout_is_inferred_from_backend_and_codec`
- `encode_frame_into_backend_frame_rejects_unsupported_buffer_types`（`unstable-raw-inputs` 有効時）

## 4. integration tests（`crates/video-hw/tests/e2e_video_hw.rs`）

### 4.1 VideoToolbox 有効時（`target_os=macos` + `backend-vt`）

- `e2e_decode_expected_frames_matrix`
  - H264/HEVC x chunk(4096,1MB) の 4ケースで decode 総数が 303
- `e2e_decode_summary_matches_observed_frames`
  - decode 実測総数と `summary().decoded_frames` が一致
- `e2e_vt_decode_metadata_includes_pts_and_decode_flags`
  - decode metadata に `pts/decode_info_flags` が設定されること
- `e2e_decode_flush_without_input_is_empty`
  - 入力なし flush が空結果、summary=0
- `e2e_encode_h264_generates_packets`
  - 30 frame submit + flush で packet 非空
- `e2e_encode_h264_rejects_invalid_argb_payload`
  - ARGB サイズ不正で `InvalidInput` を返すこと
- `e2e_encode_h264_packets_are_pts_monotonic`
  - flush 後 packet PTS が non-decreasing
- `e2e_vt_backend_accepts_explicit_session_switch_request`
  - VT session switch API 呼び出しが `Ok`

### 4.2 NVIDIA 有効時（`backend-nvidia` + Linux/Windows）

- `e2e_nv_decode_expected_frames_matrix`
- `e2e_nv_decode_summary_matches_observed_frames`
- `e2e_nv_decode_flush_without_input_is_empty`
- `e2e_nv_encode_h264_packets_are_pts_monotonic`
- `e2e_nv_encode_h264_rejects_invalid_argb_payload`
- `e2e_nv_backend_decode_and_encode_work`
- `e2e_nv_backend_hevc_decode_sample`
- `e2e_nv_backend_encode_accepts_backend_specific_options`
- `e2e_nv_backend_accepts_explicit_session_switch_request`

注記:
- NVIDIA E2E は環境依存のため、`UnsupportedConfig("CUDA context ...")` などで早期 return（skip相当）を含む
- invalid ARGB 検証は NV 実装上 `flush` タイミングで確定する

### 4.3 backend 無効時（compile-only）

- `e2e_build_without_enabled_backends_compiles`
  - backend variant が有効化されない構成でも test binary が生成できること

## 5. テスト入力資産

- `sample-videos/sample-10s.h264`
- `sample-videos/sample-10s.h265`

期待 frame 数の基準値は 303。

## 6. 既知の観測事項

- feature なしの `cargo test` では `crates/video-hw` unit 10件 + integration 1件（compile-only）
- `crates/video-hw/src/pipeline_scheduler.rs` の unit test は `backend-nvidia` 有効時のみ実行される
- sample 参照は workspace root `sample-videos/` を前提に解決する

## 7. E2E同等性を保つ最小受け入れ条件（再設計時）

1. VT有効時: decode 303件、summary一致、空flush、encode flush 非空、PTS単調性、入力妥当性エラーを維持する
2. NV有効時: decode matrix 303件、summary一致、空flush、encode flush 非空、PTS単調性、入力妥当性エラーを維持する
3. feature無効時: backend variant 非有効でも compile-only test が成立する
4. session switch API: VT/NV とも `request_session_switch` が成功する

## 8. レガシー資産の扱い

- `crates/video-hw/tests/e2e_video_hw.rs` を canonical とする
- workspace 構成は `crates/video-hw-core` + `crates/video-hw` を正とする
