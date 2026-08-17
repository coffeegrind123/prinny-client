package `in`.prinny.app

import android.Manifest
import android.app.Activity
import android.content.Context
import android.util.Log
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.Permission
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Plugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import org.unifiedpush.android.connector.UnifiedPush
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

val upScope = CoroutineScope(Dispatchers.Main + SupervisorJob())

@TauriPlugin(
    permissions = [
        Permission(
            strings = [Manifest.permission.POST_NOTIFICATIONS],
            alias = "notifications"
        )
    ]
)
class UnifiedPushPlugin(private val activity: Activity) : Plugin(activity) {

    companion object {
        const val TAG = "UnifiedPushPlugin"
        var instance: UnifiedPushPlugin? = null

        /**
         * Where the endpoint is kept between processes.
         *
         * The endpoint used to live only in the field below, which is a process
         * lifetime — and a push app's process is killed constantly. Every cold
         * start therefore found no endpoint, reported none to the frontend, and
         * drove a fresh distributor round-trip whose only possible outcome was
         * the endpoint it had just thrown away.
         *
         * It also cost the receiver its only way to notice ROTATION. The
         * distributor may hand out a new endpoint at any time, including while
         * the app is dead; the homeserver keeps pushing at the old pushkey until
         * something re-registers the pusher. Persisting it here means the next
         * launch can compare what the distributor last said against what the
         * homeserver is holding, and fix the difference.
         */
        private const val PREFS = "prinny_unifiedpush"
        private const val KEY_ENDPOINT = "endpoint"
    }

    private val prefs by lazy { activity.getSharedPreferences(PREFS, Context.MODE_PRIVATE) }

    private var savedEndpoint: String?
        get() = prefs.getString(KEY_ENDPOINT, null)
        set(value) {
            prefs.edit().apply {
                if (value == null) remove(KEY_ENDPOINT) else putString(KEY_ENDPOINT, value)
            }.apply()
        }

    override fun load(webView: WebView) {
        super.load(webView)
        instance = this
    }

    @Command
    fun register(invoke: Invoke) {
        upScope.launch {
            try {
                UnifiedPush.tryUseCurrentOrDefaultDistributor(activity) { success ->
                    if (!success) {
                        invoke.reject("No UnifiedPush distributor available. Install ntfy or NextPush.")
                        return@tryUseCurrentOrDefaultDistributor
                    }

                    instance = this@UnifiedPushPlugin
                    UnifiedPush.register(activity)

                    // The endpoint arrives asynchronously, on a broadcast to
                    // UnifiedPushReceiver -> onNewEndpoint.
                    //
                    // The stored one is NOT cleared first. Clearing it made a
                    // failed or slow re-registration destroy a perfectly good
                    // endpoint, leaving the app with nothing where it had
                    // something. A stored endpoint is answered immediately and
                    // the caller re-registers its pusher with it; if the
                    // distributor then issues a different one, `onNewEndpoint`
                    // fires `endpoint-received` and the caller registers again.
                    // Being briefly one rotation behind beats being empty.
                    upScope.launch {
                        var attempts = 0
                        while (savedEndpoint == null && attempts < 30) {
                            kotlinx.coroutines.delay(1000)
                            attempts++
                        }
                        val endpoint = savedEndpoint
                        if (endpoint != null) {
                            val result = JSObject()
                            result.put("endpoint", endpoint)
                            invoke.resolve(result)
                        } else {
                            invoke.reject(
                                "The distributor accepted the registration but never returned an endpoint " +
                                    "within 30s. Its own logs are the next place to look."
                            )
                        }
                    }
                }
            } catch (e: Exception) {
                invoke.reject("UnifiedPush registration error: ${e.message}", e)
            }
        }
    }

    @Command
    fun getEndpoint(invoke: Invoke) {
        val endpoint = savedEndpoint
        if (endpoint != null) {
            val result = JSObject()
            result.put("endpoint", endpoint)
            invoke.resolve(result)
        } else {
            invoke.reject("No UnifiedPush endpoint available")
        }
    }

    /**
     * Everything the device side knows about push, in one answer.
     *
     * Built for the diagnostics panel. Push has five links — a distributor is
     * installed, one is chosen, it issued an endpoint, the homeserver holds a
     * pusher for that endpoint, and Android will let us post — and until now a
     * user could see none of them. "Notifications don't work" was the entire
     * diagnosis available, for any of five different faults with five different
     * fixes, and the only trace of which was a console warning inside a WebView
     * on a phone.
     *
     * Deliberately never throws: a diagnostics call that fails tells the user
     * nothing, and the empty/absent values ARE the finding.
     */
    @Command
    fun getStatus(invoke: Invoke) {
        val result = JSObject()
        val distributors = try {
            UnifiedPush.getDistributors(activity)
        } catch (e: Exception) {
            Log.w(TAG, "Could not list distributors", e)
            emptyList()
        }
        val arr = org.json.JSONArray()
        distributors.forEach { arr.put(it) }
        result.put("distributors", arr)
        // Saved vs acked: saved is the one picked, acked is the one that has
        // actually answered with an endpoint. A saved-but-unacked distributor
        // is its own distinct fault — chosen, but never completed a handshake.
        result.put("savedDistributor", UnifiedPush.getSavedDistributor(activity) ?: "")
        result.put("ackDistributor", UnifiedPush.getAckDistributor(activity) ?: "")
        result.put("endpoint", savedEndpoint ?: "")
        result.put(
            "notificationsPermitted",
            getPermissionState("notifications").toString().lowercase() == "granted",
        )
        invoke.resolve(result)
    }

    @Command
    fun getDistributors(invoke: Invoke) {
        try {
            val distributors = UnifiedPush.getDistributors(activity)
            val result = JSObject()
            val arr = org.json.JSONArray()
            for (d in distributors) {
                arr.put(d)
            }
            result.put("distributors", arr.toString())
            invoke.resolve(result)
        } catch (e: Exception) {
            invoke.reject("Failed to get distributors: ${e.message}", e)
        }
    }

    @Command
    override fun requestPermissions(invoke: Invoke) {
        upScope.launch {
            requestPermissionForAlias("notifications", invoke, "requestPermissionsCallback")
        }
    }

    @app.tauri.annotation.PermissionCallback
    fun requestPermissionsCallback(invoke: Invoke) {
        val granted = getPermissionState("notifications").toString().lowercase() == "granted"
        val result = JSObject()
        result.put("granted", granted)
        invoke.resolve(result)
    }

    // Called by UnifiedPushReceiver
    fun onNewEndpoint(endpoint: String) {
        val rotated = savedEndpoint != null && savedEndpoint != endpoint
        savedEndpoint = endpoint
        // Logged because a rotation is the one push failure with no symptom at
        // all: everything keeps reporting healthy while the homeserver pushes
        // at an endpoint the distributor has already retired.
        Log.i(TAG, if (rotated) "Endpoint ROTATED — the pusher must be re-registered" else "Endpoint received")
        val data = JSObject()
        data.put("endpoint", endpoint)
        trigger("endpoint-received", data)
    }

    fun onRegistrationFailed(reason: String) {
        val data = JSObject()
        data.put("reason", reason)
        trigger("registration-failed", data)
    }

    fun onUnregistered() {
        savedEndpoint = null
        trigger("unregistered", JSObject())
    }

    fun onMessage(message: ByteArray) {
        val body = message.toString(Charsets.UTF_8)
        val data = JSObject()
        data.put("body", body)
        trigger("message-received", data)
    }
}
