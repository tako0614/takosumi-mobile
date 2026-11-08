package com.takosumi.appscreen

import android.content.Intent
import android.net.Uri
import android.webkit.ValueCallback

/** One native picker result belongs to one document, even after cancellation.
 * UI-thread only. A picker may stop the Activity normally; do not cancel for
 * pause/stop. Navigation, eviction and destruction cancel the WebView callback
 * but retain an empty slot until that picker's result has been consumed. */
internal class AppScreenFileSelection(
    private val current: () -> Context?,
    private val interactive: () -> Boolean,
    private val launch: (Intent) -> Unit,
) {
    data class Context(
        val view: Any,
        val profile: String,
        val generation: Long,
        val document: Long,
        val page: String,
    )

    private class Pending(val context: Context?, var callback: ValueCallback<Array<Uri>>?)
    private var pending: Pending? = null
    val awaitingResult: Boolean get() = pending != null

    /** The previous Activity's callback is gone, but its picker may return to
     * the restored ActivityResultRegistry. Do not give that result a new owner. */
    fun restoreAwaitingResult(awaiting: Boolean) {
        if (awaiting && pending == null) pending = Pending(null, null)
    }

    fun begin(owner: Any, callback: ValueCallback<Array<Uri>>, intent: () -> Intent) {
        if (pending?.callback === callback) return
        val context = current()
        if (pending != null || !interactive() || context == null || context.view !== owner) {
            callback.onReceiveValue(null)
            return
        }
        val item = Pending(context, callback)
        pending = item
        try {
            launch(intent())
        } catch (_: RuntimeException) {
            if (pending === item) {
                pending = null
                finish(item, null)
            }
        }
    }

    fun result(files: Array<Uri>?) {
        val item = pending ?: return
        pending = null
        finish(item, files.takeIf { item.context != null && current() == item.context })
    }

    fun pageChanged(url: String?) {
        if (pending?.context?.page != url) cancel()
    }

    fun cancel() {
        pending?.let { finish(it, null) }
    }

    private fun finish(item: Pending, files: Array<Uri>?) {
        val callback = item.callback
        item.callback = null
        callback?.onReceiveValue(files)
    }
}
