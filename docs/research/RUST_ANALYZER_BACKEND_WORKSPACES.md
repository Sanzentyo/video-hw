# rust-analyzer backend 切替運用

## 目的

`video-hw` は backend を feature + target で分岐するため、
`rust-analyzer` の補完/定義ジャンプも backend 前提で切替できるようにする。

## 追加した設定

- デフォルト設定: `.vscode/settings.json`
  - `rust-analyzer.cargo.features = "all"`
  - `rust-analyzer.check.features = "all"`
  - host target で有効 backend が見える

- backend 専用 workspace:
  - `.vscode/video-hw-vt.code-workspace`
  - `.vscode/video-hw-nv-linux.code-workspace`
  - `.vscode/video-hw-nv-windows.code-workspace`

## 使い方

1. 普段はリポジトリをそのまま開く（`settings.json` が適用される）。
2. backend 固定で見たい場合のみ `.code-workspace` を開く。
   - VT: `video-hw-vt.code-workspace`
   - NV/Linux: `video-hw-nv-linux.code-workspace`
   - NV/Windows: `video-hw-nv-windows.code-workspace`

## 注意

- `rust-analyzer` は 1 workspace あたり 1 つの cargo 設定しか持てないため、
  backend ごとの feature を「同時に自動切替」はできない。
- そのため必要時だけ backend 専用 workspace を使う運用にする。
