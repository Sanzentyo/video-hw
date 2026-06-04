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
import android.media.MediaCodec
import android.media.MediaExtractor
import android.media.MediaFormat
import android.media.MediaRecorder
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
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

class CameraSmokeActivity : Activity() {
    private lateinit var textureView: TextureView
    private lateinit var statusView: TextView
    private lateinit var cameraThread: HandlerThread
    private lateinit var cameraHandler: Handler
    private lateinit var cameraExecutor: Executor

    private var cameraDevice: CameraDevice? = null
    private var captureSession: CameraCaptureSession? = null
    private var recorder: MediaRecorder? = null
    private var outputFile: File? = null
    private var autoStarted = false
    private var recording = false

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
            if (checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
                return
            }
            manager.openCamera(
                cameraId,
                cameraExecutor,
                object : CameraDevice.StateCallback() {
                    override fun onOpened(camera: CameraDevice) {
                        Log.i(TAG, "CAMERA_OPENED")
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

    private fun startPreview() {
        try {
            closeCaptureSession()
            val camera = checkNotNull(cameraDevice)
            val previewSurface = createPreviewSurface()
            val request = camera.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW).apply {
                addTarget(previewSurface)
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

        try {
            closeCaptureSession()
            val file = File(getExternalFilesDir(null), "video_hw_camera_smoke.mp4")
            outputFile = file
            setupRecorder(file)

            val camera = checkNotNull(cameraDevice)
            val activeRecorder = checkNotNull(recorder)
            val recordSurface = activeRecorder.surface
            val request = camera.createCaptureRequest(CameraDevice.TEMPLATE_RECORD).apply {
                addTarget(recordSurface)
            }
            val surfaces = mutableListOf(recordSurface)
            createPreviewSurfaceOrNull()?.let { previewSurface ->
                request.addTarget(previewSurface)
                surfaces.add(previewSurface)
            }

            createCameraSession(
                camera = camera,
                surfaces = surfaces,
                onConfigured = { session ->
                    captureSession = session
                    session.setRepeatingRequest(request.build(), null, cameraHandler)
                    activeRecorder.start()
                    recording = true
                    setStatus("Recording ${file.name}")
                    Log.i(TAG, "RECORD_START path=${file.absolutePath}")
                    cameraHandler.postDelayed({ runOnUiThread { stopRecordingAndDecode() } }, 3_000)
                },
                onFailed = {
                    Log.e(TAG, "RECORD_CONFIG_FAIL")
                    setStatus("Record configure failed")
                },
            )
        } catch (error: Exception) {
            Log.e(TAG, "START_RECORDING_FAIL", error)
            setStatus("Recording failed: ${error.message}")
        }
    }

    private fun setupRecorder(file: File) {
        recorder?.release()
        recorder = MediaRecorder(this).apply {
            setVideoSource(MediaRecorder.VideoSource.SURFACE)
            setOutputFormat(MediaRecorder.OutputFormat.MPEG_4)
            setOutputFile(file.absolutePath)
            setVideoEncoder(MediaRecorder.VideoEncoder.H264)
            setVideoSize(RECORD_SIZE.width, RECORD_SIZE.height)
            setVideoFrameRate(30)
            setVideoEncodingBitRate(1_000_000)
            prepare()
        }
    }

    private fun stopRecordingAndDecode() {
        if (!recording) {
            return
        }

        val file = outputFile ?: return
        try {
            closeCaptureSession()
            recorder?.stop()
            recorder?.release()
            recorder = null
            recording = false

            val bytes = file.length()
            Log.i(TAG, "RECORD_DONE path=${file.absolutePath} bytes=$bytes")
            setStatus("Recorded $bytes bytes. Decoding...")
            cameraHandler.post { decodeRecordedFile(file) }
            if (textureView.isAvailable) {
                startPreview()
            }
        } catch (error: Exception) {
            Log.e(TAG, "STOP_RECORDING_FAIL", error)
            setStatus("Stop failed: ${error.message}")
        }
    }

    private fun decodeRecordedFile(file: File) {
        val extractor = MediaExtractor()
        var decoder: MediaCodec? = null
        var decodedFrames = 0

        try {
            extractor.setDataSource(file.absolutePath)
            val trackIndex = selectVideoTrack(extractor)
            check(trackIndex >= 0) { "no video track" }
            extractor.selectTrack(trackIndex)

            val format = extractor.getTrackFormat(trackIndex)
            val mime = checkNotNull(format.getString(MediaFormat.KEY_MIME))
            decoder = MediaCodec.createDecoderByType(mime).also {
                it.configure(format, null, null, 0)
                it.start()
            }

            val info = MediaCodec.BufferInfo()
            var inputDone = false
            var outputDone = false
            while (!outputDone) {
                if (!inputDone) {
                    val inputIndex = decoder.dequeueInputBuffer(10_000)
                    if (inputIndex >= 0) {
                        val input = checkNotNull(decoder.getInputBuffer(inputIndex))
                        val sampleSize = extractor.readSampleData(input, 0)
                        if (sampleSize < 0) {
                            decoder.queueInputBuffer(
                                inputIndex,
                                0,
                                0,
                                0,
                                MediaCodec.BUFFER_FLAG_END_OF_STREAM,
                            )
                            inputDone = true
                        } else {
                            decoder.queueInputBuffer(
                                inputIndex,
                                0,
                                sampleSize,
                                extractor.sampleTime,
                                extractor.sampleFlags,
                            )
                            extractor.advance()
                        }
                    }
                }

                val outputIndex = decoder.dequeueOutputBuffer(info, 10_000)
                if (outputIndex >= 0) {
                    if (info.size > 0) {
                        decodedFrames += 1
                    }
                    outputDone = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                    decoder.releaseOutputBuffer(outputIndex, false)
                }
            }

            val result = "DECODE_PASS frames=$decodedFrames bytes=${file.length()}"
            Log.i(TAG, result)
            setStatus(result)
        } catch (error: Exception) {
            Log.e(TAG, "DECODE_FAIL", error)
            setStatus("Decode failed: ${error.message}")
        } finally {
            extractor.release()
            decoder?.stop()
            decoder?.release()
        }
    }

    private fun selectVideoTrack(extractor: MediaExtractor): Int {
        for (index in 0 until extractor.trackCount) {
            val format = extractor.getTrackFormat(index)
            val mime = format.getString(MediaFormat.KEY_MIME)
            if (mime?.startsWith("video/") == true) {
                return index
            }
        }
        return -1
    }

    private fun createPreviewSurface(): Surface {
        val texture = checkNotNull(textureView.surfaceTexture)
        texture.setDefaultBufferSize(RECORD_SIZE.width, RECORD_SIZE.height)
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
        recorder?.release()
        recorder = null
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
        private val RECORD_SIZE = Size(640, 360)
    }
}
