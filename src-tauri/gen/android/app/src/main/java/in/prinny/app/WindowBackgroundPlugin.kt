package `in`.prinny.app

import android.app.Activity
import android.graphics.Color
import android.graphics.drawable.ColorDrawable
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

@InvokeArg
class WindowBackgroundArgs {
    lateinit var color: String
}

/**
 * Paints the Activity window in the app's theme background. The JS half is
 * `cinny/src/app/hooks/useAndroidWindowBackground.ts`.
 *
 * While the IME opens or closes, Android resizes the WebView a frame or more
 * after the window, and the window background shows in the gap. Left at the
 * DayNight theme default it is white or black by the *system* night mode, so
 * an in-app theme unlike the system one flashed a wrong-colour band above the
 * keyboard.
 */
@TauriPlugin
class WindowBackgroundPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun setColor(invoke: Invoke) {
        val args = invoke.parseArgs(WindowBackgroundArgs::class.java)
        val color = try {
            Color.parseColor(args.color)
        } catch (err: IllegalArgumentException) {
            invoke.reject("not a colour: ${args.color}")
            return
        }

        activity.runOnUiThread {
            activity.window.setBackgroundDrawable(ColorDrawable(color))
            invoke.resolve()
        }
    }
}
