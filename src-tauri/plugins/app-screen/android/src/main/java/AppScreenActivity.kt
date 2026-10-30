package com.takosumi.appscreen

import android.app.Activity
import android.app.ActivityManager
import android.content.ActivityNotFoundException
import android.content.ContentResolver
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.graphics.Typeface
import android.net.Uri
import android.net.http.SslError
import android.os.Bundle
import android.os.Message
import android.os.SystemClock
import android.text.TextUtils
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.webkit.GeolocationPermissions
import android.webkit.PermissionRequest
import android.webkit.RenderProcessGoneDetail
import android.webkit.SslErrorHandler
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.PopupMenu
import android.widget.ProgressBar
import android.widget.TextView
import android.widget.Toast
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import java.io.ByteArrayInputStream
import java.lang.ref.WeakReference
import java.util.UUID

/**
 * One installed app, shown in a native WebView under a thin bar: the current
 * host (always visible; there is no address field), back, reload, close and a
 * menu with share, the browser view, the browser, the app's declared native
 * app, 開き方を変える, add to the home screen and switch apps. Each app runs
 * in its own task (FLAG_ACTIVITY_NEW_DOCUMENT) named and drawn after the app,
 * so Recents lists apps separately.
 *
 * The main frame shows only the app's own origin and the instance's Accounts
 * origin. Any other https page, such as an external sign-in (Google forbids
 * OAuth in embedded webviews), opens in the browser view over this screen;
 * closing it comes back here.
 */
class AppScreenActivity : AppCompatActivity() {
    private lateinit var request: ScreenRequest
    private var entry: AppScreenRegistry.Entry? = null
    private var lastKnownUrl: String = ""
    private var visible = false
    private var evicted = false
    private var externalDialog: AlertDialog? = null
    private var lastExternalDeclinedAt = 0L
    private var lastBrowserViewAt = 0L
    private var lastDownloadPromptAt = 0L
    private var pendingFileCallback: ValueCallback<Array<Uri>>? = null

    private lateinit var container: FrameLayout
    private lateinit var hostView: TextView
    private lateinit var backButton: TextView
    private lateinit var progress: ProgressBar

    private val fileChooser =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            val callback = pendingFileCallback
            pendingFileCallback = null
            callback?.onReceiveValue(pickedFiles(result.resultCode, result.data))
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val parsed = AppScreenLaunch.parse(this, intent)
        if (parsed == null) {
            finishAndRemoveTask()
            return
        }
        request = parsed
        if (!parsed.popup && !AppScreenPolicy.isInScreen(parsed.url, parsed.origins)) {
            // A Takosumi page outside sign-in (a stale shortcut to the
            // dashboard) never runs in an app screen.
            try {
                BrowserView.open(this, parsed.url, BAR_COLOR)
            } catch (_: Exception) {
                // Nothing else can show it.
            }
            finishAndRemoveTask()
            return
        }
        if (!parsed.popup) AppScreenLaunch.remember(this, parsed)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        val root = buildLayout()
        setContentView(root)
        WindowCompat.getInsetsController(window, root).isAppearanceLightStatusBars = false
        onBackPressedDispatcher.addCallback(
            this,
            object : OnBackPressedCallback(true) {
                override fun handleOnBackPressed() = goBackOrLeave()
            },
        )
        val restored = savedInstanceState?.getString(STATE_URL)?.let { restorable(it) }
        show(target = restored ?: startUrl(), keepLivePage = keepsLivePage(request))
        applyTaskDescription()
        container.post { reportOpened() }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        val parsed = AppScreenLaunch.parse(this, intent) ?: return
        if (parsed.key != request.key || parsed.popup) return
        setIntent(intent)
        val keep = keepsLivePage(parsed)
        request = parsed
        show(target = startUrl(), keepLivePage = keep)
        applyTaskDescription()
        reportOpened()
    }

    override fun onStart() {
        super.onStart()
        visible = true
        if (evicted) {
            show(target = restorable(lastKnownUrl) ?: startUrl(), keepLivePage = false)
        }
    }

    override fun onStop() {
        visible = false
        super.onStop()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        restorable(lastKnownUrl)?.let { outState.putString(STATE_URL, it) }
    }

    override fun onDestroy() {
        externalDialog?.dismiss()
        pendingFileCallback?.onReceiveValue(null)
        pendingFileCallback = null
        val current = entry
        if (current != null && current.host === this) {
            unbind(current)
            // Closed, swiped away or a finished popup: drop its page. Otherwise
            // (the system recreating the activity) keep it for the next one.
            if (isFinishing) AppScreenRegistry.remove(current.key)
        }
        entry = null
        super.onDestroy()
    }

    /** The registry closed this screen's webview (memory, or a renderer crash). */
    internal fun releaseEvictedWebView(evictedEntry: AppScreenRegistry.Entry) {
        if (entry !== evictedEntry) return
        unbind(evictedEntry)
        entry = null
        evicted = true
        if (request.popup) finish()
    }

    /** Another activity took over this webview (same app relaunched). */
    internal fun forgetEntry(moved: AppScreenRegistry.Entry) {
        if (entry === moved) entry = null
    }

    /** Started (visible, even when not focused in split screen). */
    internal fun isVisibleScreen(): Boolean = visible

    /**
     * A page this screen may reload after the system recreated it, a renderer
     * crash or eviction: the app's own origin without credentials (not an
     * OAuth callback whose code is spent, not an Accounts authorize page).
     */
    private fun restorable(url: String): String? =
        url.takeIf { candidate ->
            !request.popup && request.appOrigin?.let { AppScreenRules.isRememberable(candidate, it) } == true
        }

    /** The page to open at: the app's last page when resuming, else its URL. */
    private fun startUrl(): String =
        (if (request.resume) AppScreenResume.recall(this, request) else null) ?: request.url

    /**
     * A live page is kept when the app is reopened (resume) or opened again
     * with its own launch URL, for the same (instance, account). A deep link to
     * another page loads that page.
     */
    private fun keepsLivePage(next: ScreenRequest): Boolean {
        val existing = AppScreenRegistry.get(next.key) ?: return false
        if (next.popup) return true
        return existing.scope == next.scope && (next.resume || existing.launchUrl == next.url)
    }

    /**
     * Show this screen's webview. With [keepLivePage] an existing page keeps
     * its state (tapping the app again); anything else (a deep link, a reload
     * after eviction) loads [target].
     */
    private fun show(target: String, keepLivePage: Boolean) {
        evicted = false
        val existing = AppScreenRegistry.get(request.key)
        if (request.popup && existing == null) {
            finish()
            return
        }
        val current = existing
            ?: AppScreenRegistry.obtain(this, request.key, request.popup).also {
                it.launchUrl = request.url
            }
        if (!request.popup) {
            current.origins = request.origins
            current.scope = request.scope
            if (!keepLivePage) current.launchUrl = request.url
        }
        bind(current)
        current.title = request.title
        val keptUrl = existing?.webView?.url
        val keep = existing != null && keptUrl != null && keepLivePage
        if (!request.popup && !keep) {
            if (AppScreenPolicy.isInScreen(target, current.origins)) current.webView.loadUrl(target)
            else current.webView.loadUrl(request.url)
        } else if (keptUrl != null && !isAllowedCommittedUrl(keptUrl, current.origins)) {
            // A detached page ended up outside the policy (a blocked POST).
            current.webView.loadUrl(
                current.lastUrl.takeIf { AppScreenPolicy.isInScreen(it, current.origins) }
                    ?: if (request.popup) "about:blank" else request.url,
            )
        }
        updateBar(current.webView.url ?: target)
    }

    private fun bind(current: AppScreenRegistry.Entry) {
        if (entry === current && current.host === this) return
        entry?.let { if (it.host === this) unbind(it) }
        current.host?.takeIf { it !== this }?.forgetEntry(current)
        val webView = current.webView
        (webView.parent as? ViewGroup)?.removeView(webView)
        current.context.baseContext = this
        current.host = this
        webView.webViewClient = ScreenClient(current)
        webView.webChromeClient = ScreenChromeClient()
        webView.setDownloadListener { url, _, _, _, _ ->
            // A page cannot keep raising download prompts.
            val now = SystemClock.elapsedRealtime()
            if (now - lastDownloadPromptAt >= DOWNLOAD_PROMPT_COOLDOWN_MS) {
                lastDownloadPromptAt = now
                confirmExternal(url, download = true, userGesture = true)
            }
        }
        container.addView(
            webView,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            ),
        )
        entry = current
    }

    private fun unbind(current: AppScreenRegistry.Entry) {
        val webView = current.webView
        container.removeView(webView)
        webView.webViewClient = current.detachedClient
        webView.webChromeClient = null
        webView.setDownloadListener(null)
        current.host = null
        current.context.baseContext = applicationContext
    }

    private fun goBackOrLeave() {
        val webView = entry?.webView
        when {
            webView != null && webView.canGoBack() -> webView.goBack()
            request.popup -> closeScreen()
            // Keep the page and its Recents entry, as a browser tab would.
            else -> moveTaskToBack(true)
        }
    }

    private fun closeScreen() {
        if (request.popup) finish() else finishAndRemoveTask()
    }

    private fun switchApps() {
        val launch = packageManager.getLaunchIntentForPackage(packageName) ?: return
        launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        startActivity(launch)
    }

    /**
     * The app itself, as a shortcut or the chooser opens it: its own launch
     * URL (not a deep-linked page inside it), reopening at its last page.
     */
    private fun appRequest(): ScreenRequest =
        request.copy(
            url = entry?.launchUrl?.takeIf { AppScreenPolicy.isLaunchable(it) } ?: request.url,
            resume = true,
        )

    private fun reportOpened() {
        if (!request.popup) AppScreenShortcuts.reportOpened(this, appRequest())
    }

    private fun rememberUrl(url: String?) {
        val current = entry ?: return
        if (url != null && AppScreenPolicy.isInScreen(url, current.origins)) {
            lastKnownUrl = url
            current.lastUrl = url
            // Only an app opened as itself (tile, recent, shortcut) keeps its
            // last page; a deep-linked page never becomes it.
            if (!request.popup && request.resume) AppScreenResume.record(this, request, url)
        }
    }

    private fun updateBar(url: String?) {
        val host = AppScreenPolicy.displayHost(url)
        val secure = url?.startsWith("https://", ignoreCase = true) == true
        hostView.text = if (secure) "🔒 $host" else host
        hostView.contentDescription = "表示中のサイト $host"
        val canGoBack = entry?.webView?.canGoBack() == true
        backButton.isEnabled = canGoBack
        backButton.alpha = if (canGoBack) 1f else 0.35f
    }

    private fun applyTaskDescription() {
        if (request.popup) return
        val label = request.title.ifBlank { AppScreenPolicy.displayHost(request.url) }
        val icon = AppScreenIcons.load(this, request.key)
        // TaskDescription.Builder takes only resource icons; the app's icon is a
        // bitmap drawn at runtime, which only this constructor accepts.
        @Suppress("DEPRECATION")
        setTaskDescription(ActivityManager.TaskDescription(label, icon, BAR_COLOR))
    }

    private fun showMenu(anchor: View) {
        val menu = PopupMenu(this, anchor)
        menu.menu.add(0, MENU_SHARE, 0, "共有")
        menu.menu.add(0, MENU_BROWSER_VIEW, 1, "ブラウザの画面で開く")
        menu.menu.add(0, MENU_BROWSER, 2, "ブラウザで開く")
        val client = request.client.takeUnless { request.popup }
        if (client != null) {
            menu.menu.add(0, MENU_CLIENT, 3, if (client.verified) "${client.name}で開く" else "${client.name}で開く…")
        }
        if (!request.popup) menu.menu.add(0, MENU_CHOOSE, 4, "開き方を変える")
        if (!request.popup && AppScreenShortcuts.isPinSupported(this)) {
            menu.menu.add(0, MENU_PIN, 5, "ホーム画面に追加")
        }
        menu.menu.add(0, MENU_SWITCH, 6, "アプリを切り替える")
        menu.menu.add(0, MENU_CLOSE, 7, "閉じる")
        menu.setOnMenuItemClickListener { item ->
            when (item.itemId) {
                MENU_SHARE -> share()
                MENU_BROWSER_VIEW -> openBrowserView(AppScreenRules.shareable(currentUrl() ?: appRequest().url))
                MENU_BROWSER -> currentUrl()?.let { openInBrowser(AppScreenRules.shareable(it)) }
                MENU_CLIENT -> client?.let { openClient(it) }
                MENU_CHOOSE -> askShell(AppScreenActions.CHANGE)
                MENU_PIN -> pinShortcut()
                MENU_SWITCH -> switchApps()
                MENU_CLOSE -> closeScreen()
            }
            true
        }
        menu.show()
    }

    /**
     * 「{name}で開く」: a verified app opens directly; a missing one, or an
     * unverified one (which must be confirmed with its real destination
     * shown), goes to the shell's chooser.
     */
    private fun openClient(client: BannerClient) {
        val packageName = client.packageName
        if (!client.verified || packageName == null) {
            askShell(AppScreenActions.UNVERIFIED)
            return
        }
        val result = AppClientLauncher.open(this, packageName, client.appLink, client.open)
        if (result != AppClientLauncher.OPENED) askShell(AppScreenActions.NOT_INSTALLED)
    }

    /** Leave the chooser for this app to the shell and bring it forward. */
    private fun askShell(reason: String) {
        try {
            AppScreenActions.request(this, PendingAction(request.key, appRequest().url, reason))
        } catch (_: Exception) {
            Toast.makeText(this, "Takosumiを開けませんでした", Toast.LENGTH_SHORT).show()
        }
    }

    /**
     * A main-frame page outside this screen's origins. Opened in the browser
     * view at once; without a user gesture (a redirect, a script) at most once
     * every few seconds, so a page cannot keep popping it up.
     */
    internal fun handOffToBrowserView(url: String, userGesture: Boolean) {
        if (isFinishing) return
        val now = SystemClock.elapsedRealtime()
        if (!userGesture && now - lastBrowserViewAt < BROWSER_VIEW_COOLDOWN_MS) return
        lastBrowserViewAt = now
        openBrowserView(url)
    }

    private fun openInBrowser(url: String) {
        try {
            BrowserView.openInBrowser(this, url)
        } catch (_: Exception) {
            Toast.makeText(this, "開けるブラウザがありません", Toast.LENGTH_SHORT).show()
        }
    }

    private fun openBrowserView(url: String) {
        if (!AppScreenPolicy.isLaunchable(url)) return
        try {
            BrowserView.open(this, url, BAR_COLOR)
        } catch (_: Exception) {
            Toast.makeText(this, "開けるブラウザがありません", Toast.LENGTH_SHORT).show()
        }
    }

    /**
     * A request to another origin that cannot be handed over as a link (a form
     * POST). It was refused; offer to continue from this page in the browser
     * view, where the person can repeat it.
     */
    internal fun offerBrowserViewForPage(blockedUrl: String) {
        if (externalDialog?.isShowing == true || isFinishing) return
        if (SystemClock.elapsedRealtime() - lastExternalDeclinedAt < PROMPT_COOLDOWN_MS) return
        val page = AppScreenRules.shareable(currentUrl() ?: appRequest().url)
        val host = AppScreenPolicy.displayHost(blockedUrl).ifEmpty { "ほかのサイト" }
        var opened = false
        externalDialog = AlertDialog.Builder(this)
            .setTitle("ブラウザの画面で続ける")
            .setMessage("$host への送信は Takosumi の中ではできません。このページをブラウザの画面で開いて続けますか？")
            .setNegativeButton("キャンセル", null)
            .setPositiveButton("ブラウザの画面で開く") { _, _ ->
                opened = true
                openBrowserView(page)
            }
            .setOnDismissListener {
                if (!opened) lastExternalDeclinedAt = SystemClock.elapsedRealtime()
            }
            .show()
    }

    private fun currentUrl(): String? =
        entry?.webView?.url?.takeIf { AppScreenPolicy.isInScreen(it, entry?.origins) }

    private fun share() {
        // Never a spent OAuth code or a token from the page's URL.
        val url = currentUrl()?.let { AppScreenRules.shareable(it) } ?: return
        val send = Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_TEXT, url)
            entry?.webView?.title?.let { putExtra(Intent.EXTRA_SUBJECT, it) }
        }
        startActivity(Intent.createChooser(send, null))
    }

    private fun pinShortcut() {
        val requested = try {
            AppScreenShortcuts.requestPin(this, appRequest())
        } catch (_: Exception) {
            false
        }
        Toast.makeText(
            this,
            if (requested) "ホーム画面への追加を確認してください" else "このホームアプリは追加に対応していません",
            Toast.LENGTH_SHORT,
        ).show()
    }

    /**
     * Ask before leaving the app. Without a user gesture, a page that was just
     * declined cannot ask again for a few seconds (no prompt loops).
     */
    internal fun confirmExternal(
        url: String,
        insecureHttp: Boolean = false,
        download: Boolean = false,
        userGesture: Boolean,
    ) {
        if (externalDialog?.isShowing == true || isFinishing) return
        if (!userGesture && SystemClock.elapsedRealtime() - lastExternalDeclinedAt < PROMPT_COOLDOWN_MS) {
            return
        }
        val decision = AppScreenPolicy.decide(url, mainFrame = true)
        if (decision.kind == NavigationKind.BLOCK) return
        if (download && decision.kind != NavigationKind.IN_APP && !decision.insecureHttp) return
        val host = AppScreenPolicy.displayHost(entry?.webView?.url).ifEmpty { "このページ" }
        val message = when {
            download -> "$host のファイルをブラウザで開きますか？\n\n$url"
            insecureHttp -> "このページは https ではないため、Takosumi の中では開けません。ブラウザで開きますか？\n\n$url"
            else -> "$host が外部のアプリを開こうとしています。開きますか？\n\n$url"
        }
        var opened = false
        externalDialog = AlertDialog.Builder(this)
            .setTitle("外部で開く")
            .setMessage(message)
            .setNegativeButton("キャンセル", null)
            .setPositiveButton("開く") { _, _ ->
                opened = true
                openExternally(url)
            }
            .setOnDismissListener {
                if (!opened) lastExternalDeclinedAt = SystemClock.elapsedRealtime()
            }
            .show()
    }

    private fun openExternally(url: String) {
        val intent = if (url.startsWith("intent:", ignoreCase = true)) {
            try {
                Intent.parseUri(url, Intent.URI_INTENT_SCHEME)
            } catch (_: Exception) {
                null
            }
        } else {
            Intent(Intent.ACTION_VIEW, Uri.parse(url))
        } ?: return
        // A page must not reach Takosumi Mobile itself (forged OIDC callbacks,
        // app-screen or connect links) through an intent: URI.
        if (intent.scheme?.equals(SHELL_SCHEME, ignoreCase = true) == true ||
            intent.`package` == packageName
        ) {
            return
        }
        // As browsers do: only browsable activities, no explicit target, no
        // URI permission grants carried over from the page.
        intent.addCategory(Intent.CATEGORY_BROWSABLE)
        intent.component = null
        intent.selector = null
        intent.clipData = null
        intent.flags = Intent.FLAG_ACTIVITY_NEW_TASK
        val ownHandler = try {
            packageManager.queryIntentActivities(intent, PackageManager.MATCH_DEFAULT_ONLY)
                .any { it.activityInfo?.packageName == packageName }
        } catch (_: Exception) {
            true
        }
        if (ownHandler) return
        try {
            startActivity(intent)
        } catch (_: ActivityNotFoundException) {
            val fallback = intent.getStringExtra("browser_fallback_url")
            if (fallback != null && AppScreenPolicy.isInScreen(fallback, entry?.origins)) {
                entry?.webView?.loadUrl(fallback)
            } else if (fallback != null && AppScreenPolicy.isLaunchable(fallback)) {
                handOffToBrowserView(fallback, userGesture = true)
            } else {
                Toast.makeText(this, "開けるアプリがありません", Toast.LENGTH_SHORT).show()
            }
        } catch (_: SecurityException) {
            Toast.makeText(this, "開けるアプリがありません", Toast.LENGTH_SHORT).show()
        }
    }

    /**
     * Files picked for `<input type=file>`. Only content URIs from other apps'
     * providers are passed on, so a picker cannot hand back this app's own
     * private files for upload. Multiple selections come through ClipData.
     */
    private fun pickedFiles(resultCode: Int, data: Intent?): Array<Uri>? {
        if (resultCode != Activity.RESULT_OK || data == null) return null
        val candidates = mutableListOf<Uri>()
        data.data?.let { candidates.add(it) }
        data.clipData?.let { clip ->
            for (index in 0 until clip.itemCount) clip.getItemAt(index).uri?.let { candidates.add(it) }
        }
        val accepted = candidates.distinct().filter { uri ->
            if (uri.scheme != ContentResolver.SCHEME_CONTENT) return@filter false
            val authority = uri.authority ?: return@filter false
            val provider = try {
                packageManager.resolveContentProvider(authority, 0)
            } catch (_: Exception) {
                null
            }
            provider != null && provider.packageName != packageName
        }
        return if (accepted.isEmpty()) null else accepted.toTypedArray()
    }

    /** A popup's first main-frame navigation: where window.open sent it. */
    private fun isPopupTarget(): Boolean {
        val current = entry ?: return false
        if (!current.popup || current.popupTargetDecided) return false
        current.popupTargetDecided = true
        // window.open(url) navigates at once; a document-written popup's first
        // navigation is a later tap on its content and must not close it.
        return SystemClock.elapsedRealtime() - current.createdAt < POPUP_TARGET_WINDOW_MS &&
            current.webView.url.let { it == null || it == "about:blank" } &&
            !current.webView.canGoBack()
    }

    private inner class ScreenClient(private val bound: AppScreenRegistry.Entry) : WebViewClient() {
        override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
            val url = request.url.toString()
            val decision = AppScreenPolicy.decide(url, request.isForMainFrame, bound.origins)
            val popupTarget = request.isForMainFrame && view === entry?.webView && isPopupTarget()
            if (decision.kind == NavigationKind.IN_APP) return false
            if (request.isForMainFrame && view === entry?.webView) {
                if (popupTarget) {
                    // A target=_blank / window.open to another origin, tel:,
                    // http: and the like: hand it over from the opener and drop
                    // the empty popup.
                    val opener = AppScreenRegistry.openerHost(this@AppScreenActivity.request.key)
                    when (decision.kind) {
                        NavigationKind.EXTERNAL_CONFIRM -> opener?.confirmExternal(
                            url,
                            insecureHttp = decision.insecureHttp,
                            userGesture = request.hasGesture(),
                        )
                        NavigationKind.BROWSER_VIEW ->
                            (opener ?: this@AppScreenActivity).handOffToBrowserView(url, request.hasGesture())
                        else -> Unit
                    }
                    closeScreen()
                } else {
                    when (decision.kind) {
                        NavigationKind.EXTERNAL_CONFIRM -> confirmExternal(
                            url,
                            insecureHttp = decision.insecureHttp,
                            userGesture = request.hasGesture(),
                        )
                        NavigationKind.BROWSER_VIEW -> handOffToBrowserView(url, request.hasGesture())
                        else -> Unit
                    }
                }
            }
            return true
        }

        /**
         * POSTs and their redirects never reach shouldOverrideUrlLoading; refuse
         * any main-frame request that is not https, or not on this screen's
         * origins, before it loads.
         */
        override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
            if (!request.isForMainFrame) return null
            val url = request.url.toString()
            if (request.url.scheme?.lowercase() != "https") return blockedResponse()
            val origins = bound.origins ?: return null
            if (AppScreenPolicy.decide(url, mainFrame = true, origins).kind == NavigationKind.IN_APP) return null
            view.post { if (view === entry?.webView) offerBrowserViewForPage(url) }
            return blockedResponse()
        }

        override fun onPageStarted(view: WebView, url: String?, favicon: android.graphics.Bitmap?) {
            if (view !== entry?.webView) return
            if (url != null && !isAllowedCommittedUrl(url, bound.origins)) {
                // Something reached the main frame without passing the policy.
                view.stopLoading()
                leavePage(view, url, offerOnly = true)
                return
            }
            progress.visibility = View.VISIBLE
            updateBar(url)
        }

        override fun onPageFinished(view: WebView, url: String?) {
            if (view !== entry?.webView) return
            progress.visibility = View.GONE
            if (url != null && !isAllowedCommittedUrl(url, bound.origins)) {
                leavePage(view, url, offerOnly = false)
                return
            }
            rememberUrl(url)
            updateBar(url)
        }

        /**
         * Also the URL changes of a single-page app (`history.pushState`),
         * which no navigation callback sees: the Takosumi sign-in screen is
         * the dashboard's app and could move to the dashboard in place.
         */
        override fun doUpdateVisitedHistory(view: WebView, url: String?, isReload: Boolean) {
            if (view !== entry?.webView) return
            if (url != null && !isAllowedCommittedUrl(url, bound.origins)) {
                leavePage(view, url, offerOnly = false)
                return
            }
            rememberUrl(url)
            updateBar(url)
        }

        /**
         * Leave a page the screen does not show: hand it over (browser view or
         * the OS) and go back to the last page that was allowed.
         */
        private fun leavePage(view: WebView, url: String, offerOnly: Boolean) {
            val decision = AppScreenPolicy.decide(url, mainFrame = true, bound.origins)
            when (decision.kind) {
                NavigationKind.EXTERNAL_CONFIRM ->
                    confirmExternal(url, insecureHttp = decision.insecureHttp, userGesture = false)
                NavigationKind.BROWSER_VIEW ->
                    if (offerOnly) offerBrowserViewForPage(url) else handOffToBrowserView(url, userGesture = false)
                else -> Unit
            }
            when {
                view.canGoBack() -> view.goBack()
                this@AppScreenActivity.request.popup -> closeScreen()
                else -> view.loadUrl(restorable(lastKnownUrl) ?: this@AppScreenActivity.request.url)
            }
        }

        override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: SslError) {
            handler.cancel()
        }

        override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
            val current = AppScreenRegistry.findByWebView(view)
            if (current == null) {
                (view.parent as? ViewGroup)?.removeView(view)
                view.destroy()
                return true
            }
            if (current === entry) {
                unbind(current)
                entry = null
            }
            AppScreenRegistry.removeCrashed(current.key)
            if (current.key == this@AppScreenActivity.request.key) {
                when {
                    this@AppScreenActivity.request.popup -> finish()
                    visible -> show(target = restorable(lastKnownUrl) ?: startUrl(), keepLivePage = false)
                    else -> evicted = true
                }
            }
            return true
        }
    }

    private inner class ScreenChromeClient : WebChromeClient() {
        override fun onCreateWindow(
            view: WebView,
            isDialog: Boolean,
            isUserGesture: Boolean,
            resultMsg: Message,
        ): Boolean {
            if (!isUserGesture) return false
            val transport = resultMsg.obj as? WebView.WebViewTransport ?: return false
            val key = "popup:${UUID.randomUUID()}"
            val popup = AppScreenRegistry.obtain(this@AppScreenActivity, key, popup = true)
            popup.openerKey = request.key
            // A popup keeps its opener's origins.
            popup.origins = entry?.origins
            popup.webView.webViewClient = PopupPrebindClient(popup)
            transport.webView = popup.webView
            resultMsg.sendToTarget()
            startActivity(AppScreenLaunch.popupIntent(this@AppScreenActivity, key, request.title))
            return true
        }

        override fun onCloseWindow(window: WebView) {
            if (window === entry?.webView) closeScreen()
        }

        override fun onProgressChanged(view: WebView, newProgress: Int) {
            if (view !== entry?.webView) return
            progress.progress = newProgress
            progress.visibility = if (newProgress in 1..99) View.VISIBLE else View.GONE
        }

        override fun onShowFileChooser(
            webView: WebView,
            filePathCallback: ValueCallback<Array<Uri>>,
            fileChooserParams: FileChooserParams,
        ): Boolean {
            pendingFileCallback?.onReceiveValue(null)
            pendingFileCallback = filePathCallback
            return try {
                fileChooser.launch(fileChooserParams.createIntent())
                true
            } catch (_: ActivityNotFoundException) {
                pendingFileCallback = null
                false
            }
        }

        // Camera, microphone and location for app screens come later with an
        // origin-named prompt; until then they are refused.
        override fun onPermissionRequest(request: PermissionRequest) {
            request.deny()
        }

        override fun onGeolocationPermissionsShowPrompt(
            origin: String,
            callback: GeolocationPermissions.Callback,
        ) {
            callback.invoke(origin, false, false)
        }
    }

    private fun buildLayout(): View {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(BAR_COLOR)
        }
        val bar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(4), 0, dp(4), 0)
        }
        bar.addView(barButton("✕", "閉じる") { closeScreen() })
        hostView = TextView(this).apply {
            setTextColor(FOREGROUND)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
            typeface = Typeface.DEFAULT_BOLD
            isSingleLine = true
            // Long hosts keep their registrable end visible.
            ellipsize = TextUtils.TruncateAt.START
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(6), 0, dp(6), 0)
        }
        bar.addView(hostView, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.MATCH_PARENT, 1f))
        backButton = barButton("‹", "戻る") { entry?.webView?.takeIf { it.canGoBack() }?.goBack() }
        bar.addView(backButton)
        bar.addView(barButton("↻", "再読み込み") { entry?.webView?.reload() })
        bar.addView(barButton("⋮", "メニュー") { showMenu(it) })
        root.addView(bar, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(48)))

        progress = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal).apply {
            max = 100
            isIndeterminate = false
            visibility = View.GONE
        }
        root.addView(progress, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(2)))

        container = FrameLayout(this).apply { setBackgroundColor(Color.WHITE) }
        root.addView(container, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f))

        ViewCompat.setOnApplyWindowInsetsListener(root) { view, insets ->
            val bars = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or
                    WindowInsetsCompat.Type.displayCutout() or
                    WindowInsetsCompat.Type.ime(),
            )
            view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            WindowInsetsCompat.CONSUMED
        }
        return root
    }

    private fun barButton(glyph: String, label: String, onClick: (View) -> Unit): TextView =
        TextView(this).apply {
            text = glyph
            contentDescription = label
            setTextColor(FOREGROUND)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 20f)
            gravity = Gravity.CENTER
            minWidth = dp(44)
            minHeight = dp(44)
            isClickable = true
            isFocusable = true
            val ripple = TypedValue()
            if (theme.resolveAttribute(android.R.attr.selectableItemBackgroundBorderless, ripple, true)) {
                setBackgroundResource(ripple.resourceId)
            }
            setOnClickListener(onClick)
        }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    private companion object {
        const val STATE_URL = "com.takosumi.appscreen.state.URL"
        const val SHELL_SCHEME = "takosumi"
        const val PROMPT_COOLDOWN_MS = 5_000L
        const val BROWSER_VIEW_COOLDOWN_MS = 3_000L
        const val DOWNLOAD_PROMPT_COOLDOWN_MS = 3_000L
        const val POPUP_TARGET_WINDOW_MS = 3_000L
        const val MENU_SHARE = 1
        const val MENU_BROWSER = 2
        const val MENU_PIN = 3
        const val MENU_SWITCH = 4
        const val MENU_CLOSE = 5
        const val MENU_BROWSER_VIEW = 6
        const val MENU_CLIENT = 7
        const val MENU_CHOOSE = 8
        val BAR_COLOR = Color.parseColor("#151517")
        val FOREGROUND = Color.parseColor("#F5F5F6")
    }
}

/** Main-frame URLs a screen may show once committed. */
internal fun isAllowedCommittedUrl(url: String, origins: ScreenOrigins?): Boolean =
    AppScreenPolicy.isInScreen(url, origins) || url == "about:blank" || url == "about:srcdoc"

internal fun blockedResponse(): WebResourceResponse =
    WebResourceResponse(
        "text/plain",
        "utf-8",
        403,
        "Forbidden",
        emptyMap(),
        ByteArrayInputStream(ByteArray(0)),
    )

/** Refuse a main-frame request that is not https or not a page the screen shows. */
internal fun refusedMainFrame(request: WebResourceRequest, origins: ScreenOrigins?): Boolean {
    if (!request.isForMainFrame) return false
    if (request.url.scheme?.lowercase() != "https") return true
    return origins != null &&
        AppScreenPolicy.decide(request.url.toString(), mainFrame = true, origins).kind != NavigationKind.IN_APP
}

/** Client for a webview no activity shows: keeps the policy, asks nothing. */
internal class DetachedScreenClient(private val entry: AppScreenRegistry.Entry) : WebViewClient() {
    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean =
        AppScreenPolicy.decide(request.url.toString(), request.isForMainFrame, entry.origins).kind !=
            NavigationKind.IN_APP

    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? =
        if (refusedMainFrame(request, entry.origins)) blockedResponse() else null

    override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: SslError) {
        handler.cancel()
    }

    // All webviews share one renderer; when it dies every client hears about
    // it. Returning false here would abort the whole app.
    override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
        AppScreenRegistry.handleCrashedWebView(view)
        return true
    }
}

/**
 * A popup's webview between `window.open` and its activity binding. An in-app
 * first page loads normally; another origin goes to the browser view, other
 * schemes are offered on the opener, and the empty popup is dropped.
 */
internal class PopupPrebindClient(private val popup: AppScreenRegistry.Entry) : WebViewClient() {
    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
        val url = request.url.toString()
        val decision = AppScreenPolicy.decide(url, request.isForMainFrame, popup.origins)
        if (request.isForMainFrame) popup.popupTargetDecided = true
        if (decision.kind == NavigationKind.IN_APP) return false
        if (request.isForMainFrame) {
            val opener = AppScreenRegistry.openerHost(popup.key)
            when (decision.kind) {
                NavigationKind.EXTERNAL_CONFIRM -> opener?.confirmExternal(
                    url,
                    insecureHttp = decision.insecureHttp,
                    userGesture = request.hasGesture(),
                )
                NavigationKind.BROWSER_VIEW -> opener?.handOffToBrowserView(url, request.hasGesture())
                else -> Unit
            }
            val target = WeakReference(view)
            view.post {
                val popup = target.get()?.let { AppScreenRegistry.findByWebView(it) } ?: return@post
                // The popup's activity may have bound it meanwhile; let it finish.
                popup.host?.releaseEvictedWebView(popup)
                AppScreenRegistry.remove(popup.key)
            }
        }
        return true
    }

    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? =
        if (refusedMainFrame(request, popup.origins)) blockedResponse() else null

    override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: SslError) {
        handler.cancel()
    }

    override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
        AppScreenRegistry.handleCrashedWebView(view)
        return true
    }
}
