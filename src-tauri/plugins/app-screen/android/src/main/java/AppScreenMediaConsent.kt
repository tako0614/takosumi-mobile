package com.takosumi.appscreen

import android.Manifest
import android.net.Uri
import android.webkit.PermissionRequest

/** One request's site consent, separate from the app-wide Android permission.
 * All calls and callbacks run on the Activity's UI thread. No site grant is
 * persisted or reused for another request, origin, document or profile. */
internal class AppScreenMediaConsent(
    private val current: () -> Context?,
    private val prompt: (Access, () -> Unit, () -> Unit) -> (() -> Unit),
    private val hasPermission: (String) -> Boolean,
    private val askAndroid: (Array<String>) -> Unit,
) {
    data class Context(
        val view: Any,
        val profile: String,
        val profileGeneration: Long,
        val document: Long,
        val page: String,
    )

    data class Access(val origin: String, val topOrigin: String, val resources: List<String>) {
        val permissions: List<String> get() = resources.map {
            if (it == PermissionRequest.RESOURCE_VIDEO_CAPTURE) Manifest.permission.CAMERA
            else Manifest.permission.RECORD_AUDIO
        }
    }

    companion object {
        fun access(origin: String, page: String, resources: Array<String>): Access? {
            if (origin.length > 2048 || origin.any { it.isISOControl() }) return null
            if (resources.isEmpty() || resources.size > 2 || resources.toSet().size != resources.size) return null
            if (resources.any { it != PermissionRequest.RESOURCE_VIDEO_CAPTURE && it != PermissionRequest.RESOURCE_AUDIO_CAPTURE }) return null
            if (!AppScreenPolicy.isLaunchable(origin) || !AppScreenPolicy.isLaunchable(page)) return null
            val uri = Uri.parse(origin)
            if (!uri.path.isNullOrEmpty() && uri.path != "/") return null
            if (uri.query != null || uri.fragment != null) return null
            val requesting = AppScreenRules.originOf(origin) ?: return null
            val top = AppScreenRules.originOf(page) ?: return null
            return Access(requesting, top, resources.toList())
        }
    }

    private class Pending(val request: PermissionRequest, val context: Context, val access: Access) {
        var dismiss: (() -> Unit)? = null
        var androidReturned = false
    }
    private var pending: Pending? = null
    // Kept after cancellation until its callback arrives. A previous OS
    // dialog's result must never settle a newer request.
    private var androidInFlight: Pending? = null

    fun begin(request: PermissionRequest, owner: Any) {
        if (pending?.request === request) return
        val context = current()
        val access = context?.let { access(request.origin.toString(), it.page, request.resources) }
        if (pending != null || androidInFlight != null || context == null || context.view !== owner || access == null) {
            request.deny()
            return
        }
        val item = Pending(request, context, access)
        pending = item
        try {
            val dismiss = prompt(access, { allow(item) }, { if (pending === item) cancel() })
            if (pending === item) item.dismiss = dismiss else dismiss()
        } catch (_: RuntimeException) {
            if (pending === item) cancel()
        }
    }

    private fun closePrompt(item: Pending) {
        val dismiss = item.dismiss
        item.dismiss = null
        dismiss?.invoke()
    }

    private fun allow(item: Pending) {
        if (pending !== item) return
        if (current() != item.context) {
            cancel()
            return
        }
        closePrompt(item)
        if (item.access.permissions.all(hasPermission)) {
            finish(item)
            return
        }
        androidInFlight = item
        try {
            askAndroid(item.access.permissions.toTypedArray())
        } catch (_: RuntimeException) {
            androidInFlight = null
            cancel()
        }
    }

    fun androidResult(granted: Map<String, Boolean>) {
        val item = androidInFlight ?: return
        androidInFlight = null
        if (pending !== item) return
        if (!item.access.permissions.all { granted[it] == true && hasPermission(it) }) {
            cancel()
            return
        }
        item.androidReturned = true
        // ActivityResult may arrive before onResume. Only the resumed host
        // can finish; onStop, detach, navigation and destroy cancel meanwhile.
        if (current() != null) finish(item)
    }

    fun resumed() {
        pending?.takeIf { it.androidReturned }?.let { finish(it) }
    }

    fun paused() {
        // The OS permission dialog can itself pause the Activity.
        if (androidInFlight == null) cancel()
    }

    fun pageChanged(url: String?) {
        if (pending?.context?.page != url) cancel()
    }

    fun canceledByWebView(request: PermissionRequest) {
        val item = pending?.takeIf { it.request === request } ?: return
        pending = null
        closePrompt(item) // already canceled by WebView: do not answer it again
    }

    fun cancel() {
        val item = pending ?: return
        pending = null
        closePrompt(item)
        item.request.deny()
    }

    private fun finish(item: Pending) {
        if (pending !== item) return
        if (current() != item.context || !item.access.permissions.all(hasPermission)) {
            cancel()
            return
        }
        pending = null
        closePrompt(item)
        // Explicit allowlist, never grant(request.resources): WebView may
        // add new protected resource kinds on future Android releases.
        item.request.grant(item.access.resources.toTypedArray())
    }
}
