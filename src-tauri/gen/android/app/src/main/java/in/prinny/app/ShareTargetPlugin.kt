package `in`.prinny.app

import android.app.Activity
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Base64
import android.util.Log
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.ByteArrayOutputStream
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicLong

@InvokeArg
class ReadSharedFileArgs {
    lateinit var token: String
}

/**
 * Android share-sheet target.
 *
 * MainActivity carries `ACTION_SEND` / `ACTION_SEND_MULTIPLE` intent filters,
 * so Prinny appears in the system share sheet of every other app. This plugin
 * is the bridge from the intent Android delivers to the frontend, which shows
 * a room picker and drops the shared text/files into that room's composer.
 *
 * ## Why JS never sees a `content://` URI
 *
 * The obvious design — hand the URIs to the frontend and give it a "read this
 * URI" command — grants page script the ability to read *any* content URI this
 * process can reach, for as long as the process lives. The URI is a capability,
 * and the frontend is the least trustworthy part of the app (it renders
 * federated message content and third-party embeds).
 *
 * So the URIs stay in Kotlin. Each one received in a share is filed under a
 * freshly minted opaque token, and `readSharedFile` accepts only a token. A
 * token is consumed on use and every outstanding token is dropped when a new
 * share arrives — one share authorises exactly one read of each of its files.
 * This is the same shape as the desktop side's `DroppedPaths` set in
 * `src-tauri/src/lib.rs`, for the same reason.
 */
@TauriPlugin
class ShareTargetPlugin(private val activity: Activity) : Plugin(activity) {
    companion object {
        private const val TAG = "ShareTarget"

        /**
         * Upper bound on one shared file.
         *
         * Deliberately far below the desktop `DROPPED_FILE_MAX_BYTES` (512
         * MiB). The bytes cross the JNI/JSON bridge as base64, which inflates
         * them by 4/3 and materialises the file in memory several times over —
         * as a byte array here, as a base64 String here, as a JS string, and
         * again as the decoded Uint8Array. A phone does not have the headroom
         * a desktop does, and Matrix homeservers cap uploads well below this
         * anyway.
         */
        private const val MAX_SHARED_FILE_BYTES = 100L * 1024L * 1024L

        /** Shared text lands in a composer, not in a send — a cap is enough. */
        private const val MAX_SHARED_TEXT_CHARS = 32_768

        /** ACTION_SEND_MULTIPLE has no bound of its own. */
        private const val MAX_SHARED_FILES = 32

        /** Filename fallback when the provider offers no display name. */
        private const val FALLBACK_FILE_NAME = "shared-file"

        private const val MAX_FILE_NAME_LEN = 200

        var instance: ShareTargetPlugin? = null

        // Plugin.load() runs during MainActivity.onCreate, long before React
        // has mounted and registered a listener. Until JS calls jsReady the
        // share is stashed and replayed; after it, shares emit live (the warm
        // path, via onNewIntent). Mirrors MessageNotificationPlugin exactly.
        private var pendingShare: JSObject? = null
        private var listenerReady = false

        /**
         * Token -> content URI, for the URIs of the CURRENT share only.
         *
         * Read and written from the main thread (intent delivery) and from the
         * reader thread (`readSharedFile`), so every access is synchronized on
         * the map itself.
         */
        private val authorizedUris = LinkedHashMap<String, Uri>()
        private val tokenSeq = AtomicLong(0)

        private fun mintToken(uri: Uri): String {
            val token = "s${tokenSeq.incrementAndGet()}"
            synchronized(authorizedUris) { authorizedUris[token] = uri }
            return token
        }

        /** Consumes the authorisation: a token is good for exactly one read. */
        private fun takeUri(token: String): Uri? =
            synchronized(authorizedUris) { authorizedUris.remove(token) }

        /**
         * A new share revokes the previous one's tokens. Without this, every
         * file ever shared into the app stays readable by page script for the
         * life of the process — the same bug the desktop `DroppedPaths` set had
         * before it started pruning.
         */
        private fun revokeAllTokens() {
            synchronized(authorizedUris) { authorizedUris.clear() }
        }

        /**
         * Called by MainActivity once it has turned an ACTION_SEND intent into
         * a payload. Emits immediately when JS is listening, stashes otherwise.
         */
        fun deliverShare(payload: JSObject) {
            if (listenerReady) {
                instance?.emitShare(payload)
            } else {
                pendingShare = payload
            }
        }

        /**
         * Builds the JS payload for a share. Returns null when the intent
         * carried nothing usable, so the caller can leave the app on whatever
         * screen it was already showing rather than opening an empty picker.
         *
         * Every value here comes from another app on the device and is treated
         * as hostile input: text is length-capped, the file list is count-
         * capped, and each display name is stripped of path separators and
         * control characters before it can become an upload filename.
         */
        fun buildPayload(text: CharSequence?, subject: CharSequence?, uris: List<Uri>): JSObject? {
            revokeAllTokens()

            val cleanText = text?.toString()?.take(MAX_SHARED_TEXT_CHARS)?.trim().orEmpty()
            val cleanSubject = subject?.toString()?.take(MAX_SHARED_TEXT_CHARS)?.trim().orEmpty()

            val files = JSArray()
            var fileCount = 0
            for (uri in uris) {
                if (fileCount >= MAX_SHARED_FILES) {
                    Log.w(TAG, "Share carried more than $MAX_SHARED_FILES files; dropping the rest")
                    break
                }
                // Only content:// and file:// are meaningful as a stream. A
                // provider is free to hand over anything, so filter rather than
                // trusting the scheme to be sane.
                val scheme = uri.scheme?.lowercase()
                if (scheme != "content" && scheme != "file") {
                    Log.w(TAG, "Ignoring shared stream with scheme: $scheme")
                    continue
                }
                files.put(JSObject().apply { put("token", mintToken(uri)) })
                fileCount += 1
            }

            if (cleanText.isEmpty() && cleanSubject.isEmpty() && fileCount == 0) return null

            return JSObject().apply {
                put("text", cleanText)
                put("subject", cleanSubject)
                put("files", files)
            }
        }

        /**
         * A display name from another app becomes an upload filename, so it
         * cannot be allowed to carry a path or control characters. Kept to the
         * last path segment, stripped of anything below U+0020, and bounded.
         */
        fun sanitizeFileName(raw: String?): String {
            val base = raw
                ?.substringAfterLast('/')
                ?.substringAfterLast('\\')
                ?.filter { it.code >= 0x20 && it.code != 0x7F }
                ?.trim()
                ?.trimStart('.')
                .orEmpty()
            if (base.isEmpty()) return FALLBACK_FILE_NAME
            return base.take(MAX_FILE_NAME_LEN)
        }
    }

    // Reads happen off the main thread: a 100 MiB content-provider read on the
    // UI thread is an ANR, and the provider can be an arbitrary other app that
    // is slow or wedged.
    private val reader = Executors.newSingleThreadExecutor { r ->
        Thread(r, "share-target-reader").apply { isDaemon = true }
    }

    override fun load(webView: WebView) {
        super.load(webView)
        instance = this
    }

    private fun emitShare(payload: JSObject) {
        trigger("share-received", payload)
    }

    /**
     * JS calls this once its listener is wired up. Replays a share that
     * arrived during cold start and flips the plugin into live-emit mode.
     */
    @Command
    fun jsReady(invoke: Invoke) {
        listenerReady = true
        pendingShare?.let { payload ->
            pendingShare = null
            emitShare(payload)
        }
        invoke.resolve()
    }

    /**
     * Reads one shared file by token and returns it as base64 plus the
     * metadata the frontend needs to rebuild a `File`.
     */
    @Command
    fun readSharedFile(invoke: Invoke) {
        val args = invoke.parseArgs(ReadSharedFileArgs::class.java)
        val uri = takeUri(args.token)
        if (uri == null) {
            // Distinguishable on purpose: "this token was never issued, or has
            // already been used" is a different failure from a read error, and
            // a single generic message for both is what makes this class of bug
            // undebuggable from the JS side.
            invoke.reject("share token is not valid (already used, or not from a share)")
            return
        }

        reader.execute {
            try {
                val resolver = activity.contentResolver

                var displayName: String? = null
                var declaredSize: Long = -1
                try {
                    resolver.query(uri, null, null, null, null)?.use { cursor ->
                        if (cursor.moveToFirst()) {
                            val nameIdx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                            if (nameIdx >= 0 && !cursor.isNull(nameIdx)) {
                                displayName = cursor.getString(nameIdx)
                            }
                            val sizeIdx = cursor.getColumnIndex(OpenableColumns.SIZE)
                            if (sizeIdx >= 0 && !cursor.isNull(sizeIdx)) {
                                declaredSize = cursor.getLong(sizeIdx)
                            }
                        }
                    }
                } catch (err: Throwable) {
                    // A provider that refuses to be queried can still be
                    // readable, so this is not fatal — we just lose the name.
                    Log.w(TAG, "Could not query shared file metadata", err)
                }

                // Reject on the declared size before reading a byte, when the
                // provider bothered to declare one.
                if (declaredSize > MAX_SHARED_FILE_BYTES) {
                    invoke.reject("shared file is larger than the ${MAX_SHARED_FILE_BYTES} byte limit")
                    return@execute
                }

                val mime = resolver.getType(uri) ?: "application/octet-stream"
                val name = sanitizeFileName(displayName ?: uri.lastPathSegment)

                val stream = resolver.openInputStream(uri)
                if (stream == null) {
                    invoke.reject("could not open the shared file")
                    return@execute
                }

                val bytes = stream.use { input ->
                    val buffer = ByteArray(64 * 1024)
                    val out = ByteArrayOutputStream()
                    var total = 0L
                    while (true) {
                        val read = input.read(buffer)
                        if (read <= 0) break
                        total += read
                        // The declared size is a claim, not a guarantee — a
                        // provider can under-report it or stream forever. This
                        // is the bound that actually holds.
                        if (total > MAX_SHARED_FILE_BYTES) {
                            return@execute invoke.reject(
                                "shared file is larger than the ${MAX_SHARED_FILE_BYTES} byte limit"
                            )
                        }
                        out.write(buffer, 0, read)
                    }
                    out.toByteArray()
                }

                val result = JSObject().apply {
                    put("name", name)
                    put("mime", mime)
                    put("base64", Base64.encodeToString(bytes, Base64.NO_WRAP))
                }
                invoke.resolve(result)
            } catch (err: Throwable) {
                Log.w(TAG, "Failed to read shared file", err)
                invoke.reject("could not read the shared file: ${err.javaClass.simpleName}")
            }
        }
    }
}
