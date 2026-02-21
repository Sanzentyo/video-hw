# Test Spec Inventory（現存テスト仕様の棚卸し）

更新日: 2026-02-21

## 1. 目的

この文書は、現時点でリポジトリに存在するテストの「実行対象」「前提条件」「検証内容」を漏れなく整理した台帳である。  
最終的に E2E が実質同一の挙動を担保できているかを判断するための基準として使う。

## 2. 実行サーフェスの整理

### 2.1 root crate（現行の主対象）

- パッケージ: root `video-hw`（`Cargo.toml`）
- 通常実行: `cargo test`
- 実行対象:
  - unit tests（`src/*.rs` 内 `#[cfg(test)]`）
  - integration tests（`tests/e2e_video_hw.rs`）

### 2.2 `crates/` 配下

- 旧レガシー E2E は削除済み（2026-02-21）
- canonical な実行面は root `tests/e2e_video_hw.rs`

## 3. root unit tests 仕様

## 3.1 `src/bitstream.rs`

- `chunked_parse_converges`
  - H.264 Annex-B を 3byte chunk に分割投入しても AU 組み立て結果が収束すること
  - 期待: AU 2件、先頭 keyframe / 次 non-keyframe
- `extracts_required_parameter_sets`
  - ParameterSet cache が H.264 の SPS/PPS を抽出できること
  - 期待: `required_for_codec(H264)` が 2件

## 3.2 `src/pipeline.rs`

- `queue_stats_track_depth_and_peak`
  - bounded queue の depth / peak_depth が正しく更新されること
- `inflight_credits_work`
  - credit 上限、release 後の再獲得が正しく機能すること

## 3.3 `src/transform.rs`

- `nv12_to_rgb_returns_expected_size`
  - NV12->RGB24 変換後の寸法/バイト数整合
- `dispatcher_runs_transform_job`
  - worker dispatcher がジョブを処理し結果を返すこと
- `keep_native_fast_path_bypasses_transform`
  - `ColorRequest::KeepNative` + resize無しで enqueue 不要判定になること

## 3.4 `src/backend_transform_adapter.rs`

- `keep_native_fast_path_returns_input`
  - NVIDIA adapter の KeepNative fast-path が入力をそのまま返すこと
- `nv12_rgb_request_runs_worker`
  - NVIDIA adapter で NV12->RGB 要求時に即時 or 非同期 reaping のどちらでも完了すること
- `vt_keep_native_fast_path_returns_input`
  - VT adapter の KeepNative fast-path が入力をそのまま返すこと
- `vt_nv12_rgb_request_runs_worker`
  - VT adapter で NV12->RGB 要求時に即時 or 非同期 reaping のどちらでも完了すること

## 3.5 `src/pipeline_scheduler.rs`

- `keep_native_frame_passes_through_scheduler`
  - scheduler 経由で Metadata frame が保持されること
- `rgb_request_reaps_async_result`
  - scheduler が非同期 RGB 変換結果を回収できること
- `stale_generation_is_dropped`
  - stale generation の入力が `TemporaryBackpressure` として破棄されること

## 3.6 `src/vt_backend.rs`（`target_os=macos` + `backend-vt`）

- `detect_h264_keyframe_from_length_prefixed_payload`
  - AVCC payload から H.264 keyframe 判定できること
- `detect_h264_non_keyframe_from_length_prefixed_payload`
  - AVCC payload から non-keyframe 判定できること
- `detect_hevc_keyframe_from_length_prefixed_payload`
  - HVCC payload から HEVC keyframe 判定できること
- `vt_switch_immediate_updates_generation_hint`
  - Immediate switch で generation hint と reconfigure pending が更新されること
- `vt_switch_on_next_keyframe_stays_pending_when_frames_are_buffered`
  - pending frame があると OnNextKeyframe switch が保留されること
- `vt_pending_switch_generation_syncs_to_pipeline_scheduler`
  - pending generation が scheduler generation に同期されること

## 3.7 `src/nv_backend.rs`（`backend-nvidia` + Linux/Windows）

- `switch_on_next_keyframe_stays_pending_when_frames_are_buffered`
  - frame バッファ済み時に OnNextKeyframe switch が pending のままになること
- `switch_immediate_updates_config_even_without_active_session`
  - active session 無しでも Immediate switch で GOP 設定等が更新されること
- `pending_switch_generation_syncs_to_pipeline_scheduler`
  - switch 後 generation が scheduler と同期すること
- `push_frame_succeeds_with_integrated_pipeline_scheduler`
  - scheduler 連携時でも `push_frame` が成功し generation が一致すること

## 3.8 `src/lib.rs`

- `unpack_length_prefixed_sample_to_annexb_converts_nals`
  - length-prefixed sample を Annex-B へ正しく展開できること
- `encoded_layout_is_inferred_from_backend_and_codec`
  - backend+codec から `EncodedLayout` 推論（VT/H264=AVCC, VT/HEVC=HVCC, NV=AnnexB）
- `encode_frame_to_legacy_rejects_unsupported_buffer_types`
  - 型付き `EncodeFrame` 変換で未対応バッファ種別が `InvalidInput` になること

## 4. root integration tests（`tests/e2e_video_hw.rs`）

## 4.1 VideoToolbox 有効時（`target_os=macos` + `backend-vt`）

- `e2e_decode_expected_frames_matrix`
  - H264/HEVC x chunk(4096,1MB) の 4ケースで decode 総数が 303
- `e2e_decode_summary_matches_observed_frames`
  - decode 実測総数と `decode_summary().decoded_frames` が一致（H264/HEVC）
- `e2e_decode_flush_without_input_is_empty`
  - 入力なし flush が空結果、summary=0
- `e2e_encode_h264_generates_packets`
  - 30 frame push（返り値空）+ flush で packet 非空
- `e2e_encode_h264_rejects_invalid_argb_payload`
  - ARGBサイズ不正で `InvalidInput("argb payload size mismatch")`
- `e2e_encode_h264_packets_are_pts_monotonic`
  - flush 後 packet PTS が non-decreasing
- `e2e_vt_backend_accepts_explicit_session_switch_request`
  - VT session switch API 呼び出しが `Ok`

## 4.2 backend 無効時（compile-only）

- `e2e_build_without_enabled_backends_compiles`
  - backend variant が1つも有効でない構成でも test binary が生成できること

## 4.3 NVIDIA 有効時（`backend-nvidia`）

- `e2e_nv_decode_expected_frames_matrix`
  - H264/HEVC x chunk(4096,1MB) の 4ケースで decode 総数が 303
- `e2e_nv_decode_summary_matches_observed_frames`
  - decode 実測総数と `decode_summary().decoded_frames` が一致（H264/HEVC）
- `e2e_nv_decode_flush_without_input_is_empty`
  - 入力なし flush が空結果、summary=0
- `e2e_nv_backend_decode_and_encode_work`
  - H264 decode/encode E2E
  - capability が decode/encode/hw_accel=true
  - decode で frame>0、summary一致
  - encode flush で packet 非空
  - CUDA未利用環境は `UnsupportedConfig("CUDA context ...")` で早期 skip
- `e2e_nv_backend_hevc_decode_sample`
  - HEVC decode E2E
  - frame>0、summary一致
  - 非対応GPU/環境は `"CUDA context"` または `"unsupported"` で skip
- `e2e_nv_encode_h264_rejects_invalid_argb_payload`
  - ARGBサイズ不正で `InvalidInput("argb payload size mismatch")`
  - NV は入力検証が `flush` 時に実行されるため、`submit` は enqueue 成功後に `flush` で検証
- `e2e_nv_encode_h264_packets_are_pts_monotonic`
  - flush 後 packet PTS が non-decreasing
- `e2e_nv_backend_encode_accepts_backend_specific_options`
  - `NvidiaEncoderOptions` 指定時の encode 動作確認
  - CUDA未利用環境は skip
- `e2e_nv_backend_accepts_explicit_session_switch_request`
  - NVIDIA session switch API 呼び出しが `Ok`

## 4.4 NVIDIA 無効時

- runtime unsupported テストは廃止
- feature 無効時は variant 自体が非定義（compile-time 制約）

## 5. テスト入力資産

- `sample-videos/sample-10s.h264`
- `sample-videos/sample-10s.h265`

期待 frame 数の基準値は 303（VT decode matrix / 旧テスト資産と整合）。

## 6. 既知の観測事項

- root `cargo test` では `crates/` 配下の旧テストは走らない。
- `e2e_nvidia_backend_requires_feature_when_disabled` はメッセージ文言への厳密依存があり、実装文言変更に弱い。
- NVIDIA E2E は環境依存で早期 return（skip相当）を含むため、常時同一アサーションにはならない。

## 7. E2E同等性を保つ最小受け入れ条件（再設計時）

1. VT有効時: decode 303件、encode flush 非空、PTS単調性、入力妥当性エラーを維持する。
2. NV有効時: decode 303件 matrix、summary 一致、空flush、encode flush 非空、PTS単調性、入力妥当性エラー（環境非対応時は明示 skip）を維持する。
3. feature無効時: backend variant が非定義であること（compile-time 制約）を維持する。
4. session switch API: VT/NV とも `request_session_switch` が成功する。

## 8. レガシーテストの扱い

- `crates/video-hw/tests/e2e_video_hw.rs` は削除済み
- `crates/vt-backend/tests/e2e_vt.rs` は削除済み
- E2E は root `tests/e2e_video_hw.rs` のみを正とする
