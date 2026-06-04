plugins {
    id("com.android.application") version "8.7.0" apply false
}

// App module sketch:
// android {
//     namespace = "example.video.hw.android"
//     compileSdk = 36
//
//     defaultConfig {
//         applicationId = "example.video.hw.android"
//         minSdk = 28
//         targetSdk = 36
//         versionCode = 1
//         versionName = "1.0"
//
//         ndk {
//             abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
//         }
//     }
//
//     externalNativeBuild {
//         cmake {
//             path = file("src/main/cpp/CMakeLists.txt")
//         }
//     }
// }
