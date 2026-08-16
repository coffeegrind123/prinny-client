package `in`.prinny.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log
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
    private data class PushSummary(val title: String, val text: String, val roomId: String?, val eventId: String?)

    private fun parsePush(raw: String): PushSummary {
        val fallback = PushSummary("Cinny", "New message", null, null)
        return try {
            val notification = JSONObject(raw).optJSONObject("notification") ?: return fallback
            val roomId = notification.optString("room_id", "").ifEmpty { null }
            val eventId = notification.optString("event_id", "").ifEmpty { null }
            val sender = notification.optString("sender_display_name", "")
                .ifEmpty { notification.optString("sender", "") }
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
            PushSummary(title, text.take(500), roomId, eventId)
        } catch (e: Exception) {
            Log.w(TAG, "Unparseable push payload, falling back to a generic notification", e)
            fallback
        }
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

        val notification = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(context, CHANNEL_ID)
                .setContentTitle(summary.title)
                .setContentText(summary.text)
                .setStyle(Notification.BigTextStyle().bigText(summary.text))
                .setSmallIcon(android.R.drawable.stat_notify_chat)
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
                .setAutoCancel(true)
                .setContentIntent(openIntent)
                .build()
        }

        val manager = context.getSystemService(NotificationManager::class.java)
        // One notification per room instead of a single shared id, so a message
        // in one room no longer silently replaces an unread one from another.
        manager.notify(summary.roomId?.hashCode() ?: NOTIFICATION_ID, notification)
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
