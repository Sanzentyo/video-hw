# Intel Linux Troubleshooting

この文書は Linux 上で `backend-intel` を使うときに、`oneVPL` / `VA-API` / 権限周りで詰まる典型パターンをまとめたものです。

## 対象症状

- `pkg-config --libs --cflags vpl` が失敗する
- `intel-onevpl-sys` の build script が `Package 'vpl' ... not found` で落ちる
- 実行時に `libvpl.so: cannot open shared object file` で落ちる
- `decode failed: unsupported config: Loader::new_session: NotFound`
- `--intel-force-software` を付けても `Loader::new_session: NotFound`

## この環境で実際に観測した状態

- GPU: `Intel Alder Lake-N [UHD Graphics]`
- `/dev/dri/renderD128` は存在
- `intel-media-va-driver` は導入済み
- `libva-dev` は導入済み
- `libvpl2` は導入済み
- ただし次が不足
  - `libvpl-dev`
  - `libmfx-gen1.2`
  - `libmfx-gen-dev`
  - `vainfo`
- さらに次も不足
  - `/usr/lib/x86_64-linux-gnu/libvpl.so`
  - `/usr/lib/x86_64-linux-gnu/pkgconfig/vpl.pc`
- ユーザーが `render` / `video` グループに入っていない

上記状態では `video-hw` の Intel backend は build までは通せても、runtime で `Loader::new_session: NotFound` に収束します。

## 何が足りないのか

`libvpl2` は dispatcher です。これだけでは GPU 実装が足りません。

Linux の Intel oneVPL 実行には、通常次も必要です。

- `libmfx-gen1.2`
  Intel oneVPL GPU runtime
- `libmfx-gen-dev`
  上記 runtime の開発ファイル
- `libvpl-dev`
  `libvpl.so` と `vpl.pc` を含む開発ファイル
- `intel-media-va-driver`
  VA-API driver
- `vainfo`
  VA / driver の疎通確認

## Ubuntu 24.04 系の推奨導入

```bash
sudo apt-get update
sudo apt-get install -y \
  libva-dev \
  libvpl-dev \
  libmfx-gen1.2 \
  libmfx-gen-dev \
  intel-media-va-driver \
  vainfo \
  onevpl-tools
```

補足:

- `libva-dev` は `libva-drm.pc` を提供する
- `libvpl-dev` は `vpl.pc` と `libvpl.so` のために必要
- `libmfx-gen1.2` が無いと dispatcher だけいても実装列挙に失敗しやすい

## 権限

`/dev/dri/renderD128` に触れるには、通常 `render` グループが必要です。環境によっては `video` も必要です。

確認:

```bash
ls -l /dev/dri
id
groups
```

追加:

```bash
sudo usermod -aG render,video "$USER"
```

その後:

- ログアウト / ログイン
- もしくは再起動

`usermod` 後にシェルだけ開き直しても、既存セッションでは group が反映されないことがあります。

## build 前チェック

```bash
pkg-config --libs --cflags libva-drm
pkg-config --libs --cflags vpl
ldconfig -p | rg 'libva|libvpl|mfx'
find /usr/lib /lib -maxdepth 3 -name 'libmfx-gen*.so*' 2>/dev/null
```

期待:

- `libva-drm` が通る
- `vpl` が通る
- `libvpl.so` が見える
- `libmfx-gen*.so` が見える

## runtime 前チェック

```bash
vainfo --display drm --device /dev/dri/renderD128
```

期待:

- `iHD` など Intel driver が使われる
- H.264 / HEVC の decode/encode profile が列挙される

`vainfo` がここで失敗するなら、`video-hw` の Intel backend もまず失敗します。

## `libvpl.so` が無い場合

`onevpl-rs` は `libvpl.so` 名で `dlopen` します。`libvpl.so.2` しか無い環境では runtime で次の panic になります。

```text
libvpl.so: cannot open shared object file
```

通常は `libvpl-dev` 導入で解消するはずです。

一時回避だけなら:

```bash
mkdir -p /tmp/video-hw-lib
ln -sf /lib/x86_64-linux-gnu/libvpl.so.2 /tmp/video-hw-lib/libvpl.so
LD_LIBRARY_PATH=/tmp/video-hw-lib ...
```

ただしこれは dispatcher の soname 解決だけです。`Loader::new_session: NotFound` を直すものではありません。

## `vpl.pc` が無い場合

`intel-onevpl-sys` は build 時に `pkg-config vpl` を引きます。

失敗例:

```text
Package 'vpl', required by 'virtual:world', not found
```

通常解:

- `libvpl-dev` を入れる

一時回避:

```bash
LIBVPL_INCLUDE_PATH=/nonexistent cargo check ...
```

これで pregenerated bindings fallback に入れますが、runtime を直すものではありません。

## `Loader::new_session: NotFound` の意味

これは `oneVPL` dispatcher 自体は読めていても、利用可能な implementation を作れなかった状態です。

典型原因:

- `libmfx-gen1.2` 未導入
- Intel media runtime / driver の不足
- `/dev/dri/renderD128` へのアクセス権不足
- login し直しておらず `render` group が反映されていない

## `video-hw` での確認コマンド

build:

```bash
cargo check -p video-hw --features backend-intel --example decode_annexb
```

pregenerated bindings fallback を使う場合:

```bash
LIBVPL_INCLUDE_PATH=/nonexistent \
cargo check -p video-hw --features backend-intel --example decode_annexb
```

runtime:

```bash
cargo run -p video-hw --example decode_annexb --features backend-intel -- \
  --backend intel \
  --codec h264 \
  --input sample-videos/sample-10s.h264 \
  --require-hardware
```

ソフトウェア明示:

```bash
cargo run -p video-hw --example decode_annexb --features backend-intel -- \
  --backend intel \
  --codec h264 \
  --input sample-videos/sample-10s.h264 \
  --intel-force-software
```

両方で `Loader::new_session: NotFound` なら、依存解決ではなく Linux 側 runtime が欠けています。

## この環境での結論

この環境では次が原因で Intel backend が runtime まで到達しませんでした。

1. `libvpl-dev` 未導入
2. `libmfx-gen1.2` 未導入
3. `libvpl.so` / `vpl.pc` が無い
4. ユーザーが `render` / `video` グループに未所属

コード側や `git+rev` 依存指定の問題ではなく、Linux 側 oneVPL runtime の不足です。
