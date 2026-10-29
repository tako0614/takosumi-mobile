package com.takosumi.appscreen

import android.annotation.SuppressLint
import android.content.Context
import android.content.MutableContextWrapper
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.WebSettings
import android.webkit.WebView

/**
 * Process-wide app screen webviews, kept alive across activity changes so
 * switching apps keeps each page's state. Only [MAX_ALIVE] stay alive; the
 * least recently used one is closed first, and its activity reloads the last
 * URL when it comes back.
 *
 * Every webview uses the process's single WebView profile (one cookie jar and
 * storage area), so a Takosumi sign-in made in one app screen carries over to
 * the others. Do not give screens their own data directory or profile.
 *
 * Each webview lives in a [MutableContextWrapper]: it points at the activity
 * that shows it (dialogs, pickers) and back at the application while detached.
 */
internal object AppScreenRegistry {
    const val MAX_ALIVE = 4

    class Entry(
        val key: String,
        val webView: WebView,
        val context: MutableContextWrapper,
        val popup: Boolean,
    ) {
        /** URL the screen was opened with; reopening with it keeps the state. */
        var launchUrl: String = ""
        var lastUrl: String = ""
        var title: String = ""
        var host: AppScreenActivity? = null
        /** For a popup: the screen whose page opened it. */
        var openerKey: String? = null
        /** For a popup: its first main-frame navigation has been judged. */
        var popupTargetDecided = false
        val createdAt: Long = android.os.SystemClock.elapsedRealtime()
        var destroyed = false
    }

    // Access-ordered: iteration starts at the least recently used entry.
    private val entries = LinkedHashMap<String, Entry>(8, 0.75f, true)

    fun get(key: String): Entry? = entries[key]?.takeUnless { it.destroyed }

    fun obtain(context: Context, key: String, popup: Boolean = false): Entry {
        get(key)?.let { return it }
        val wrapper = MutableContextWrapper(context.applicationContext)
        val entry = Entry(key, createWebView(wrapper), wrapper, popup)
        entries[key] = entry
        evictExcept(key)
        return entry
    }

    fun remove(key: String) {
        val entry = entries.remove(key) ?: return
        destroy(entry, crashed = false)
    }

    /** A webview whose renderer died may only be destroyed, never used. */
    fun removeCrashed(key: String) {
        val entry = entries.remove(key) ?: return
        destroy(entry, crashed = true)
    }

    fun findByWebView(webView: WebView): Entry? =
        entries.values.firstOrNull { it.webView === webView && !it.destroyed }

    /** The activity showing the screen that opened popup [popupKey], if any. */
    fun openerHost(popupKey: String): AppScreenActivity? =
        get(popupKey)?.openerKey?.let { get(it)?.host }

    /** Renderer gone for a webview no activity is handling: drop it safely. */
    fun handleCrashedWebView(webView: WebView) {
        val entry = findByWebView(webView)
        if (entry == null) {
            (webView.parent as? ViewGroup)?.removeView(webView)
            webView.destroy()
            return
        }
        entry.host?.releaseEvictedWebView(entry)
        removeCrashed(entry.key)
    }

    /** Close the oldest screens beyond the limit, never [keepKey] or a visible one. */
    private fun evictExcept(keepKey: String) {
        if (entries.size <= MAX_ALIVE) return
        val candidates = entries.values.filter { entry ->
            entry.key != keepKey && entry.host?.isVisibleScreen() != true
        }
        for (entry in candidates) {
            if (entries.size <= MAX_ALIVE) break
            entries.remove(entry.key)
            entry.host?.releaseEvictedWebView(entry)
            destroy(entry, crashed = false)
        }
    }

    private fun destroy(entry: Entry, crashed: Boolean) {
        if (entry.destroyed) return
        entry.destroyed = true
        val webView = entry.webView
        (webView.parent as? ViewGroup)?.removeView(webView)
        entry.host = null
        entry.context.baseContext = entry.context.applicationContext
        if (!crashed) {
            webView.stopLoading()
            webView.loadUrl("about:blank")
        }
        webView.destroy()
    }

    @SuppressLint("SetJavaScriptEnabled")
    private fun createWebView(context: MutableContextWrapper): WebView {
        val webView = WebView(context)
        with(webView.settings) {
            javaScriptEnabled = true
            domStorageEnabled = true
            setSupportMultipleWindows(true)
            // Popups need a user gesture, as in a browser.
            javaScriptCanOpenWindowsAutomatically = false
            allowFileAccess = false
            allowContentAccess = false
            mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
            mediaPlaybackRequiresUserGesture = true
            setGeolocationEnabled(false)
            setSupportZoom(true)
            builtInZoomControls = true
            displayZoomControls = false
            safeBrowsingEnabled = true
        }
        val cookies = CookieManager.getInstance()
        cookies.setAcceptCookie(true)
        cookies.setAcceptThirdPartyCookies(webView, false)
        // A webview without a client hands navigations to other apps; keep the
        // policy from the first navigation (a popup loads before its activity).
        webView.webViewClient = DetachedScreenClient
        // TASK-0119 U4 step 4 adds the document-start Push API bridge here.
        return webView
    }
}
