param(
    [string]$SdkRoot = "$PSScriptRoot\..\..\output\android-sdk",
    [string]$KotlinHome = "$PSScriptRoot\..\..\output\kotlin\kotlinc",
    [string]$OutDir = "$PSScriptRoot\..\..\output\android-camera-apk"
)

$ErrorActionPreference = "Stop"

$ProjectDir = $PSScriptRoot
$BuildDir = Join-Path $OutDir "build"
$DexDir = Join-Path $BuildDir "dex"
$GenDir = Join-Path $BuildDir "gen"
$ClassesDir = Join-Path $BuildDir "classes"
$JniLibsDir = Join-Path $BuildDir "jniLibs"
$Keystore = Join-Path $OutDir "debug.keystore"

$AndroidJar = Join-Path $SdkRoot "platforms\android-36.1\android.jar"
$BuildTools = Join-Path $SdkRoot "build-tools\36.1.0"
$Aapt2 = Join-Path $BuildTools "aapt2.exe"
$D8 = Join-Path $BuildTools "d8.bat"
$Zipalign = Join-Path $BuildTools "zipalign.exe"
$ApkSigner = Join-Path $BuildTools "apksigner.bat"
$Kotlinc = Join-Path $KotlinHome "bin\kotlinc.bat"
$KotlinStdlib = Join-Path $KotlinHome "lib\kotlin-stdlib.jar"
$NdkRoot = Join-Path $SdkRoot "ndk\29.0.14206865"
$NativeLinker = Join-Path $NdkRoot "toolchains\llvm\prebuilt\windows-x86_64\bin\aarch64-linux-android31-clang.cmd"

foreach ($Path in @($AndroidJar, $Aapt2, $D8, $Zipalign, $ApkSigner, $Kotlinc, $KotlinStdlib, $NativeLinker)) {
    if (-not (Test-Path $Path)) {
        throw "Required tool or file not found: $Path"
    }
}

if (Test-Path $BuildDir) {
    Remove-Item -LiteralPath $BuildDir -Recurse -Force
}
New-Item -ItemType Directory -Force $OutDir, $BuildDir, $DexDir, $GenDir, $ClassesDir, $JniLibsDir | Out-Null

$CompiledRes = Join-Path $BuildDir "compiled-res.zip"
$BaseApk = Join-Path $BuildDir "base.apk"
$ClassesJar = Join-Path $BuildDir "classes.jar"
$UnsignedApk = Join-Path $BuildDir "video-hw-camera-smoke-unsigned.apk"
$AlignedApk = Join-Path $BuildDir "video-hw-camera-smoke-aligned.apk"
$FinalApk = Join-Path $OutDir "video-hw-camera-smoke.apk"
$NativeProject = Join-Path $ProjectDir "native-recorder"
$NativeLibSource = Join-Path $NativeProject "target\aarch64-linux-android\debug\libvideo_hw_android_camera_jni.so"
$NativeLibPackageDir = Join-Path $JniLibsDir "lib\arm64-v8a"
$NativeLibPackage = Join-Path $NativeLibPackageDir "libvideo_hw_android_camera_jni.so"

$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = $NativeLinker
& cargo build --manifest-path (Join-Path $NativeProject "Cargo.toml") --target aarch64-linux-android
if ($LASTEXITCODE -ne 0) { throw "native recorder cargo build failed" }
if (-not (Test-Path $NativeLibSource)) {
    throw "Native library not found after cargo build: $NativeLibSource"
}
New-Item -ItemType Directory -Force $NativeLibPackageDir | Out-Null
Copy-Item -LiteralPath $NativeLibSource -Destination $NativeLibPackage -Force

& $Aapt2 compile --dir (Join-Path $ProjectDir "res") -o $CompiledRes
if ($LASTEXITCODE -ne 0) { throw "aapt2 compile failed" }

& $Aapt2 link `
    -I $AndroidJar `
    --manifest (Join-Path $ProjectDir "AndroidManifest.xml") `
    --java $GenDir `
    --min-sdk-version 31 `
    --target-sdk-version 36 `
    -o $BaseApk `
    $CompiledRes
if ($LASTEXITCODE -ne 0) { throw "aapt2 link failed" }

$KotlinSources = @(Get-ChildItem -Path (Join-Path $ProjectDir "src") -Recurse -Filter *.kt | ForEach-Object { $_.FullName })
& $Kotlinc -classpath $AndroidJar -jvm-target 1.8 "-Xlambdas=class" "-Xsam-conversions=class" -d $ClassesDir @KotlinSources
if ($LASTEXITCODE -ne 0) { throw "kotlinc failed" }

& jar cf $ClassesJar -C $ClassesDir .
if ($LASTEXITCODE -ne 0) { throw "jar create failed" }

& $D8 --min-api 31 --no-desugaring --lib $AndroidJar --output $DexDir $ClassesJar $KotlinStdlib
if ($LASTEXITCODE -ne 0) { throw "d8 failed" }

Copy-Item -LiteralPath $BaseApk -Destination $UnsignedApk -Force
& jar uf $UnsignedApk -C $DexDir classes.dex
if ($LASTEXITCODE -ne 0) { throw "jar update failed" }
& jar uf $UnsignedApk -C $JniLibsDir lib
if ($LASTEXITCODE -ne 0) { throw "jar native lib update failed" }

if (-not (Test-Path $Keystore)) {
    & keytool -genkeypair `
        -keystore $Keystore `
        -storepass android `
        -keypass android `
        -alias androiddebugkey `
        -keyalg RSA `
        -keysize 2048 `
        -validity 10000 `
        -dname "CN=Android Debug,O=video-hw,C=JP"
    if ($LASTEXITCODE -ne 0) { throw "keytool failed" }
}

& $Zipalign -f 4 $UnsignedApk $AlignedApk
if ($LASTEXITCODE -ne 0) { throw "zipalign failed" }

& $ApkSigner sign `
    --ks $Keystore `
    --ks-pass pass:android `
    --key-pass pass:android `
    --out $FinalApk `
    $AlignedApk
if ($LASTEXITCODE -ne 0) { throw "apksigner sign failed" }

& $ApkSigner verify --verbose $FinalApk
if ($LASTEXITCODE -ne 0) { throw "apksigner verify failed" }

Write-Output $FinalApk
