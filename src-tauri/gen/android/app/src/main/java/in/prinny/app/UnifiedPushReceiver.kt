package `in`.prinny.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.graphics.BitmapFactory
import android.os.Build
import android.util.Log
import java.io.File
import java.security.MessageDigest
import org.json.JSONObject
import org.unifiedpush.android.connector.FailedReason
import org.unifiedpush.android.connector.MessagingReceiver
import org.unifiedpush.android.connector.UnifiedPush
import org.unifiedpush.android.connector.data.PushEndpoint
import org.unifiedpush.android.connector.data.PushMessage

class UnifiedPushReceiver : MessagingReceiver() {

    companion object {
        const val TAG = "UnifiedPushReceiver"

        /**
         * The SAME channel the in-app path posts on
         * (`MessageNotificationPlugin.CHANNEL_ID`).
         *
         * It used to be a second channel, `cinny_messages`, which gave Android
         * settings two entries both called "Messages" — so silencing the one
         * you could see left the other one ringing, and the sound/importance a
         * user chose applied to only half their notifications.
         */
        const val CHANNEL_ID = MessageNotificationPlugin.CHANNEL_ID
        const val NOTIFICATION_ID = 100
    }

    private fun createNotificationChannel(context: Context) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "Messages",
                NotificationManager.IMPORTANCE_HIGH
            ).apply {
                description = "Incoming Matrix messages"
                setShowBadge(true)
            }
            val manager = context.getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(channel)
        }
    }

    /**
     * What a Matrix push actually contains, once decoded.
     *
     * The pusher is registered WITHOUT `format: "event_id_only"` (see
     * useUnifiedPush.ts), so the homeserver sends the event itself:
     *
     *   {"notification":{"event_id":"$…","room_id":"!…","sender":"@a:b",
     *     "sender_display_name":"Alice","room_name":"Room",
     *     "content":{"msgtype":"m.text","body":"hello"},"counts":{"unread":1}}}
     *
     * Encrypted rooms are the interesting case: the server cannot decrypt them,
     * so `content` is absent and there is nothing to show but a generic line.
     * That is the correct outcome — it is the encryption working.
     */
    private data class PushSummary(
        val title: String,
        val text: String,
        val roomId: String?,
        val eventId: String?,
        // The raw mxid, kept separately from `title`: the title prefers a
        // display name and may fold in the room, neither of which identifies
        // anyone well enough to look an avatar up by.
        val senderId: String?,
        // From `counts.unread`. Absent when the server did not send one.
        val unreadCount: Int?,
    )

    private fun parsePush(raw: String): PushSummary {
        val fallback = PushSummary("Cinny", "New message", null, null, null, null)
        return try {
            val notification = JSONObject(raw).optJSONObject("notification") ?: return fallback
            val roomId = notification.optString("room_id", "").ifEmpty { null }
            val eventId = notification.optString("event_id", "").ifEmpty { null }
            val senderId = notification.optString("sender", "").ifEmpty { null }
            val sender = notification.optString("sender_display_name", "")
                .ifEmpty { senderId ?: "" }
            val roomName = notification.optString("room_name", "")

            // Title: prefer "Sender (Room)" so a group chat is distinguishable
            // from a direct message at a glance.
            val title = when {
                sender.isNotEmpty() && roomName.isNotEmpty() && roomName != sender -> "$sender ($roomName)"
                sender.isNotEmpty() -> sender
                roomName.isNotEmpty() -> roomName
                else -> "Cinny"
            }

            val content = notification.optJSONObject("content")
            val msgBody = content?.optString("body", "") ?: ""
            val text = when {
                msgBody.isNotEmpty() -> msgBody
                // No content: either an encrypted room, or an event type that
                // carries no body. Either way, say something useful.
                else -> "New message"
            }
            val unread = notification.optJSONObject("counts")?.let {
                if (it.has("unread")) it.optInt("unread", 0) else null
            }
            PushSummary(title, text.take(500), roomId, eventId, senderId, unread)
        } catch (e: Exception) {
            Log.w(TAG, "Unparseable push payload, falling back to a generic notification", e)
            fallback
        }
    }

    /**
     * The avatar the frontend cached for this key, if it cached one.
     *
     * The cache is written by `cache_notification_icon` while the app is
     * running (see `useNotificationAvatarCache.ts`) and named after a SHA-256
     * of an agreed key — `user:@alice:example.org` or `room:!abc:example.org`.
     * Recomputing that here is what lets a notification carry a face without
     * this receiver holding an access token, knowing the homeserver, or making
     * a single network request while handling a push.
     *
     * Both halves must agree on the key strings, the hash, and the truncation.
     * Rust takes the first 16 bytes of the digest and hex-encodes them.
     */
    private fun cachedAvatar(context: Context, key: String): File? = try {
        val digest = MessageDigest.getInstance("SHA-256").digest(key.toByteArray(Charsets.UTF_8))
        val hash = digest.take(16).joinToString("") { "%02x".format(it) }
        val dir = File(context.cacheDir, "notif-icons")
        // The extension is whatever the server served, so it is discovered
        // rather than assumed — the same list Rust sniffs into.
        listOf("png", "jpg", "jpeg", "gif", "webp", "bmp")
            .map { File(dir, "$hash.$it") }
            .firstOrNull { it.isFile }
    } catch (e: Exception) {
        Log.w(TAG, "Could not look up a cached avatar", e)
        null
    }

    /**
     * Show a system notification directly from the receiver.
     *
     * This is the path that runs when the app is not on screen, which is the
     * common case for a push — so it must not depend on any running JavaScript.
     */
    private fun showNotification(context: Context, body: String) {
        createNotificationChannel(context)

        val summary = parsePush(body)

        /**
         * A push with no `event_id` is not a message — it is the homeserver
         * saying the unread count changed, which is what it sends when you read
         * the room somewhere else. The Push Gateway API defines `event_id` as
         * optional for exactly this, and a zero count means nothing is left
         * unread.
         *
         * Handling it is what stops a phone buzzing about messages already read
         * on the desktop, and clears the ones sitting in the shade. Without this
         * branch the payload fell through to the ordinary path and posted "New
         * message" — so reading on another device *created* a notification
         * instead of dismissing one.
         */
        val notificationId = summary.roomId?.hashCode() ?: NOTIFICATION_ID
        if (summary.eventId == null || summary.unreadCount == 0) {
            Log.i(TAG, "Clearing notification (read elsewhere), unread=${summary.unreadCount}")
            context.getSystemService(NotificationManager::class.java)?.cancel(notificationId)
            return
        }

        // Carry the room/event through so tapping the notification lands in the
        // right room. MainActivity validates both before using them.
        val launchIntent = Intent(context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
            summary.roomId?.let { putExtra("roomId", it) }
            summary.eventId?.let { putExtra("eventId", it) }
        }

        val openIntent = PendingIntent.getActivity(
            context,
            // A per-room request code keeps rooms from overwriting each other's
            // pending intent; FLAG_UPDATE_CURRENT alone would share one.
            summary.roomId?.hashCode() ?: 0,
            launchIntent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

        // Sender first, room second. In a direct message the room's avatar is
        // the other person's anyway, but in a group the sender is who the
        // notification is actually about, and the room only says where.
        val avatar = summary.senderId?.let { cachedAvatar(context, "user:$it") }
            ?: summary.roomId?.let { cachedAvatar(context, "room:$it") }
        // Decoding can still fail on a truncated or corrupt file, and a missing
        // icon must never cost the notification itself.
        val avatarBitmap = avatar?.let {
            try {
                BitmapFactory.decodeFile(it.absolutePath)
            } catch (e: Throwable) {
                Log.w(TAG, "Could not decode cached avatar", e)
                null
            }
        }

        val notification = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(context, CHANNEL_ID)
                .setContentTitle(summary.title)
                .setContentText(summary.text)
                .setStyle(Notification.BigTextStyle().bigText(summary.text))
                .setSmallIcon(android.R.drawable.stat_notify_chat)
                .apply { avatarBitmap?.let { setLargeIcon(it) } }
                .setAutoCancel(true)
                .setContentIntent(openIntent)
                .build()
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(context)
                .setContentTitle(summary.title)
                .setContentText(summary.text)
                .setStyle(Notification.BigTextStyle().bigText(summary.text))
                .setSmallIcon(android.R.drawable.stat_notify_chat)
                .apply { avatarBitmap?.let { setLargeIcon(it) } }
                .setAutoCancel(true)
                .setContentIntent(openIntent)
                .build()
        }

        val manager = context.getSystemService(NotificationManager::class.java)
        // One notification per room instead of a single shared id, so a message
        // in one room no longer silently replaces an unread one from another —
        // and so the clearing branch above can cancel exactly one room's.
        manager.notify(notificationId, notification)
    }

    override fun onNewEndpoint(
        context: Context,
        endpoint: PushEndpoint,
        instance: String,
    ) {
        Log.i(TAG, "New endpoint received: ${endpoint.url}")
        UnifiedPushPlugin.instance?.onNewEndpoint(endpoint.url)
    }

    override fun onRegistrationFailed(
        context: Context,
        reason: FailedReason,
        instance: String,
    ) {
        Log.w(TAG, "Registration failed: $reason")
        UnifiedPushPlugin.instance?.onRegistrationFailed(reason.toString())
    }

    override fun onUnregistered(
        context: Context,
        instance: String,
    ) {
        Log.i(TAG, "Unregistered")
        UnifiedPushPlugin.instance?.onUnregistered()
    }

    override fun onMessage(
        context: Context,
        message: PushMessage,
        instance: String,
    ) {
        Log.i(TAG, "Push message received (${message.content.size} bytes, decrypted=${message.decrypted})")
        val body = message.content.toString(Charsets.UTF_8)

        // Route on whether the app is actually ON SCREEN, not on whether the
        // plugin object happens to exist.
        //
        // This used to test `UnifiedPushPlugin.instance != null`. That field is
        // process-static and is never cleared, so after the first launch it is
        // non-null for the life of the process — every push was handed to the
        // WebView, and the Kotlin branch below became unreachable. Android
        // suspends WebView JS once the activity leaves the screen, so a
        // backgrounded push reached JS that never ran and no notification was
        // ever posted. That is why notifications only worked with the app open.
        //
        // Foreground: let JS handle it, so the in-app timeline and the
        // notification stay consistent and we do not double-post.
        // Not foreground: post from Kotlin, which needs no running JS.
        val plugin = UnifiedPushPlugin.instance
        if (plugin != null && MainActivity.isAppInForeground) {
            plugin.onMessage(message.content)
        } else {
            showNotification(context, body)
            // Still nudge JS if the process happens to be alive, so the client
            // syncs and the room is up to date when the user opens it. Harmless
            // when JS is suspended — it simply never runs.
            plugin?.onMessage(message.content)
        }
    }
}
