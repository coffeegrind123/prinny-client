package `in`.prinny.app

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.View
import android.view.ViewGroup
import android.webkit.ConsoleMessage
import android.webkit.GeolocationPermissions
import android.webkit.JsPromptResult
import android.webkit.JsResult
import android.webkit.MimeTypeMap
import android.webkit.PermissionRequest
import android.webkit.ValueCallback
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

        // Cold start from the system share sheet. Same stash-and-replay
        // arrangement, in ShareTargetPlugin.
        handleShareIntent(intent)
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
        handleShareIntent(intent)
    }

    /**
     * Turns an ACTION_SEND / ACTION_SEND_MULTIPLE intent into a share payload
     * for the frontend, which opens a room picker.
     *
     * The activity is exported, so any app on the device can start it with any
     * text and any content URI. Nothing here is trusted or acted on: the text
     * ends up in a composer the user still has to send, and the URIs never
     * leave Kotlin — ShareTargetPlugin hands JS opaque one-shot tokens instead.
     * See its class comment for why.
     */
    private fun handleShareIntent(intent: Intent?) {
        if (intent == null) return
        if (intent.action != Intent.ACTION_SEND && intent.action != Intent.ACTION_SEND_MULTIPLE) {
            return
        }

        val payload = ShareTargetPlugin.buildPayload(
            intent.getCharSequenceExtra(Intent.EXTRA_TEXT),
            intent.getCharSequenceExtra(Intent.EXTRA_SUBJECT),
            extraStreamUris(intent),
        )
        if (payload == null) {
            Log.w(TAG, "Share intent carried no text and no readable stream")
            return
        }

        // Consume it. With launchMode=singleTask the same Intent stays on the
        // activity until another replaces it, so without this a configuration
        // change (rotation, theme switch) re-runs onCreate against the old
        // intent and pops the room picker again for a share already handled.
        intent.removeExtra(Intent.EXTRA_TEXT)
        intent.removeExtra(Intent.EXTRA_SUBJECT)
        intent.removeExtra(Intent.EXTRA_STREAM)
        intent.action = Intent.ACTION_MAIN

        ShareTargetPlugin.deliverShare(payload)
    }

    /**
     * EXTRA_STREAM is a single Uri for ACTION_SEND and an ArrayList<Uri> for
     * ACTION_SEND_MULTIPLE — but a sending app is free to get that wrong, so
     * both shapes are tried regardless of the action.
     *
     * The typed overloads are required from API 33; the untyped ones are
     * deprecated there but are the only option below it.
     */
    @Suppress("DEPRECATION")
    private fun extraStreamUris(intent: Intent): List<Uri> {
        val uris = mutableListOf<Uri>()

        // A malformed or hostile parcel throws out of getParcelableExtra rather
        // than returning null, and it must not take the whole share with it.
        try {
            val single: Uri? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableExtra(Intent.EXTRA_STREAM) as? Uri
            }
            single?.let { uris.add(it) }
        } catch (err: Throwable) {
            Log.w(TAG, "Malformed single EXTRA_STREAM", err)
        }

        try {
            val many: List<Uri>? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)
            }
            many?.filterNotNull()?.let { uris.addAll(it) }
        } catch (err: Throwable) {
            Log.w(TAG, "Malformed EXTRA_STREAM list", err)
        }

        return uris.distinct()
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

    /**
     * Wraps the WebView's existing chrome client rather than replacing it.
     *
     * This used to assign `RtcChromeClient()` outright, and that quietly threw
     * away wry's `RustWebChromeClient` — which Tauri installs during onCreate
     * (it registers activity-result launchers, so it cannot be built later) and
     * which is the only implementation of `onShowFileChooser` in the app. With
     * it gone, `<input type="file">.click()` — every attachment button in the
     * client — reached the base `WebChromeClient`, whose implementation returns
     * false and opens nothing. No error, no callback: the picker simply did not
     * appear. `window.alert/confirm/prompt`, the geolocation prompt, WebView
     * console output in logcat and title changes went with it.
     *
     * `getWebChromeClient()` is API 26+, so on Android 7.x there is nothing to
     * delegate to; `RtcChromeClient` handles the file chooser itself in that
     * case (see `showFilePicker`), and the rest of the behaviour falls back to
     * the platform default.
     */
    private fun installRtcPermissionHandler() {
        if (chromeClientInstalled) return
        val webView = findWebView(window.decorView as? ViewGroup) ?: return
        val delegate = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            webView.webChromeClient
        } else {
            null
        }
        if (delegate == null) {
            Log.w(TAG, "No existing WebChromeClient to delegate to; using built-in fallbacks")
        }
        webView.webChromeClient = RtcChromeClient(delegate)
        chromeClientInstalled = true
    }

    /**
     * The `<input type="file">` callback waiting on a picker we launched
     * ourselves. Only used when there is no wry client to delegate to.
     */
    private var pendingFileChooserCallback: ValueCallback<Array<Uri>>? = null

    /**
     * Registered as a field so it is created while the activity is still
     * INITIALIZED — `registerForActivityResult` throws once the activity has
     * STARTED, which is why this cannot live inside the chrome client itself.
     */
    private val fileChooserLauncher =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            val callback = pendingFileChooserCallback ?: return@registerForActivityResult
            pendingFileChooserCallback = null

            val data = result.data
            val clipData = data?.clipData
            val uris: Array<Uri>? = when {
                result.resultCode != Activity.RESULT_OK -> null
                clipData != null -> Array(clipData.itemCount) { i -> clipData.getItemAt(i).uri }
                else -> WebChromeClient.FileChooserParams.parseResult(result.resultCode, data)
            }
            // Always answer, including with null: the WebView keeps the file
            // input blocked until the callback fires, so a dropped answer means
            // the button never works again for the life of the page.
            callback.onReceiveValue(uris)
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

    /**
     * Launches the system picker for a file input.
     *
     * Only reached when there is no wry client to delegate to (API < 26).
     * Mirrors what `RustWebChromeClient.showFilePicker` does, minus the camera
     * capture branch: `FileChooserParams.createIntent()` already encodes the
     * `accept` attribute, and the extra MIME types cover an `accept` listing
     * several types or bare extensions, which the single `type` field cannot.
     */
    private fun showFilePicker(
        filePathCallback: ValueCallback<Array<Uri>>,
        fileChooserParams: WebChromeClient.FileChooserParams
    ): Boolean {
        // A picker already in flight would strand the earlier callback, so
        // answer it before taking the new one.
        pendingFileChooserCallback?.onReceiveValue(null)
        pendingFileChooserCallback = filePathCallback

        val intent = fileChooserParams.createIntent()
        if (fileChooserParams.mode == WebChromeClient.FileChooserParams.MODE_OPEN_MULTIPLE) {
            intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
        }

        val acceptTypes = fileChooserParams.acceptTypes ?: emptyArray()
        val type = intent.type
        if (acceptTypes.size > 1 || type?.startsWith(".") == true) {
            val mimeTypeMap = MimeTypeMap.getSingleton()
            val validTypes = acceptTypes
                .mapNotNull { accept ->
                    when {
                        accept.isEmpty() -> null
                        accept.startsWith(".") ->
                            mimeTypeMap.getMimeTypeFromExtension(accept.substring(1))
                        else -> accept
                    }
                }
                .distinct()
            if (validTypes.isNotEmpty()) {
                intent.putExtra(Intent.EXTRA_MIME_TYPES, validTypes.toTypedArray())
                if (type?.startsWith(".") == true) intent.type = validTypes[0]
            }
        }

        return try {
            fileChooserLauncher.launch(intent)
            true
        } catch (e: ActivityNotFoundException) {
            Log.w(TAG, "No activity can handle the file chooser intent", e)
            pendingFileChooserCallback = null
            filePathCallback.onReceiveValue(null)
            false
        }
    }

    /**
     * Adds an origin gate to the WebView's chrome client without discarding the
     * rest of it.
     *
     * Every method wry's `RustWebChromeClient` implements is forwarded here, so
     * the only behaviour that changes is `onPermissionRequest`. Forwarding has
     * to be explicit and exhaustive — `RustWebChromeClient` is a final Kotlin
     * class, so it cannot be subclassed, and Kotlin's `by` delegation needs an
     * interface, which `WebChromeClient` is not. A method missing from this
     * list silently reverts to the platform default, which is how the file
     * chooser was lost in the first place.
     */
    private inner class RtcChromeClient(
        private val delegate: WebChromeClient?
    ) : WebChromeClient() {
        override fun onShowFileChooser(
            webView: WebView?,
            filePathCallback: ValueCallback<Array<Uri>>?,
            fileChooserParams: FileChooserParams?
        ): Boolean {
            delegate?.let { return it.onShowFileChooser(webView, filePathCallback, fileChooserParams) }
            if (filePathCallback == null || fileChooserParams == null) return false
            return showFilePicker(filePathCallback, fileChooserParams)
        }

        override fun onShowCustomView(view: View?, callback: CustomViewCallback?) {
            if (delegate != null) {
                delegate.onShowCustomView(view, callback)
            } else {
                super.onShowCustomView(view, callback)
            }
        }

        override fun onHideCustomView() {
            if (delegate != null) {
                delegate.onHideCustomView()
            } else {
                super.onHideCustomView()
            }
        }

        override fun onJsAlert(
            view: WebView?,
            url: String?,
            message: String?,
            result: JsResult?
        ): Boolean =
            delegate?.onJsAlert(view, url, message, result)
                ?: super.onJsAlert(view, url, message, result)

        override fun onJsConfirm(
            view: WebView?,
            url: String?,
            message: String?,
            result: JsResult?
        ): Boolean =
            delegate?.onJsConfirm(view, url, message, result)
                ?: super.onJsConfirm(view, url, message, result)

        override fun onJsPrompt(
            view: WebView?,
            url: String?,
            message: String?,
            defaultValue: String?,
            result: JsPromptResult?
        ): Boolean =
            delegate?.onJsPrompt(view, url, message, defaultValue, result)
                ?: super.onJsPrompt(view, url, message, defaultValue, result)

        override fun onGeolocationPermissionsShowPrompt(
            origin: String?,
            callback: GeolocationPermissions.Callback?
        ) {
            // Same origin rule as capture: geolocation is not something an
            // embedded third-party frame gets to ask for. wry's handler prompts
            // for any origin, so the gate has to be here rather than delegated.
            val requestOrigin = origin?.trimEnd('/') ?: ""
            if (requestOrigin !in ALLOWED_CAPTURE_ORIGINS) {
                Log.w(TAG, "Denying geolocation request from disallowed origin: $origin")
                callback?.invoke(origin, false, false)
                return
            }
            if (delegate != null) {
                delegate.onGeolocationPermissionsShowPrompt(origin, callback)
            } else {
                super.onGeolocationPermissionsShowPrompt(origin, callback)
            }
        }

        override fun onConsoleMessage(consoleMessage: ConsoleMessage?): Boolean =
            delegate?.onConsoleMessage(consoleMessage) ?: super.onConsoleMessage(consoleMessage)

        override fun onReceivedTitle(view: WebView?, title: String?) {
            // Not cosmetic: wry routes this into `Rust.handleReceivedTitle`,
            // which backs the window-title APIs.
            if (delegate != null) {
                delegate.onReceivedTitle(view, title)
            } else {
                super.onReceivedTitle(view, title)
            }
        }

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
