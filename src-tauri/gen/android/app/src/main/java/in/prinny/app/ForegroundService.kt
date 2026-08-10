package `in`.prinny.app

import android.app.AlarmManager
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build
import android.os.IBinder
import android.os.SystemClock
import androidx.core.content.ContextCompat

/**
 * Foreground service that keeps the Matrix WebSocket alive in the background.
 * Required on de-Googled devices (GrapheneOS) without FCM.
 *
 * - IMPORTANCE_LOW channel: visible status bar icon, no sound/vibration
 * - Network change detection: restarts on WiFi ↔ mobile switch
 * - AlarmManager watchdog: wakes every 15 min to survive doze kills
 */
class ForegroundService : Service() {

    companion object {
        const val CHANNEL_ID = "prinny_foreground"
        const val NOTIFICATION_ID = 1
        const val PI_WATCHDOG = 10
        const val WATCHDOG_INTERVAL_MS = 15 * 60 * 1000L // 15 minutes
        var isRunning = false
            private set

        // When `true`, startForeground() also includes the MICROPHONE
        // service type so Android 14+ will let the WebView keep the mic
        // open during Element Call when the app is backgrounded. Off by
        // default so we don't permanently advertise mic usage to users
        // who never call.
        @Volatile
        var microphoneActive: Boolean = false

        // Page JS (CallEmbedProvider) flips this on when a call embed mounts —
        // which happens BEFORE the widget asks for capture. It is only a
        // request: it never enables the microphone service type by itself.
        // ForegroundServicePlugin applies it if capture is already authorized,
        // otherwise MainActivity applies it at the moment it grants audio
        // capture to an allowed origin. If no grant ever happens, the mic
        // service type is never advertised.
        @Volatile
        var microphoneRequested: Boolean = false

        fun startWithForeground(service: ForegroundService) {
            try {
                val typeBitmask = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                    var mask = ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
                    if (microphoneActive) {
                        mask = mask or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
                    }
                    mask
                } else 0

                service.startForeground(
                    NOTIFICATION_ID,
                    service.buildNotification(),
                    typeBitmask
                )
            } catch (_: Exception) {}
        }

        /**
         * Toggle whether the foreground service advertises microphone use.
         * Called from JS when an Element Call is joined / left so the mic
         * stays accessible when the app is backgrounded mid-call.
         *
         * SECURITY: enabling this is what allows capture to continue while
         * backgrounded, so it is NOT open to arbitrary page JS. The only
         * caller, ForegroundServicePlugin.setMicrophoneActive, refuses
         * `active = true` unless MainActivity actually granted audio capture
         * to an allowed origin and RECORD_AUDIO is still held. Any new caller
         * must apply the same gate; `active = false` is always safe.
         */
        fun setMicrophoneActive(context: Context, active: Boolean) {
            microphoneActive = active
            // Restart the foreground state to apply the new type bitmask.
            val intent = Intent(context, ForegroundService::class.java)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
        }

        /**
         * Periodic alarm that restarts the service if Android killed it during
         * doze or due to memory pressure.
         */
        fun scheduleWatchdog(context: Context) {
            val am = context.getSystemService(AlarmManager::class.java) ?: return
            val intent = Intent(context, ForegroundService::class.java)
            val pi = PendingIntent.getService(
                context, PI_WATCHDOG, intent,
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
            )

            val trigger = SystemClock.elapsedRealtime() + WATCHDOG_INTERVAL_MS
            try {
                am.setExactAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, trigger, pi)
            } catch (_: Exception) {}
        }
    }

    private var connectivityCallback: ConnectivityManager.NetworkCallback? = null
    private var connectionChangedReceiver: ConnectionChangedReceiver? = null

    // Network callback — wakes the service when internet becomes available
    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            startWithForeground(this@ForegroundService)
        }

        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
            if (caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)) {
                startWithForeground(this@ForegroundService)
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        startWithForeground(this)

        // Listen for network changes
        val cm = getSystemService(ConnectivityManager::class.java)
        val builder = NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
        connectivityCallback = networkCallback
        cm.registerNetworkCallback(builder.build(), networkCallback)

        // Broadcast receiver for connectivity/airplane mode changes
        connectionChangedReceiver = ConnectionChangedReceiver()
        val iif = IntentFilter().apply {
            addAction(ConnectivityManager.CONNECTIVITY_ACTION)
            addAction(Intent.ACTION_AIRPLANE_MODE_CHANGED)
        }
        ContextCompat.registerReceiver(
            this, connectionChangedReceiver, iif,
            ContextCompat.RECEIVER_NOT_EXPORTED
        )

        // Schedule watchdog alarm
        scheduleWatchdog(this)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        startWithForeground(this)
        isRunning = true
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        isRunning = false

        connectivityCallback?.let {
            val cm = getSystemService(ConnectivityManager::class.java)
            cm.unregisterNetworkCallback(it)
        }
        connectionChangedReceiver?.let {
            try { unregisterReceiver(it) } catch (_: Exception) {}
        }

        // Reschedule watchdog so we come back after doze kills
        scheduleWatchdog(this)

        super.onDestroy()
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "Background service",
                NotificationManager.IMPORTANCE_LOW // visible icon, no sound — appropriate for IM
            ).apply {
                description = "Shown when Cinny keeps the Matrix connection alive. " +
                    "Disable this channel in system settings to hide the notification."
                setShowBadge(false)
            }
            val manager = getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(channel)
        }
    }

    private fun buildNotification(): Notification {
        createNotificationChannel()

        val openIntent = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val n = Notification.Builder(this, CHANNEL_ID)
                .setContentTitle("Cinny")
                .setContentText("Connected to Matrix")
                .setSmallIcon(android.R.drawable.ic_dialog_info)
                .setOngoing(true)
                .setContentIntent(openIntent)
                .setShowWhen(false)
                .setCategory(Notification.CATEGORY_SERVICE)
                .build()
            n.flags = n.flags or Notification.FLAG_NO_CLEAR
            return n
        } else {
            @Suppress("DEPRECATION")
            val n = Notification.Builder(this)
                .setContentTitle("Cinny")
                .setContentText("Connected to Matrix")
                .setSmallIcon(android.R.drawable.ic_dialog_info)
                .setOngoing(true)
                .setContentIntent(openIntent)
                .setPriority(Notification.PRIORITY_MIN)
                .build()
            n.flags = n.flags or Notification.FLAG_NO_CLEAR
            return n
        }
    }

    /**
     * Broadcast receiver that restarts the foreground notification when
     * connectivity or airplane mode changes.
     */
    inner class ConnectionChangedReceiver : android.content.BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            startWithForeground(this@ForegroundService)
        }
    }

}
