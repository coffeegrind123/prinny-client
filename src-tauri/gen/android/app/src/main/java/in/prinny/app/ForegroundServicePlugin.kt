package `in`.prinny.app

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import android.webkit.WebView
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.JSObject
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

@TauriPlugin
class ForegroundServicePlugin(private val activity: Activity) : Plugin(activity) {

    @Command
    fun startForeground(invoke: Invoke) {
        val intent = Intent(activity, ForegroundService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            activity.startForegroundService(intent)
        } else {
            activity.startService(intent)
        }
        invoke.resolve()
    }

    @Command
    fun stopForeground(invoke: Invoke) {
        val intent = Intent(activity, ForegroundService::class.java)
        activity.stopService(intent)
        invoke.resolve()
    }

    @Command
    fun isForegroundRunning(invoke: Invoke) {
        val result = JSObject()
        result.put("running", ForegroundService.isRunning)
        invoke.resolve(result)
    }

    /**
     * Enabling the `microphone` foreground-service type is what lets capture
     * continue while the app is backgrounded, so it must not be a freely
     * callable command: any JS running in the WebView could otherwise arrange
     * background mic capture. Enabling is gated on the app's own state —
     * MainActivity must have granted RESOURCE_AUDIO_CAPTURE to an allowed
     * origin (see MainActivity.ALLOWED_CAPTURE_ORIGINS) AND the OS runtime
     * RECORD_AUDIO grant must still be held (the user can revoke it from
     * Settings at any time).
     *
     * Disabling is always permitted — dropping the mic type can only ever
     * reduce what the service advertises.
     *
     * A not-yet-authorized enable is DEFERRED rather than rejected: the call
     * UI legitimately asks for this on mount, a moment before the embedded
     * widget requests capture. MainActivity applies the pending request the
     * instant it grants audio capture to an allowed origin (and never
     * otherwise), so the security property holds without breaking calls.
     */
    @Command
    fun setMicrophoneActive(invoke: Invoke) {
        val active = invoke.parseArgs(SetMicrophoneActiveArgs::class.java).active
        ForegroundService.microphoneRequested = active

        if (active && !micCaptureAuthorized()) {
            Log.w(
                TAG,
                "Not enabling microphone foreground-service type yet: no WebView " +
                    "audio-capture grant for an allowed origin — deferred until one exists"
            )
            invoke.resolve()
            return
        }

        ForegroundService.setMicrophoneActive(activity, active)
        invoke.resolve()
    }

    private fun micCaptureAuthorized(): Boolean {
        if (!MainActivity.audioCaptureGranted) return false
        val recordAudio =
            ContextCompat.checkSelfPermission(activity, Manifest.permission.RECORD_AUDIO)
        return recordAudio == PackageManager.PERMISSION_GRANTED
    }

    companion object {
        private const val TAG = "ForegroundServicePlugin"
    }
}

class SetMicrophoneActiveArgs {
    var active: Boolean = false
}
