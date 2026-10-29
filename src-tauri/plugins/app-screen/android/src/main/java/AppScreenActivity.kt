package com.takosumi.appscreen

import android.app.ActivityManager
import android.content.ActivityNotFoundException
import android.content.Intent
import android.graphics.Color
import android.graphics.Typeface
import android.net.Uri
import android.net.http.SslError
import android.os.Bundle
import android.os.Message
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
import java.util.UUID

/**
 * One installed app, shown in a native WebView under a thin bar: the current
 * host (always visible; there is no address field), back, reload, close and a
 * menu with share, open in the browser, add to the home screen and switch
 * apps. Each app runs in its own task (FLAG_ACTIVITY_NEW_DOCUMENT) named and
 * drawn after the app, so Recents lists apps separately.
 */
class AppScreenActivity : AppCompatActivity() {
    private lateinit var request: ScreenRequest
    private var entry: AppScreenRegistry.Entry? = null
    private var lastKnownUrl: String = ""
    private var resumed = false
    private var evicted = false
    private var externalDialog: AlertDialog? = null
    private var pendingFileCallback: ValueCallback<Array<Uri>>? = null

    private lateinit var container: FrameLayout
    private lateinit var hostView: TextView
    private lateinit var backButton: TextView
    private lateinit var progress: ProgressBar

    private val fileChooser =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            val callback = pendingFileCallback
            pendingFileCallback = null
            callback?.onReceiveValue(
                WebChromeClient.FileChooserParams.parseResult(result.resultCode, result.data),
            )
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val parsed = AppScreenLaunch.parse(intent)
        if (parsed == null) {
            finishAndRemoveTask()
            return
        }
        request = parsed
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
        val restored = savedInstanceState?.getString(STATE_URL)
            ?.takeIf { AppScreenPolicy.isLaunchable(it) }
        show(target = restored ?: request.url, keepIfLaunchedWith = request.url)
        applyTaskDescription()
        container.post { AppScreenShortcuts.reportOpened(this, request) }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        val parsed = AppScreenLaunch.parse(intent) ?: return
        if (parsed.key != request.key || parsed.popup) return
        setIntent(intent)
        request = parsed
        show(target = parsed.url, keepIfLaunchedWith = parsed.url)
        applyTaskDescription()
        AppScreenShortcuts.reportOpened(this, request)
    }

    override fun onResume() {
        super.onResume()
        resumed = true
        if (evicted) {
            evicted = false
            show(target = lastKnownUrl.ifEmpty { request.url }, keepIfLaunchedWith = null)
        }
    }

    override fun onPause() {
        resumed = false
        super.onPause()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        if (AppScreenPolicy.isLaunchable(lastKnownUrl)) outState.putString(STATE_URL, lastKnownUrl)
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

    /** Called by the registry when this screen's webview is closed to save memory. */
    internal fun releaseEvictedWebView(evictedEntry: AppScreenRegistry.Entry) {
        if (entry !== evictedEntry) return
        unbind(evictedEntry)
        entry = null
        evicted = true
    }

    /** Another activity took over this webview (same app relaunched). */
    internal fun forgetEntry(moved: AppScreenRegistry.Entry) {
        if (entry === moved) entry = null
    }

    internal fun isVisibleScreen(): Boolean = resumed

    /**
     * Show this screen's webview. An existing page opened with
     * [keepIfLaunchedWith] keeps its state (tapping the app again); anything
     * else (a deep link, a reload after eviction) loads [target].
     */
    private fun show(target: String, keepIfLaunchedWith: String?) {
        val existing = AppScreenRegistry.get(request.key)
        if (request.popup && existing == null) {
            finish()
            return
        }
        val current = existing
            ?: AppScreenRegistry.obtain(this, request.key, request.popup).also {
                it.launchUrl = request.url
            }
        bind(current)
        current.title = request.title
        val keep = existing != null &&
            existing.webView.url != null &&
            keepIfLaunchedWith != null &&
            keepIfLaunchedWith == existing.launchUrl
        if (!request.popup && !keep) {
            if (AppScreenPolicy.isLaunchable(target)) current.webView.loadUrl(target)
            else current.webView.loadUrl(request.url)
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
        webView.webViewClient = ScreenClient()
        webView.webChromeClient = ScreenChromeClient()
        webView.setDownloadListener { url, _, _, _, _ -> confirmExternal(url, download = true) }
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
        webView.webViewClient = DetachedScreenClient
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

    private fun rememberUrl(url: String?) {
        if (url != null && AppScreenPolicy.isLaunchable(url)) {
            lastKnownUrl = url
            entry?.lastUrl = url
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
        menu.menu.add(0, MENU_BROWSER, 1, "ブラウザで開く")
        if (!request.popup && AppScreenShortcuts.isPinSupported(this)) {
            menu.menu.add(0, MENU_PIN, 2, "ホーム画面に追加")
        }
        menu.menu.add(0, MENU_SWITCH, 3, "アプリを切り替える")
        menu.menu.add(0, MENU_CLOSE, 4, "閉じる")
        menu.setOnMenuItemClickListener { item ->
            when (item.itemId) {
                MENU_SHARE -> share()
                MENU_BROWSER -> currentUrl()?.let { openExternally(it) }
                MENU_PIN -> pinShortcut()
                MENU_SWITCH -> switchApps()
                MENU_CLOSE -> closeScreen()
            }
            true
        }
        menu.show()
    }

    private fun currentUrl(): String? =
        entry?.webView?.url?.takeIf { AppScreenPolicy.isLaunchable(it) }

    private fun share() {
        val url = currentUrl() ?: return
        val send = Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_TEXT, url)
            entry?.webView?.title?.let { putExtra(Intent.EXTRA_SUBJECT, it) }
        }
        startActivity(Intent.createChooser(send, null))
    }

    private fun pinShortcut() {
        val requested = try {
            AppScreenShortcuts.requestPin(this, request)
        } catch (_: Exception) {
            false
        }
        Toast.makeText(
            this,
            if (requested) "ホーム画面への追加を確認してください" else "このホームアプリは追加に対応していません",
            Toast.LENGTH_SHORT,
        ).show()
    }

    private fun confirmExternal(url: String, insecureHttp: Boolean = false, download: Boolean = false) {
        if (externalDialog?.isShowing == true || isFinishing) return
        val decision = AppScreenPolicy.decide(url, mainFrame = true)
        if (download && decision.kind != NavigationKind.IN_APP && !decision.insecureHttp) return
        val host = AppScreenPolicy.displayHost(entry?.webView?.url).ifEmpty { "このページ" }
        val message = when {
            download -> "$host のファイルをブラウザで開きますか？\n\n$url"
            insecureHttp -> "このページは https ではないため、Takosumi の中では開けません。ブラウザで開きますか？\n\n$url"
            else -> "$host が外部のアプリを開こうとしています。開きますか？\n\n$url"
        }
        externalDialog = AlertDialog.Builder(this)
            .setTitle("外部で開く")
            .setMessage(message)
            .setNegativeButton("キャンセル", null)
            .setPositiveButton("開く") { _, _ -> openExternally(url) }
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
        // As browsers do: only browsable activities, no explicit target, no
        // URI permission grants carried over from the page.
        intent.addCategory(Intent.CATEGORY_BROWSABLE)
        intent.component = null
        intent.selector = null
        intent.clipData = null
        intent.flags = Intent.FLAG_ACTIVITY_NEW_TASK
        try {
            startActivity(intent)
        } catch (_: ActivityNotFoundException) {
            val fallback = intent.getStringExtra("browser_fallback_url")
            if (fallback != null && AppScreenPolicy.isLaunchable(fallback)) {
                entry?.webView?.loadUrl(fallback)
            } else {
                Toast.makeText(this, "開けるアプリがありません", Toast.LENGTH_SHORT).show()
            }
        } catch (_: SecurityException) {
            Toast.makeText(this, "開けるアプリがありません", Toast.LENGTH_SHORT).show()
        }
    }

    private inner class ScreenClient : WebViewClient() {
        override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
            val url = request.url.toString()
            val decision = AppScreenPolicy.decide(url, request.isForMainFrame)
            return when (decision.kind) {
                NavigationKind.IN_APP -> false
                NavigationKind.BLOCK -> true
                NavigationKind.EXTERNAL_CONFIRM -> {
                    if (request.isForMainFrame && view === entry?.webView) {
                        confirmExternal(url, insecureHttp = decision.insecureHttp)
                    }
                    true
                }
            }
        }

        override fun onPageStarted(view: WebView, url: String?, favicon: android.graphics.Bitmap?) {
            if (view !== entry?.webView) return
            progress.visibility = View.VISIBLE
            updateBar(url)
        }

        override fun onPageFinished(view: WebView, url: String?) {
            if (view !== entry?.webView) return
            progress.visibility = View.GONE
            rememberUrl(url)
            updateBar(url)
        }

        override fun doUpdateVisitedHistory(view: WebView, url: String?, isReload: Boolean) {
            if (view !== entry?.webView) return
            rememberUrl(url)
            updateBar(url)
        }

        override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: SslError) {
            handler.cancel()
        }

        override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
            val current = entry
            if (current != null && view === current.webView) {
                unbind(current)
                entry = null
                AppScreenRegistry.remove(current.key)
                if (request.popup) {
                    finish()
                } else if (resumed) {
                    show(target = lastKnownUrl.ifEmpty { request.url }, keepIfLaunchedWith = null)
                } else {
                    evicted = true
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
        const val MENU_SHARE = 1
        const val MENU_BROWSER = 2
        const val MENU_PIN = 3
        const val MENU_SWITCH = 4
        const val MENU_CLOSE = 5
        val BAR_COLOR = Color.parseColor("#151517")
        val FOREGROUND = Color.parseColor("#F5F5F6")
    }
}

/** Client for webviews no activity shows: keeps the policy, asks nothing. */
internal object DetachedScreenClient : WebViewClient() {
    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean =
        AppScreenPolicy.decide(request.url.toString(), request.isForMainFrame).kind !=
            NavigationKind.IN_APP

    override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: SslError) {
        handler.cancel()
    }
}
