package `in`.prinny.app

import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.graphics.BitmapFactory
import android.os.Build
import android.util.Log
import android.webkit.WebView
import androidx.core.app.NotificationCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File

/**
 * Direct notification path for Android — bypasses tauri-plugin-notification
 * because its `icon` field only resolves drawable resource names, not file
 * paths. We need file paths so per-message sender avatars (downloaded by
 * cache_notification_icon into the app cache dir) can be passed as a
 * Notification.Builder large icon.
 */
@InvokeArg
class ShowMessageNotificationArgs {
    lateinit var title: String
    lateinit var body: String
    var iconPath: String? = null
    var roomId: String? = null
    var eventId: String? = null
    var notificationId: Int? = null
}

@TauriPlugin
class MessageNotificationPlugin(private val activity: Activity) : Plugin(activity) {
    companion object {
        private const val TAG = "MessageNotification"
        const val CHANNEL_ID = "prinny_messages"
        private var nextId = 1000

        // Singleton handle so MainActivity.onNewIntent / onCreate can hand
        // back the roomId/eventId extras when the user taps a notification.
        // The plugin emits a JS-side event so the frontend can navigate to
        // the room — mirrors the 'notification://activated' Windows path.
        var instance: MessageNotificationPlugin? = null

        // Plugin.load() runs during MainActivity.onCreate, well before the
        // React app has mounted and called onNotificationAction(). Until JS
        // calls the jsReady command we stash clicks and replay the most
        // recent one when JS signals it's listening. After jsReady, click
        // arrivals (e.g. via onNewIntent while the app is warm) emit live.
        private var pendingClick: Pair<String, String>? = null
        private var listenerReady = false

        fun deliverClick(roomId: String, eventId: String) {
            if (listenerReady) {
                instance?.emitClick(roomId, eventId)
            } else {
                pendingClick = roomId to eventId
            }
        }
    }

    override fun load(webView: WebView) {
        super.load(webView)
        instance = this
    }

    private fun emitClick(roomId: String, eventId: String) {
        val data = JSObject()
        data.put("roomId", roomId)
        data.put("eventId", eventId)
        trigger("message-notification-clicked", data)
    }

    // JS calls this once after onNotificationAction's listener is wired up.
    // Replays the stashed cold-start click (if any) and flips the plugin
    // into live-emit mode so subsequent clicks fire trigger() immediately.
    @Command
    fun jsReady(invoke: Invoke) {
        listenerReady = true
        pendingClick?.let { (roomId, eventId) ->
            pendingClick = null
            emitClick(roomId, eventId)
        }
        invoke.resolve()
    }

    /**
     * Returns the icon file only if it canonically resolves inside the app's
     * own cache directory — the subtree the Rust `cache_notification_icon`
     * command writes to (app_cache_dir()/notif-icons). Canonicalising both
     * sides collapses `..` segments and symlinks, and the `File.separator`
     * suffix on the prefix is what stops a sibling like `/data/.../cache-evil`
     * from passing a naive `startsWith` on the cache path.
     */
    private fun resolveCachedIcon(path: String): File? = try {
        val cacheRoot = activity.cacheDir.canonicalFile
        val candidate = File(path).canonicalFile
        val prefix = cacheRoot.path + File.separator
        if (candidate.path.startsWith(prefix) && candidate.isFile) candidate else null
    } catch (_: Throwable) {
        // IOException/SecurityException from canonicalFile — treat as untrusted.
        null
    }

    private fun ensureChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val nm = activity.getSystemService(NotificationManager::class.java) ?: return
            if (nm.getNotificationChannel(CHANNEL_ID) == null) {
                val channel = NotificationChannel(
                    CHANNEL_ID,
                    "Messages",
                    NotificationManager.IMPORTANCE_HIGH,
                ).apply {
                    description = "New Matrix messages"
                }
                nm.createNotificationChannel(channel)
            }
        }
    }

    @Command
    fun show(invoke: Invoke) {
        val args = invoke.parseArgs(ShowMessageNotificationArgs::class.java)
        ensureChannel()

        val openIntent = Intent(activity, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            args.roomId?.let { putExtra("roomId", it) }
            args.eventId?.let { putExtra("eventId", it) }
        }
        val pendingIntent = PendingIntent.getActivity(
            activity,
            (args.notificationId ?: nextId),
            openIntent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

        val builder = NotificationCompat.Builder(activity, CHANNEL_ID)
            .setContentTitle(args.title)
            .setContentText(args.body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(args.body))
            .setSmallIcon(android.R.drawable.stat_notify_chat)
            .setAutoCancel(true)
            .setContentIntent(pendingIntent)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)

        args.iconPath?.let { path ->
            // `iconPath` arrives from webview JavaScript, so it is attacker-
            // influenced input being handed to a file read. Only paths the Rust
            // `cache_notification_icon` command produced (app cache dir) are
            // acceptable; anything else — /sdcard/…, a ../ traversal, another
            // app's exported file — is dropped. A rejected icon never fails the
            // notification itself; we just post it without a large icon.
            val safe = resolveCachedIcon(path)
            if (safe == null) {
                Log.w(TAG, "Ignoring notification iconPath outside the app cache dir")
            } else {
                try {
                    BitmapFactory.decodeFile(safe.absolutePath)?.let { bitmap ->
                        builder.setLargeIcon(bitmap)
                    }
                } catch (_: Throwable) {
                }
            }
        }

        val notificationId = args.notificationId ?: nextId++
        val nm = activity.getSystemService(NotificationManager::class.java)
        nm?.notify(notificationId, builder.build())

        invoke.resolve()
    }
}
