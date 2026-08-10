package `in`.prinny.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.View
import android.view.ViewGroup
import android.webkit.PermissionRequest
import android.webkit.WebChromeClient
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat

class MainActivity : TauriActivity() {
    companion object {
        private const val TAG = "MainActivity"

        /**
         * Whether the activity is currently on screen.
         *
         * UnifiedPushReceiver needs this to decide who renders an incoming
         * push. It cannot use `UnifiedPushPlugin.instance != null` for that:
         * that field is process-static and never cleared, so it stays non-null
         * long after the app leaves the screen — which is precisely why
         * background notifications silently never appeared. Android suspends
         * WebView JavaScript once the activity is not resumed, so "the plugin
         * object exists" says nothing about whether any JS can still run.
         *
         * Written only from onResume/onPause on the main thread and read from
         * the receiver's binder thread, hence @Volatile.
         */
        @Volatile
        var isAppInForeground: Boolean = false
            private set

        /**
         * Origins allowed to obtain mic/camera through the WebView.
         *
         * `onPermissionRequest` fires for ANY frame in the WebView, including
         * third-party iframes (link previews, embeds, a malicious URL a user
         * taps). Granting purely on "does the app hold RECORD_AUDIO" handed
         * capture to whatever origin asked. Only our own frontend may ask.
         *
         * The Element Call widget is NOT remote here: CallEmbed.ts builds its
         * URL from `window.location.origin` +
         * `/public/element-call/index.html`, i.e. Element Call is bundled and
         * served from the app's own origin. So there is deliberately no
         * `https://call.element.io` entry — if the frontend is ever changed to
         * embed a hosted Element Call deployment, add that exact origin here,
         * otherwise calls will fail with a denied permission request.
         *
         * Release builds load http://localhost:44548 (tauri-plugin-localhost,
         * see src-tauri/src/lib.rs). Debug builds load the Tauri asset origin
         * http(s)://tauri.localhost. 127.0.0.1 is included because the WebView
         * reports whichever loopback spelling the page was loaded with.
         */
        private val ALLOWED_CAPTURE_ORIGINS = setOf(
            "http://localhost:44548",
            "http://127.0.0.1:44548",
            "http://tauri.localhost",
            "https://tauri.localhost",
        )

        /**
         * True while an allowed origin holds a mic-capture grant we issued.
         * ForegroundServicePlugin.setMicrophoneActive consults this before it
         * will let page JS add the `microphone` foreground-service type —
         * otherwise any JS in the WebView could keep background capture alive.
         */
        @Volatile
        var audioCaptureGranted: Boolean = false
            private set

        /**
         * Printable-ASCII only: no whitespace, no control characters, no
         * non-ASCII. Matrix ids are printable ASCII by grammar, and this is
         * what keeps injected newlines/NULs out of the id we forward to JS.
         */
        private val PRINTABLE_ASCII = Regex("^[\\x21-\\x7E]+$")

        /** Matrix ids are capped well below this; 255 is the spec's own limit. */
        private const val MAX_MATRIX_ID_LEN = 255
    }

    /**
     * The Element Call iframe (and any other WebRTC content embedded in
     * the WebView) calls `navigator.mediaDevices.getUserMedia`, which
     * triggers `WebChromeClient.onPermissionRequest` on Android. Tauri's
     * default chrome client denies these requests, so calls silently
     * never connect mic/camera.
     *
     * We swap the WebView's chrome client out for one that maps each
     * requested resource to its Android runtime permission, prompts the
     * user when needed, and grants whatever the OS allowed.
     */

    private var pendingWebPermissionRequest: PermissionRequest? = null

    private val webPermissionLauncher =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { granted ->
            val req = pendingWebPermissionRequest ?: return@registerForActivityResult
            pendingWebPermissionRequest = null

            val allowed = req.resources.filter { resource ->
                when (resource) {
                    PermissionRequest.RESOURCE_AUDIO_CAPTURE ->
                        granted[Manifest.permission.RECORD_AUDIO] == true ||
                            hasRuntimePermission(Manifest.permission.RECORD_AUDIO)
                    PermissionRequest.RESOURCE_VIDEO_CAPTURE ->
                        granted[Manifest.permission.CAMERA] == true ||
                            hasRuntimePermission(Manifest.permission.CAMERA)
                    else -> false
                }
            }.toTypedArray()

            if (allowed.isNotEmpty()) {
                req.grant(allowed)
                noteAudioCaptureGrant(allowed.toList())
            } else {
                req.deny()
            }
        }

    /**
     * Records that an allowed origin now holds a mic-capture grant. This is the
     * only thing that unlocks the `microphone` foreground-service type (see
     * ForegroundServicePlugin.setMicrophoneActive) — page JS on its own must
     * not be able to turn background microphone capture on.
     */
    private fun noteAudioCaptureGrant(granted: List<String>) {
        if (!granted.contains(PermissionRequest.RESOURCE_AUDIO_CAPTURE)) return
        audioCaptureGranted = true
        // The call UI asks for the microphone foreground-service type when the
        // embed mounts, i.e. before the widget requests capture, so that call
        // is deferred by ForegroundServicePlugin. Apply it now that capture is
        // genuinely authorized — this is the ONLY path that turns it on.
        if (ForegroundService.microphoneRequested) {
            ForegroundService.setMicrophoneActive(this, true)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        UpdateChecker(this).check()

        // Tauri creates the WebView during super.onCreate via JNI. Hook
        // the chrome client on the next post so the view hierarchy is
        // settled. Retry once after a short delay in case the WebView
        // is attached lazily on some devices.
        window.decorView.post { installRtcPermissionHandler() }
        window.decorView.postDelayed({
            if (!chromeClientInstalled) installRtcPermissionHandler()
        }, 1500L)

        // Cold start from a notification tap: forward the roomId/eventId
        // extras to the JS layer so it can navigate to the room. The
        // plugin may not have loaded yet — MessageNotificationPlugin
        // stashes the click and replays it on load().
        handleNotificationIntent(intent)
    }

    // Hot start: the activity is already running and the system delivers
    // a new intent (FLAG_ACTIVITY_SINGLE_TOP is set on the PendingIntent
    // we build in MessageNotificationPlugin.show). Forward the extras
    // straight to the plugin.
    override fun onResume() {
        super.onResume()
        // See isAppInForeground: this pair is what lets UnifiedPushReceiver
        // tell "app on screen, let JS render it" from "app backgrounded, Kotlin
        // must render it because JS is suspended".
        isAppInForeground = true
    }

    override fun onPause() {
        isAppInForeground = false
        super.onPause()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleNotificationIntent(intent)
    }

    private fun handleNotificationIntent(intent: Intent?) {
        val roomId = intent?.getStringExtra("roomId") ?: return
        val eventId = intent.getStringExtra("eventId") ?: return
        // This activity is exported (LAUNCHER filter), so ANY app on the device
        // can start it with arbitrary roomId/eventId extras. Those strings are
        // handed straight to the JS layer, which navigates on them — validate
        // them against the Matrix id grammar first and drop anything else
        // silently (a rejected intent is not something to surface to the user).
        if (!isValidRoomId(roomId) || !isValidEventId(eventId)) {
            Log.w(TAG, "Dropping notification intent with malformed Matrix ids")
            return
        }
        MessageNotificationPlugin.deliverClick(roomId, eventId)
    }

    // `!localpart:server` (room id) or `#alias:server` (room alias).
    private fun isValidRoomId(value: String): Boolean {
        if (value.length < 3 || value.length > MAX_MATRIX_ID_LEN) return false
        if (value[0] != '!' && value[0] != '#') return false
        if (!PRINTABLE_ASCII.matches(value)) return false
        // Must have a server part: a ':' that is neither first nor last.
        val colon = value.indexOf(':')
        return colon > 1 && colon < value.length - 1
    }

    // `$` + opaque id (v3+ event ids are unpadded base64, older ones have a
    // `:server` suffix — both are covered by the printable-ASCII + length cap).
    private fun isValidEventId(value: String): Boolean {
        if (value.length < 2 || value.length > MAX_MATRIX_ID_LEN) return false
        if (value[0] != '$') return false
        return PRINTABLE_ASCII.matches(value)
    }

    private var chromeClientInstalled = false

    private fun installRtcPermissionHandler() {
        if (chromeClientInstalled) return
        val webView = findWebView(window.decorView as? ViewGroup) ?: return
        webView.webChromeClient = RtcChromeClient()
        chromeClientInstalled = true
    }

    private fun findWebView(root: ViewGroup?): WebView? {
        if (root == null) return null
        for (i in 0 until root.childCount) {
            val child: View = root.getChildAt(i)
            if (child is WebView) return child
            if (child is ViewGroup) {
                findWebView(child)?.let { return it }
            }
        }
        return null
    }

    private fun hasRuntimePermission(perm: String): Boolean =
        ContextCompat.checkSelfPermission(this, perm) == PackageManager.PERMISSION_GRANTED

    private inner class RtcChromeClient : WebChromeClient() {
        override fun onPermissionRequest(request: PermissionRequest) {
            // Origin gate FIRST. onPermissionRequest fires for every frame in
            // the WebView, so without this any third-party iframe the user ends
            // up loading gets mic/camera the moment the app itself holds the
            // runtime permission — no prompt, no indication. request.origin is
            // the security origin of the requesting frame (scheme://host[:port])
            // as computed by the WebView, not something page JS can forge.
            val origin = request.origin?.toString()?.trimEnd('/') ?: ""
            if (origin !in ALLOWED_CAPTURE_ORIGINS) {
                Log.w(TAG, "Denying WebView capture request from disallowed origin: $origin")
                request.deny()
                return
            }

            // RESOURCE_PROTECTED_MEDIA_ID (DRM) gets auto-granted — it's
            // the standard policy for WebView and unrelated to mic/cam.
            val resources = request.resources
            val toRequest = mutableListOf<String>()
            val canGrantImmediately = mutableListOf<String>()

            for (resource in resources) {
                when (resource) {
                    PermissionRequest.RESOURCE_AUDIO_CAPTURE -> {
                        if (hasRuntimePermission(Manifest.permission.RECORD_AUDIO)) {
                            canGrantImmediately.add(resource)
                        } else {
                            toRequest.add(Manifest.permission.RECORD_AUDIO)
                        }
                    }
                    PermissionRequest.RESOURCE_VIDEO_CAPTURE -> {
                        if (hasRuntimePermission(Manifest.permission.CAMERA)) {
                            canGrantImmediately.add(resource)
                        } else {
                            toRequest.add(Manifest.permission.CAMERA)
                        }
                    }
                    // Anything else (DRM, MIDI, etc.) — pass through to
                    // the default behavior, which denies.
                }
            }

            if (toRequest.isEmpty()) {
                if (canGrantImmediately.isNotEmpty()) {
                    request.grant(canGrantImmediately.toTypedArray())
                    noteAudioCaptureGrant(canGrantImmediately)
                } else {
                    request.deny()
                }
                return
            }

            // We need to prompt for at least one runtime permission.
            // Stash the request so the launcher callback can grant/deny
            // after the system dialog returns. Only one outstanding
            // request is supported — if a second one arrives mid-flight,
            // deny it.
            if (pendingWebPermissionRequest != null) {
                request.deny()
                return
            }
            pendingWebPermissionRequest = request
            webPermissionLauncher.launch(toRequest.toTypedArray())
        }

        override fun onPermissionRequestCanceled(request: PermissionRequest?) {
            super.onPermissionRequestCanceled(request)
            if (pendingWebPermissionRequest === request) {
                pendingWebPermissionRequest = null
            }
        }
    }
}
