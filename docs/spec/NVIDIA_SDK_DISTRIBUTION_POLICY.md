# NVIDIA SDK Distribution Policy

更新日: 2026-02-23

## 1. 目的

この文書は、`video-hw` の `backend-nvidia` 利用時における配布・ビルド・ライセンス運用の基準を定義する。

## 2. 前提

- `backend-nvidia` は `nvidia-video-codec-sdk` crate を依存として利用する
- `nvidia-video-codec-sdk` は NVIDIA Video Codec SDK の Rust bindings（ラッパー）である
- NVIDIA Video Codec SDK 本体（lib/headers）は本リポジトリに同梱しない

## 3. 配布ポリシー

1. SDK 本体を本リポジトリへコミットしない
- 例: `nvEncodeAPI.lib`, `nvcuvid.lib`, `libnvidia-encode.so`, `libnvcuvid.so`, SDK headers

2. SDK 本体をリリース成果物へ同梱しない
- 利用者が NVIDIA 公式配布元から別途取得する前提

3. NVIDIA 依存は optional feature として扱う
- `backend-nvidia` を有効化しない構成を常に維持する

## 4. ビルド運用

`backend-nvidia` 利用時は、環境に応じて SDK ライブラリ探索パスを設定する。

Windows 例:

```powershell
$env:NVIDIA_VIDEO_CODEC_SDK_PATH = "C:\Path\To\Video_Codec_SDK\Lib\x64"
```

Linux 例:

- `libnvidia-encode.so` / `libnvcuvid.so` が探索可能なライブラリパス上にあること
- 必要に応じて `NVIDIA_VIDEO_CODEC_SDK_PATH` を設定

## 5. リリース前チェック

1. SDK 実体ファイルが repo に含まれていないことを確認する
2. `README` / `USAGE_STRICT` / `IO_FORMAT_CONTRACT` / `THIRD_PARTY_NOTICES` の記述が一致していることを確認する
3. `backend-nvidia` 無効構成でビルドとテストが成立することを確認する
4. 依存ライセンス確認を実行する（`cargo deny check licenses advisories bans sources`）

## 6. 参照

- `README.md`
- `docs/USAGE_STRICT.md`
- `docs/spec/IO_FORMAT_CONTRACT.md`
- `THIRD_PARTY_NOTICES.md`
- `deny.toml`
- `https://github.com/Sanzentyo/nvidia-video-codec-sdk`
