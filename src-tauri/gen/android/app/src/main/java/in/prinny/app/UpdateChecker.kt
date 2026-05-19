package `in`.prinny.app

import android.app.DownloadManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.SharedPreferences
import android.database.Cursor
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.util.Log
import androidx.core.content.FileProvider
import `in`.prinny.app.BuildConfig
import org.json.JSONObject
import java.io.BufferedReader
import java.io.File
import java.io.InputStreamReader
import java.net.HttpURLConnection
import java.net.URL

/**
 * Polls release.json on app start, decides whether to download a newer
 * APK, and prompts the user to install it.
 *
 * Three behavioural guarantees:
 *   1. **No re-downloads.** Once an APK for a given version is on disk
 *      we never enqueue it again — even after a force-close. The version
 *      is checked against (a) `BuildConfig.VERSION_NAME` of the running
 *      build, (b) the in-flight DownloadManager queue, and (c) a previously
 *      stored APK file. Only (a) matters for "user already installed it"
 *      but (b) and (c) cover the install-pending window.
 *   2. **Install prompt fires on completion.** A BroadcastReceiver on
 *      `ACTION_DOWNLOAD_COMPLETE` opens the APK with a FileProvider
 *      content:// URI. DownloadManager's built-in tap-the-notification
 *      handler throws FileUriExposedException on Android 7+ with the
 *      file:// it generates, so the user sees "Can't open file" — we
 *      bypass that by launching the install intent ourselves.
 *   3. **Idempotent install.** If the user backgrounds the install dialog
 *      and re-launches the app, `check()` sees the APK already on disk
 *      and re-prompts instead of starting a new download.
 *
 * If `packageManager.canRequestPackageInstalls()` is false (Android 8+
 * per-app "install unknown apps" toggle off), the install intent is
 * still dispatched — Android shows the toggle-and-retry dialog itself.
 */
class UpdateChecker(private val context: Context) {

    companion object {
        private const val TAG = "UpdateChecker"
        private const val RELEASE_JSON_URL =
            "https://github.com/coffeegrind123/prinny-client/releases/download/tauri/release.json"
        private const val PREFS = "prinny_updater"
        private const val KEY_LAST_DL_ID = "last_download_id"
        private const val KEY_LAST_DL_VERSION = "last_download_version"

        // Process-lifetime guard: we re-prompt at most once per cold start
        // when an already-downloaded APK is sitting on disk. Without this
        // the user would face the install dialog on every launch they
        // dismissed previously — too aggressive. The freshly-completed
        // download path (`registerCompletionReceiver`) ignores this flag
        // because that's the one moment when the install prompt feels
        // expected. A static is fine here — the field resets when the
        // process is killed, which is the same boundary as "cold start."
        @Volatile
        private var promptedThisSession = false
    }

    private val prefs: SharedPreferences
        get() = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun check() {
        Thread {
            try {
                val json = fetchReleaseJson() ?: return@Thread
                // Android lives at the top-level `android` key (not under `platforms`,
                // which the Tauri updater deserializes as { signature, url } per entry —
                // putting android there fails with "missing field signature").
                // Fall back to platforms.android for older release.json versions.
                val android = json.optJSONObject("android")
                    ?: json.optJSONObject("platforms")?.optJSONObject("android")
                    ?: return@Thread

                val latestVersion = android.optString("version", "").removePrefix("v")
                val apkUrl = android.optString("url", "")
                val sha256 = android.optString("sha256", "")

                if (latestVersion.isEmpty() || apkUrl.isEmpty()) {
                    Log.w(TAG, "release.json missing android version or URL")
                    return@Thread
                }

                val currentVersion = BuildConfig.VERSION_NAME.removePrefix("v")

                if (!isNewer(latestVersion, currentVersion)) {
                    Log.d(TAG, "Already up to date (v$currentVersion)")
                    // Clean up any stale APK from a previous version cycle —
                    // the user installed it, we no longer need it on disk.
                    cleanupStaleApks(latestVersion)
                    return@Thread
                }

                Log.i(TAG, "New version available: v$latestVersion (current: v$currentVersion)")

                // Already downloaded? Prompt install instead of redownload —
                // but only once per cold start so the user isn't pestered
                // every launch after they've dismissed the dialog.
                val existing = existingApk(latestVersion)
                if (existing != null && existing.exists() && existing.length() > 0) {
                    if (promptedThisSession) {
                        Log.d(TAG, "APK already on disk and we've already prompted this session — skipping")
                        return@Thread
                    }
                    Log.i(TAG, "APK already on disk: ${existing.absolutePath}, prompting install")
                    promptedThisSession = true
                    promptInstall(existing)
                    return@Thread
                }

                // Download in flight? Skip enqueueing a duplicate.
                if (isDownloadInFlight(latestVersion)) {
                    Log.i(TAG, "Download already in flight for v$latestVersion")
                    return@Thread
                }

                downloadApk(apkUrl, latestVersion, sha256)
            } catch (e: Exception) {
                Log.w(TAG, "Update check failed", e)
            }
        }.start()
    }

    private fun fetchReleaseJson(): JSONObject? {
        var connection: HttpURLConnection? = null
        try {
            val url = URL(RELEASE_JSON_URL)
            connection = url.openConnection() as HttpURLConnection
            connection.connectTimeout = 15_000
            connection.readTimeout = 15_000
            connection.instanceFollowRedirects = true

            if (connection.responseCode != 200) {
                Log.w(TAG, "HTTP ${connection.responseCode} fetching release.json")
                return null
            }

            val reader = BufferedReader(InputStreamReader(connection.inputStream))
            val body = reader.use { it.readText() }
            return JSONObject(body)
        } finally {
            connection?.disconnect()
        }
    }

    private fun isNewer(latest: String, current: String): Boolean {
        val latestParts = latest.split(".").map { it.toIntOrNull() ?: 0 }
        val currentParts = current.split(".").map { it.toIntOrNull() ?: 0 }
        val maxLen = maxOf(latestParts.size, currentParts.size)

        for (i in 0 until maxLen) {
            val l = latestParts.getOrElse(i) { 0 }
            val c = currentParts.getOrElse(i) { 0 }
            if (l > c) return true
            if (l < c) return false
        }
        return false
    }

    private fun apkFile(version: String): File =
        File(
            Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS),
            "cinny-v$version.apk"
        )

    private fun existingApk(version: String): File? {
        val file = apkFile(version)
        return if (file.exists() && file.length() > 0) file else null
    }

    private fun cleanupStaleApks(currentLatestVersion: String) {
        // Only delete the version we previously downloaded — leave any
        // user-placed APKs alone. Conservative: if SharedPrefs disagrees,
        // do nothing.
        val prevVersion = prefs.getString(KEY_LAST_DL_VERSION, null) ?: return
        if (prevVersion == currentLatestVersion) return
        val stale = apkFile(prevVersion)
        if (stale.exists()) {
            if (stale.delete()) {
                Log.i(TAG, "Cleaned up stale APK: ${stale.absolutePath}")
                prefs.edit().remove(KEY_LAST_DL_VERSION).remove(KEY_LAST_DL_ID).apply()
            }
        }
    }

    /**
     * Returns true if DownloadManager has an entry for this version that
     * is queued, running, or paused. Prevents enqueueing a duplicate when
     * the user re-opens the app mid-download.
     */
    private fun isDownloadInFlight(version: String): Boolean {
        val savedId = prefs.getLong(KEY_LAST_DL_ID, -1L)
        val savedVersion = prefs.getString(KEY_LAST_DL_VERSION, null)
        if (savedId < 0 || savedVersion != version) return false

        val dm = context.getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager
        val query = DownloadManager.Query().setFilterById(savedId)
        var cursor: Cursor? = null
        try {
            cursor = dm.query(query)
            if (cursor != null && cursor.moveToFirst()) {
                val statusIdx = cursor.getColumnIndex(DownloadManager.COLUMN_STATUS)
                if (statusIdx < 0) return false
                val status = cursor.getInt(statusIdx)
                return status == DownloadManager.STATUS_PENDING ||
                    status == DownloadManager.STATUS_RUNNING ||
                    status == DownloadManager.STATUS_PAUSED
            }
        } finally {
            cursor?.close()
        }
        return false
    }

    private fun downloadApk(url: String, version: String, sha256: String) {
        val file = apkFile(version)
        // Stale partial leftover from a killed download — clear it so the
        // new enqueue lands on a clean path.
        if (file.exists() && file.length() == 0L) file.delete()

        val request = DownloadManager.Request(Uri.parse(url))
            .setTitle("Cinny v$version")
            .setDescription("Downloading update...")
            .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED)
            .setDestinationInExternalPublicDir(Environment.DIRECTORY_DOWNLOADS, "cinny-v$version.apk")
            .setAllowedOverMetered(true)
            .setAllowedOverRoaming(true)
            // MIME type so the system associates the file with the package
            // installer once it lands on disk.
            .setMimeType("application/vnd.android.package-archive")

        val dm = context.getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager
        val id = dm.enqueue(request)
        prefs.edit()
            .putLong(KEY_LAST_DL_ID, id)
            .putString(KEY_LAST_DL_VERSION, version)
            .apply()
        Log.i(TAG, "Download queued: cinny-v$version.apk (id=$id)")

        registerCompletionReceiver(id, version)
    }

    private fun registerCompletionReceiver(downloadId: Long, version: String) {
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(ctx: Context, intent: Intent) {
                val received =
                    intent.getLongExtra(DownloadManager.EXTRA_DOWNLOAD_ID, -1L)
                if (received != downloadId) return
                try {
                    ctx.unregisterReceiver(this)
                } catch (_: Exception) {
                }

                val dm = ctx.getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager
                val q = DownloadManager.Query().setFilterById(downloadId)
                var cursor: Cursor? = null
                try {
                    cursor = dm.query(q)
                    if (cursor == null || !cursor.moveToFirst()) return
                    val statusIdx = cursor.getColumnIndex(DownloadManager.COLUMN_STATUS)
                    val status = if (statusIdx >= 0) cursor.getInt(statusIdx) else -1
                    if (status != DownloadManager.STATUS_SUCCESSFUL) {
                        Log.w(TAG, "Download finished with status=$status")
                        return
                    }
                    val file = apkFile(version)
                    if (!file.exists() || file.length() == 0L) {
                        Log.w(TAG, "Download reported success but file missing: $file")
                        return
                    }
                    // Fresh-download path — always prompt. Also mark the
                    // session as prompted so the launch-time path doesn't
                    // re-prompt if the user backgrounds the dialog and
                    // the activity comes back to the foreground.
                    promptedThisSession = true
                    promptInstall(file)
                } finally {
                    cursor?.close()
                }
            }
        }

        val filter = IntentFilter(DownloadManager.ACTION_DOWNLOAD_COMPLETE)
        // Android 14 (API 34) requires explicit export flags on dynamic
        // receivers. ACTION_DOWNLOAD_COMPLETE is sent by the system, so
        // we need RECEIVER_EXPORTED. The flag constant exists from Tiramisu
        // (API 33) onwards; older targets ignore it.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(receiver, filter, Context.RECEIVER_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            context.registerReceiver(receiver, filter)
        }
    }

    private fun promptInstall(apk: File) {
        val authority = "${context.packageName}.fileprovider"
        val uri: Uri = try {
            FileProvider.getUriForFile(context, authority, apk)
        } catch (e: IllegalArgumentException) {
            Log.e(TAG, "FileProvider can't expose ${apk.absolutePath} via $authority — check file_paths.xml", e)
            return
        }

        val intent = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, "application/vnd.android.package-archive")
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }

        try {
            context.startActivity(intent)
            Log.i(TAG, "Install prompt launched for ${apk.name}")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to launch install intent", e)
        }
    }
}
