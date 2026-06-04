# cargo-ndk build example

```bash
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk

export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/<version>"

cargo ndk \
  -t arm64-v8a \
  -t armeabi-v7a \
  -t x86_64 \
  -o app/src/main/jniLibs \
  build -p your_android_wrapper --release
```

`video-hw` を直接 Android app から使う場合、JNI boundary を持つ wrapper crate を別に作り、`cdylib` として app に組み込むのが扱いやすいです。
