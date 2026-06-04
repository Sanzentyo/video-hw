# Android Camera Smoke APK

Small Kotlin Camera2/MediaRecorder/MediaCodec validation APK for `video-hw` Android work.

The app opens the back camera, shows a preview, records a short H.264 MP4, then
decodes the recorded video track with `MediaCodec`. It logs results with the
`VideoHwCameraSmoke` tag.

If the preview surface is not available, the app still records the camera stream
directly into `MediaRecorder` and then validates the MP4 with `MediaCodec`.

Build with:

```powershell
.\scripts\android-camera-apk\build.ps1
```

The manual build uses Android SDK platform `android-36.1`, build-tools `36.1.0`,
target SDK 36, and the Kotlin compiler under `output/kotlin/kotlinc`.

The sample source follows the repository license. The built APK includes the
Kotlin standard library from JetBrains, which is licensed under Apache-2.0.
