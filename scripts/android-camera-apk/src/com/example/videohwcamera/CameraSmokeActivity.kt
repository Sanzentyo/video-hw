package com.example.videohwcamera

import android.Manifest
import android.app.Activity
import android.app.KeyguardManager
import android.content.pm.PackageManager
import android.graphics.SurfaceTexture
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.hardware.camera2.params.StreamConfigurationMap
import android.media.MediaCodec
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.util.Log
import android.util.Range
import android.util.Size
import android.view.Gravity
import android.view.Surface
import android.view.TextureView
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import java.io.File
import java.util.concurrent.Executor
import kotlin.math.max

class CameraSmokeActivity : Activity() {
    private lateinit var textureView: TextureView
    private lateinit var statusView: TextView
    private lateinit var cameraThread: HandlerThread
    private lateinit var cameraHandler: Handler
    private lateinit var cameraExecutor: Executor

    private var cameraDevice: CameraDevice? = null
    private var captureSession: CameraCaptureSession? = null
    private var encoderSurface: Surface? = null
    private var outputFile: File? = null
    private var autoStarted = false
    private var recording = false
    private var selectedCameraId: String? = null
    private var profileCandidates: List<RecordingProfile> = emptyList()
    private var activeRecordingProfile: RecordingProfile? = null
    private var nativeRecorderHandle = 0L
    private var packetsDrained = 0
    private var recordingStartedAtMs = 0L
    private var stopRequested = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setShowWhenLocked(true)
        setTurnScreenOn(true)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        requestKeyguardDismiss()
        setupUi()
        startCameraThread()

        if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            openWhenTextureReady()
        } else {
            requestPermissions(arrayOf(Manifest.permission.CAMERA), REQ_CAMERA)
        }
    }

    private fun requestKeyguardDismiss() {
        getSystemService(KeyguardManager::class.java).requestDismissKeyguard(
            this,
            object : KeyguardManager.KeyguardDismissCallback() {
                override fun onDismissSucceeded() {
                    Log.i(TAG, "KEYGUARD_DISMISS_SUCCEEDED")
                }

                override fun onDismissCancelled() {
                    Log.i(TAG, "KEYGUARD_DISMISS_CANCELLED")
                }

                override fun onDismissError() {
                    Log.w(TAG, "KEYGUARD_DISMISS_ERROR")
                }
            },
        )
        Log.i(TAG, "APP_CREATE showWhenLocked=true turnScreenOn=true")
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == REQ_CAMERA &&
            grantResults.isNotEmpty() &&
            grantResults[0] == PackageManager.PERMISSION_GRANTED
        ) {
            openWhenTextureReady()
        } else {
            setStatus("Camera permission denied")
            Log.e(TAG, "CAMERA_PERMISSION_DENIED")
        }
    }

    override fun onDestroy() {
        Log.i(TAG, "APP_DESTROY")
        closeSession()
        stopCameraThread()
        super.onDestroy()
    }

    private fun setupUi() {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            setBackgroundColor(0xfff7f7f4.toInt())
        }

        textureView = TextureView(this)
        root.addView(
            textureView,
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1.0f),
        )

        statusView = TextView(this).apply {
            textSize = 14f
            setTextColor(0xff102027.toInt())
            setPadding(24, 18, 24, 18)
            text = "Starting"
        }
        root.addView(
            statusView,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )

        val buttons = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setPadding(16, 0, 16, 16)
        }
        buttons.addView(
            Button(this).apply {
                text = "Record"
                setOnClickListener { startRecording() }
            },
            LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1.0f),
        )
        buttons.addView(
            Button(this).apply {
                text = "Stop"
                setOnClickListener { stopRecordingAndDecode() }
            },
            LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1.0f),
        )
        root.addView(buttons)

        setContentView(root)
    }

    private fun startCameraThread() {
        cameraThread = HandlerThread("video-hw-camera").also { it.start() }
        cameraHandler = Handler(cameraThread.looper)
        cameraExecutor = Executor { command -> cameraHandler.post(command) }
    }

    private fun stopCameraThread() {
        cameraThread.quitSafely()
        cameraThread.join()
    }

    private fun openWhenTextureReady() {
        if (textureView.isAvailable) {
            openCamera()
            return
        }

        textureView.surfaceTextureListener = object : TextureView.SurfaceTextureListener {
            override fun onSurfaceTextureAvailable(surface: SurfaceTexture, width: Int, height: Int) {
                Log.i(TAG, "TEXTURE_AVAILABLE width=$width height=$height")
                if (cameraDevice == null) {
                    openCamera()
                } else if (!recording) {
                    startPreview()
                }
            }

            override fun onSurfaceTextureSizeChanged(surface: SurfaceTexture, width: Int, height: Int) = Unit

            override fun onSurfaceTextureDestroyed(surface: SurfaceTexture): Boolean = true

            override fun onSurfaceTextureUpdated(surface: SurfaceTexture) = Unit
        }
        openCamera()
    }

    private fun openCamera() {
        try {
            val manager = getSystemService(CameraManager::class.java)
            val cameraId = chooseBackCamera(manager)
            selectedCameraId = cameraId
            profileCandidates = recordingProfiles(manager, cameraId)
            if (checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
                return
            }
            manager.openCamera(
                cameraId,
                cameraExecutor,
                object : CameraDevice.StateCallback() {
                    override fun onOpened(camera: CameraDevice) {
                        Log.i(TAG, "CAMERA_OPENED id=$cameraId")
                        cameraDevice = camera
                        if (textureView.isAvailable) {
                            startPreview()
                        } else {
                            setStatus("Camera opened. Recording without preview.")
                            Log.i(TAG, "CAMERA_OPENED_NO_TEXTURE")
                            scheduleAutoRecording()
                        }
                    }

                    override fun onDisconnected(camera: CameraDevice) {
                        Log.i(TAG, "CAMERA_DISCONNECTED")
                        camera.close()
                        cameraDevice = null
                    }

                    override fun onError(camera: CameraDevice, error: Int) {
                        Log.e(TAG, "CAMERA_ERROR error=$error")
                        setStatus("Camera error $error")
                        camera.close()
                        cameraDevice = null
                    }
                },
            )
        } catch (error: Exception) {
            Log.e(TAG, "OPEN_CAMERA_FAIL", error)
            setStatus("Open camera failed: ${error.message}")
        }
    }

    private fun chooseBackCamera(manager: CameraManager): String {
        return manager.cameraIdList.firstOrNull { id ->
            val facing = manager.getCameraCharacteristics(id)
                .get(CameraCharacteristics.LENS_FACING)
            facing == CameraCharacteristics.LENS_FACING_BACK
        } ?: manager.cameraIdList.first()
    }

    private fun recordingProfiles(manager: CameraManager, cameraId: String): List<RecordingProfile> {
        val characteristics = manager.getCameraCharacteristics(cameraId)
        val streamMap = characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
        val sizes = streamMap?.mediaCodecSizes().orEmpty()
        val ranges = characteristics.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES)
            ?.toList()
            .orEmpty()
            .ifEmpty { listOf(Range(30, 30)) }

        Log.i(TAG, "SURFACE_CODEC_SIZES ${sizes.joinToString { "${it.width}x${it.height}" }}")
        Log.i(TAG, "FPS_RANGES ${ranges.joinToString { "${it.lower}-${it.upper}" }}")

        return sizes
            .flatMap { size -> ranges.map { range -> RecordingProfile(size, range) } }
            .sortedWith(
                compareByDescending<RecordingProfile> { it.size.width.toLong() * it.size.height }
                    .thenByDescending { it.fpsRange.upper }
                    .thenByDescending { it.fpsRange.lower },
            )
            .distinctBy { "${it.size.width}x${it.size.height}@${it.fpsRange.lower}-${it.fpsRange.upper}" }
            .also { candidates ->
                Log.i(
                    TAG,
                    "PROFILE_CANDIDATES ${
                        candidates.take(24).joinToString { it.describe() }
                    } total=${candidates.size}",
                )
            }
    }

    private fun startPreview() {
        try {
            closeCaptureSession()
            val camera = checkNotNull(cameraDevice)
            val previewSurface = createPreviewSurface()
            val request = camera.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW).apply {
                addTarget(previewSurface)
                profileCandidates.firstOrNull()?.let {
                    set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, it.fpsRange)
                }
            }
            createCameraSession(
                camera = camera,
                surfaces = listOf(previewSurface),
                onConfigured = { session ->
                    captureSession = session
                    session.setRepeatingRequest(request.build(), null, cameraHandler)
                    setStatus("Preview ready. Auto recording soon.")
                    Log.i(TAG, "PREVIEW_READY")
                    if (!autoStarted) {
                        scheduleAutoRecording()
                    }
                },
                onFailed = {
                    Log.e(TAG, "PREVIEW_CONFIG_FAIL")
                    setStatus("Preview configure failed")
                },
            )
        } catch (error: Exception) {
            Log.e(TAG, "START_PREVIEW_FAIL", error)
            setStatus("Preview failed: ${error.message}")
        }
    }

    private fun startRecording() {
        if (recording || cameraDevice == null) {
            return
        }
        startRecordingWithProfile(0)
    }

    private fun startRecordingWithProfile(profileIndex: Int) {
        val profile = profileCandidates.getOrNull(profileIndex) ?: RecordingProfile(FALLBACK_RECORD_SIZE, Range(30, 30))
        try {
            closeCaptureSession()
            activeRecordingProfile = profile
            val file = File(
                getExternalFilesDir(null),
                "video_hw_camera_rust_${profile.size.width}x${profile.size.height}_${profile.fpsRange.upper}fps.mp4",
            )
            outputFile = file
            nativeRecorderHandle = RustRecorder.nativeStart(
                file.absolutePath,
                profile.size.width,
                profile.size.height,
                profile.fpsRange.upper,
                profile.bitrate,
            )
            check(nativeRecorderHandle != 0L) { "Rust recorder failed to start" }
            val recordSurface = RustRecorder.nativeInputSurface(nativeRecorderHandle)
            check(recordSurface != null) { "Rust recorder did not return an input surface" }
            encoderSurface = recordSurface
            packetsDrained = 0
            stopRequested = false

            val camera = checkNotNull(cameraDevice)
            val request = camera.createCaptureRequest(CameraDevice.TEMPLATE_RECORD).apply {
                addTarget(recordSurface)
                set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, profile.fpsRange)
            }
            val surfaces = listOf(recordSurface)

            createCameraSession(
                camera = camera,
                surfaces = surfaces,
                onConfigured = { session ->
                    captureSession = session
                    session.setRepeatingRequest(request.build(), null, cameraHandler)
                    recording = true
                    recordingStartedAtMs = SystemClock.elapsedRealtime()
                    scheduleSurfaceDrain()
                    cameraHandler.postDelayed({ requestStopRecording("time limit") }, SURFACE_RECORDING_MS)
                    setStatus("Recording ${file.name}")
                    Log.i(TAG, "RUST_SURFACE_RECORD_START profile=${profile.describe()} path=${file.absolutePath}")
                },
                onFailed = {
                    Log.e(TAG, "RECORD_CONFIG_FAIL profile=${profile.describe()}")
                    retryRecordingProfile(profileIndex, "configure failed")
                },
            )
        } catch (error: Exception) {
            Log.e(TAG, "START_RECORDING_FAIL profile=${profile.describe()}", error)
            retryRecordingProfile(profileIndex, error.message ?: error.javaClass.simpleName)
        }
    }

    private fun retryRecordingProfile(currentIndex: Int, reason: String) {
        encoderSurface?.release()
        encoderSurface = null
        if (nativeRecorderHandle != 0L) {
            RustRecorder.nativeFinish(nativeRecorderHandle)
            nativeRecorderHandle = 0L
        }
        activeRecordingProfile = null
        val nextIndex = currentIndex + 1
        if (nextIndex < profileCandidates.size) {
            val next = profileCandidates[nextIndex]
            setStatus("Retry recording ${next.describe()}")
            Log.i(TAG, "RECORD_RETRY next=${next.describe()} reason=$reason")
            cameraHandler.post { runOnUiThread { startRecordingWithProfile(nextIndex) } }
        } else {
            Log.e(TAG, "RECORD_NO_PROFILE_WORKED reason=$reason")
            setStatus("No recording profile worked: $reason")
        }
    }

    private fun requestStopRecording(reason: String) {
        if (stopRequested) {
            return
        }
        stopRequested = true
        Log.i(TAG, "RUST_SURFACE_RECORD_STOP_REQUEST reason=$reason packets=$packetsDrained")
        runOnUiThread { stopRecordingAndDecode() }
    }

    private fun scheduleSurfaceDrain() {
        cameraHandler.postDelayed(
            {
                if (recording && nativeRecorderHandle != 0L) {
                    val drained = RustRecorder.nativeDrain(nativeRecorderHandle)
                    if (drained >= 0) {
                        packetsDrained = drained
                        if (drained == 1 || drained % 30 == 0) {
                            Log.i(TAG, "RUST_SURFACE_DRAIN packets=$drained")
                        }
                    } else {
                        Log.e(TAG, "RUST_SURFACE_DRAIN_FAIL")
                    }
                    scheduleSurfaceDrain()
                }
            },
            SURFACE_DRAIN_INTERVAL_MS,
        )
    }

    private fun stopRecordingAndDecode() {
        if (!recording) {
            return
        }

        val file = outputFile ?: return
        try {
            closeCaptureSession()
            recording = false

            val result = if (nativeRecorderHandle != 0L) {
                RustRecorder.nativeFinish(nativeRecorderHandle)
            } else {
                "{\"status\":\"FAIL\",\"error\":\"missing native recorder handle\"}"
            }
            nativeRecorderHandle = 0L
            encoderSurface?.release()
            encoderSurface = null
            val bytes = file.length()
            Log.i(TAG, "RUST_SURFACE_RECORD_DONE profile=${activeRecordingProfile?.describe()} path=${file.absolutePath} bytes=$bytes")
            Log.i(TAG, "RUST_SURFACE_RECORD_RESULT $result")
            setStatus("Rust result $result")
            if (textureView.isAvailable) {
                startPreview()
            }
        } catch (error: Exception) {
            Log.e(TAG, "STOP_RUST_RECORDING_FAIL", error)
            setStatus("Rust stop failed: ${error.message}")
        }
    }

    private fun createPreviewSurface(): Surface {
        val texture = checkNotNull(textureView.surfaceTexture)
        texture.setDefaultBufferSize(PREVIEW_SIZE.width, PREVIEW_SIZE.height)
        return Surface(texture)
    }

    private fun createPreviewSurfaceOrNull(): Surface? {
        if (!textureView.isAvailable) {
            Log.i(TAG, "PREVIEW_SURFACE_SKIPPED")
            return null
        }
        return createPreviewSurface()
    }

    private fun scheduleAutoRecording() {
        autoStarted = true
        cameraHandler.postDelayed({ runOnUiThread { startRecording() } }, 1_000)
    }

    private fun createCameraSession(
        camera: CameraDevice,
        surfaces: List<Surface>,
        onConfigured: (CameraCaptureSession) -> Unit,
        onFailed: () -> Unit,
    ) {
        val callback = object : CameraCaptureSession.StateCallback() {
            override fun onConfigured(session: CameraCaptureSession) = onConfigured(session)

            override fun onConfigureFailed(session: CameraCaptureSession) = onFailed()
        }
        val config = SessionConfiguration(
            SessionConfiguration.SESSION_REGULAR,
            surfaces.map(::OutputConfiguration),
            cameraExecutor,
            callback,
        )
        camera.createCaptureSession(config)
    }

    private fun closeSession() {
        closeCaptureSession()
        cameraDevice?.close()
        cameraDevice = null
        encoderSurface?.release()
        encoderSurface = null
        if (nativeRecorderHandle != 0L) {
            RustRecorder.nativeFinish(nativeRecorderHandle)
            nativeRecorderHandle = 0L
        }
    }

    private fun closeCaptureSession() {
        captureSession?.close()
        captureSession = null
    }

    private fun setStatus(status: String) {
        Log.i(TAG, "STATUS $status")
        runOnUiThread { statusView.text = status }
    }

    companion object {
        private const val TAG = "VideoHwCameraSmoke"
        private const val REQ_CAMERA = 100
        private const val SURFACE_RECORDING_MS = 3_000L
        private const val SURFACE_DRAIN_INTERVAL_MS = 50L
        private val PREVIEW_SIZE = Size(1280, 720)
        private val FALLBACK_RECORD_SIZE = Size(640, 360)
    }
}

private object RustRecorder {
    init {
        System.loadLibrary("video_hw_android_camera_jni")
    }

    external fun nativeStart(
        outputPath: String,
        width: Int,
        height: Int,
        fps: Int,
        bitrate: Int,
    ): Long

    external fun nativeInputSurface(handle: Long): Surface?

    external fun nativeDrain(handle: Long): Int

    external fun nativeFinish(handle: Long): String
}

private data class RecordingProfile(
    val size: Size,
    val fpsRange: Range<Int>,
) {
    val bitrate: Int =
        (size.width.toLong() * size.height.toLong() * max(fpsRange.upper, 30) / 4)
            .coerceIn(1_000_000, 80_000_000)
            .toInt()

    fun describe(): String =
        "${size.width}x${size.height}@${fpsRange.lower}-${fpsRange.upper}fps/${bitrate}bps"
}

private fun StreamConfigurationMap.mediaCodecSizes(): List<Size> =
    getOutputSizes(MediaCodec::class.java)?.toList().orEmpty()
