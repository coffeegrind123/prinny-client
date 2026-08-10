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
import java.io.FileInputStream
import java.io.InputStreamReader
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

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

        // The digest release.json advertised for the APK we staged. Persisted
        // next to the version so the launch-time "APK already on disk" path can
        // verify the file too — without it, a re-launch would install whatever
        // bytes are sitting at the staging path.
        private const val KEY_LAST_DL_SHA256 = "last_download_sha256"

        // The only hosts we will hand to DownloadManager. release.json is
        // fetched over the network, so a tampered/served-wrong release.json
        // must not be able to redirect the updater at an arbitrary APK. GitHub
        // serves release assets from github.com and redirects to
        // objects.githubusercontent.com.
        private val ALLOWED_APK_HOSTS = setOf("github.com", "objects.githubusercontent.com")

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
                    // Never install a staged file on trust. Prefer the digest we
                    // just fetched; fall back to the one recorded when we staged
                    // the file. No digest at all = refuse (verifyApk deletes it),
                    // so the next check re-downloads from a verified source.
                    val expected = sha256.ifEmpty { storedSha256(latestVersion) }
                    if (!verifyApk(existing, expected)) {
                        Log.e(TAG, "Staged APK for v$latestVersion failed SHA-256 verification — not prompting")
                        forgetStagedDownload()
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

    private fun apkName(version: String): String = "cinny-v$version.apk"

    /**
     * Staging directory for the update APK: app-private external storage
     * (`Android/data/<pkg>/files/Download`), NOT the shared public Downloads
     * folder. The public folder is writable by any app holding storage access,
     * and the filename is entirely predictable (`cinny-v<version>.apk`), so
     * another app could plant or swap the APK between download and install.
     * The app-private dir is not reachable by other apps' file access.
     */
    private fun apkDir(): File? = context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS)

    private fun apkFile(version: String): File? {
        val dir = apkDir() ?: return null
        return File(dir, apkName(version))
    }

    private fun existingApk(version: String): File? {
        val file = apkFile(version) ?: return null
        return if (file.exists() && file.length() > 0) file else null
    }

    private fun cleanupStaleApks(currentLatestVersion: String) {
        // Only delete the version we previously downloaded — leave any
        // user-placed APKs alone. Conservative: if SharedPrefs disagrees,
        // do nothing.
        val prevVersion = prefs.getString(KEY_LAST_DL_VERSION, null) ?: return
        if (prevVersion == currentLatestVersion) return
        val stale = apkFile(prevVersion) ?: return
        if (stale.exists()) {
            if (stale.delete()) {
                Log.i(TAG, "Cleaned up stale APK: ${stale.absolutePath}")
                forgetStagedDownload()
            }
        }
    }

    private fun storedSha256(version: String): String {
        // The digest is only meaningful for the version we actually staged.
        if (prefs.getString(KEY_LAST_DL_VERSION, null) != version) return ""
        return prefs.getString(KEY_LAST_DL_SHA256, "") ?: ""
    }

    private fun forgetStagedDownload() {
        prefs.edit()
            .remove(KEY_LAST_DL_VERSION)
            .remove(KEY_LAST_DL_ID)
            .remove(KEY_LAST_DL_SHA256)
            .apply()
    }

    /**
     * Integrity gate for every install path. release.json publishes a `sha256`
     * for the Android APK; until this existed it was parsed and thrown away, so
     * the updater would install whatever bytes were at the staging path —
     * a truncated download, a MITM'd body, or a file planted by another app.
     *
     * Compares case-insensitively (release.json digests are lowercase hex, but
     * hand-edited ones may not be). On ANY failure — no digest available,
     * unreadable file, mismatch — the file is deleted and we return false so
     * the caller does not prompt; the next check re-downloads it.
     */
    private fun verifyApk(file: File, expectedSha256: String?): Boolean {
        val expected = expectedSha256?.trim().orEmpty()
        if (expected.isEmpty()) {
            Log.e(TAG, "No expected SHA-256 available for ${file.name} — refusing to install")
            file.delete()
            return false
        }

        val actual = try {
            sha256Of(file)
        } catch (e: Exception) {
            Log.e(TAG, "Failed to hash ${file.absolutePath} — refusing to install", e)
            file.delete()
            return false
        }

        if (!actual.equals(expected, ignoreCase = true)) {
            Log.e(
                TAG,
                "SHA-256 mismatch for ${file.name}: expected=$expected actual=$actual — deleting"
            )
            file.delete()
            return false
        }

        Log.i(TAG, "SHA-256 verified for ${file.name}")
        return true
    }

    private fun sha256Of(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        FileInputStream(file).use { input ->
            val buffer = ByteArray(8192)
            while (true) {
                val read = input.read(buffer)
                if (read < 0) break
                if (read > 0) digest.update(buffer, 0, read)
            }
        }
        val out = StringBuilder(64)
        for (b in digest.digest()) {
            val v = b.toInt() and 0xFF
            if (v < 0x10) out.append('0')
            out.append(Integer.toHexString(v))
        }
        return out.toString()
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
        // Validate the URL before it reaches DownloadManager. Without this,
        // whatever `url` release.json carries is fetched verbatim — plain http
        // (swappable in flight) or an attacker-controlled host both worked.
        val parsed = Uri.parse(url)
        val host = parsed.host?.lowercase() ?: ""
        if (parsed.scheme != "https" || host !in ALLOWED_APK_HOSTS) {
            Log.e(TAG, "Refusing update download from untrusted URL: $url")
            return
        }

        // Without a published digest we could never verify the result, so the
        // download would be staged only to be deleted at install time. Bail early.
        if (sha256.isBlank()) {
            Log.e(TAG, "release.json has no sha256 for v$version — refusing to download")
            return
        }

        val file = apkFile(version)
        if (file == null) {
            Log.w(TAG, "App-private external storage unavailable — skipping update download")
            return
        }
        // Stale partial leftover from a killed download — clear it so the
        // new enqueue lands on a clean path.
        if (file.exists() && file.length() == 0L) file.delete()

        val request = DownloadManager.Request(parsed)
            .setTitle("Cinny v$version")
            .setDescription("Downloading update...")
            .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED)
            // App-private external files dir — see apkDir(). The public
            // Downloads folder let any storage-capable app swap the APK.
            .setDestinationInExternalFilesDir(
                context,
                Environment.DIRECTORY_DOWNLOADS,
                apkName(version)
            )
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
            // Persist the expected digest so a cold start can verify the staged
            // file even though release.json isn't re-read at that point.
            .putString(KEY_LAST_DL_SHA256, sha256)
            .apply()
        Log.i(TAG, "Download queued: ${apkName(version)} (id=$id)")

        registerCompletionReceiver(id, version, sha256)
    }

    private fun registerCompletionReceiver(downloadId: Long, version: String, sha256: String) {
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
                    if (file == null || !file.exists() || file.length() == 0L) {
                        Log.w(TAG, "Download reported success but file missing: $file")
                        return
                    }
                    // Integrity gate: a successful DownloadManager status only
                    // means bytes arrived, not that they're the bytes the
                    // release published. Verify before handing the file to the
                    // package installer.
                    if (!verifyApk(file, sha256)) {
                        Log.e(TAG, "Downloaded APK for v$version failed SHA-256 verification — not installing")
                        forgetStagedDownload()
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
        // receivers. NOT_EXPORTED: ACTION_DOWNLOAD_COMPLETE is delivered by the
        // system directly to the app that enqueued the download, so the
        // receiver never needs to accept broadcasts from other apps. Exported,
        // any app could fire the action with a guessed EXTRA_DOWNLOAD_ID and
        // drive us into the install path. The download-id equality check above
        // stays as a second gate. The flag constant exists from Tiramisu
        // (API 33) onwards; older targets ignore it.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            context.registerReceiver(receiver, filter)
        }
    }

    /**
     * Callers MUST have run [verifyApk] on `apk` first — this hands the file to
     * the package installer, so an unverified file installs unverified code.
     * The FileProvider only exposes the app-private staging dir (see
     * res/xml/file_paths.xml `external-files-path`).
     */
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
