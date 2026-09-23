package `in`.prinny.app

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.util.Log
import androidx.activity.result.ActivityResult
import androidx.core.content.FileProvider
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.InputStream
import java.util.concurrent.Executors

@InvokeArg
class EditCssArgs {
    lateinit var content: String
}

@InvokeArg
class ExportCssArgs {
    lateinit var content: String
    var fileName: String? = null
}

/**
 * Custom CSS "Edit in your text editor", Android half. The JS half is
 * `cinny/src/app/features/custom-css/externalEditor.ts`.
 *
 * `edit` writes the stylesheet to app-private storage and hands it to an
 * editor with ACTION_EDIT through the app's FileProvider (`custom_css` in
 * res/xml/file_paths.xml), granting read AND write on that one URI to the app
 * the user picks. When the user comes back, the file is read and returned; the
 * frontend diffs it against the defaults.
 *
 * Whether the edit lands depends on the editor: Acode, QuickEdit and Markor
 * save back into the shared URI, while some editors only view it or save a copy
 * elsewhere. `export_file` / `import_file` (the system document picker) are the
 * fallback for those and work with any file manager or editor.
 *
 * The only file this plugin ever writes is [cssFile]; exports go wherever the
 * user points the system picker, which is the user's own choice of location.
 */
@TauriPlugin
class CustomCssEditorPlugin(private val activity: Activity) : Plugin(activity) {
    companion object {
        private const val TAG = "CustomCssEditor"

        private const val DIR_NAME = "custom-css"
        private const val FILE_NAME = "prinny.css"
        private const val DEFAULT_EXPORT_NAME = "prinny.css"

        // Must match `MAX_FILE_BYTES` in externalEditor.ts; the real file is ~0.3 MB.
        private const val MAX_FILE_BYTES = 8L * 1024L * 1024L

        private const val MIME_CSS = "text/css"
        private const val MIME_TEXT = "text/plain"
    }

    // File I/O off the main thread; activity-result callbacks run on it.
    private val io = Executors.newSingleThreadExecutor { r ->
        Thread(r, "custom-css-io").apply { isDaemon = true }
    }

    private val cssFile: File
        get() = File(File(activity.filesDir, DIR_NAME).apply { mkdirs() }, FILE_NAME)

    private val fileUri: Uri
        get() = FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", cssFile)

    private fun readCapped(input: InputStream): String {
        val buffer = ByteArray(64 * 1024)
        val out = ByteArrayOutputStream()
        var total = 0L
        while (true) {
            val read = input.read(buffer)
            if (read <= 0) break
            total += read
            if (total > MAX_FILE_BYTES) {
                throw IllegalStateException("file is larger than the $MAX_FILE_BYTES byte limit")
            }
            out.write(buffer, 0, read)
        }
        return out.toString(Charsets.UTF_8.name())
    }

    private fun resolveContent(invoke: Invoke, content: String) {
        invoke.resolve(JSObject().apply { put("content", content) })
    }

    /**
     * Many editors register for text/plain but not text/css, and a chooser with
     * no targets is a dead end - so fall back to text/plain when nothing claims
     * CSS. Both are only type hints; the content is the same.
     */
    private fun editIntent(uri: Uri): Intent? {
        for (mime in listOf(MIME_CSS, MIME_TEXT)) {
            val intent = Intent(Intent.ACTION_EDIT).apply {
                setDataAndType(uri, mime)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
                // Grants travel with ClipData through the chooser.
                clipData = ClipData.newRawUri(FILE_NAME, uri)
            }
            val handlers = activity.packageManager.queryIntentActivities(intent, PackageManager.MATCH_DEFAULT_ONLY)
            if (handlers.isNotEmpty()) {
                return intent
            }
        }
        return null
    }

    @Command
    fun edit(invoke: Invoke) {
        val args = invoke.parseArgs(EditCssArgs::class.java)
        if (args.content.length.toLong() > MAX_FILE_BYTES) {
            invoke.reject("stylesheet is larger than the $MAX_FILE_BYTES byte limit")
            return
        }

        io.execute {
            try {
                cssFile.writeText(args.content, Charsets.UTF_8)
            } catch (err: Throwable) {
                Log.w(TAG, "Writing $FILE_NAME failed", err)
                invoke.reject("could not write the stylesheet: ${err.javaClass.simpleName}")
                return@execute
            }

            activity.runOnUiThread {
                val target = editIntent(fileUri)
                if (target == null) {
                    // A code the UI keys on to point at Export/Import instead.
                    invoke.reject("no installed app can edit text files", "NO_EDITOR")
                    return@runOnUiThread
                }
                val chooser = Intent.createChooser(target, "Edit Prinny CSS with").apply {
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
                    clipData = target.clipData
                }
                try {
                    startActivityForResult(invoke, chooser, "editReturned")
                } catch (err: ActivityNotFoundException) {
                    invoke.reject("no installed app can edit text files", "NO_EDITOR")
                }
            }
        }
    }

    /** Editors almost never set a result, so the code is ignored: the file is the result. */
    @ActivityCallback
    private fun editReturned(invoke: Invoke, result: ActivityResult) {
        activity.revokeUriPermission(
            fileUri,
            Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
        )
        readFile(invoke)
    }

    /** The current content of the shared file, e.g. after the app was killed while the editor was open. */
    @Command
    fun readFile(invoke: Invoke) {
        io.execute {
            try {
                val file = cssFile
                if (!file.exists()) {
                    invoke.reject("no stylesheet has been handed to an editor yet")
                    return@execute
                }
                resolveContent(invoke, file.inputStream().use { readCapped(it) })
            } catch (err: Throwable) {
                Log.w(TAG, "Reading $FILE_NAME failed", err)
                invoke.reject("could not read the stylesheet: ${err.message ?: err.javaClass.simpleName}")
            }
        }
    }

    @Command
    fun exportFile(invoke: Invoke) {
        val args = invoke.parseArgs(ExportCssArgs::class.java)
        val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = MIME_CSS
            putExtra(Intent.EXTRA_TITLE, args.fileName ?: DEFAULT_EXPORT_NAME)
        }
        activity.runOnUiThread { startActivityForResult(invoke, intent, "exportPicked") }
    }

    @ActivityCallback
    private fun exportPicked(invoke: Invoke, result: ActivityResult) {
        val uri = result.data?.data
        if (result.resultCode != Activity.RESULT_OK || uri == null) {
            invoke.reject("export cancelled", "CANCELLED")
            return
        }
        val args = invoke.parseArgs(ExportCssArgs::class.java)
        io.execute {
            try {
                // "wt": truncate, so exporting over a longer file leaves no tail.
                val stream = activity.contentResolver.openOutputStream(uri, "wt")
                if (stream == null) {
                    invoke.reject("could not open the chosen file for writing")
                    return@execute
                }
                stream.use { it.write(args.content.toByteArray(Charsets.UTF_8)) }
                invoke.resolve()
            } catch (err: Throwable) {
                Log.w(TAG, "Export failed", err)
                invoke.reject("could not write the chosen file: ${err.javaClass.simpleName}")
            }
        }
    }

    @Command
    fun importFile(invoke: Invoke) {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            // Providers label .css files inconsistently (text/css, text/plain,
            // application/octet-stream), so accept anything and let the parser judge.
            type = "*/*"
        }
        activity.runOnUiThread { startActivityForResult(invoke, intent, "importPicked") }
    }

    @ActivityCallback
    private fun importPicked(invoke: Invoke, result: ActivityResult) {
        val uri = result.data?.data
        if (result.resultCode != Activity.RESULT_OK || uri == null) {
            invoke.reject("import cancelled", "CANCELLED")
            return
        }
        io.execute {
            try {
                val stream = activity.contentResolver.openInputStream(uri)
                if (stream == null) {
                    invoke.reject("could not open the chosen file")
                    return@execute
                }
                resolveContent(invoke, stream.use { readCapped(it) })
            } catch (err: Throwable) {
                Log.w(TAG, "Import failed", err)
                invoke.reject("could not read the chosen file: ${err.message ?: err.javaClass.simpleName}")
            }
        }
    }
}
