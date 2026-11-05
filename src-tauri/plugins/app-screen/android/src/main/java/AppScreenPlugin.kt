package com.takosumi.appscreen

import android.app.Activity
import android.graphics.Color
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.lang.ref.WeakReference

private const val SHARED_PROFILE = "shared"
private const val KEY_MAX_LENGTH = 512
private const val TITLE_MAX_LENGTH = 128

@InvokeArg
class BannerClientArgs {
    var name: String = ""
    var verified: Boolean = false
    var `package`: String? = null
    var appLink: String? = null
    var universalLink: String? = null
    var open: String? = null
}

@InvokeArg
class OpenScreenArgs {
    var key: String = ""
    var url: String = ""
    var title: String = ""
    var iconPng: String? = null
    var profile: String = ""
    var webHome: Boolean = false
    var accountsOrigins: Array<String>? = null
    var scope: String? = null
    var resume: Boolean = false
    var client: BannerClientArgs? = null
}

@InvokeArg
class OpenClientArgs {
    var platform: String = ""
    var `package`: String? = null
    var appLink: String? = null
    var universalLink: String? = null
    var open: String? = null
}

@InvokeArg
class BrowserViewArgs {
    var url: String = ""
    var toolbarColor: String? = null
}

@InvokeArg
class ClearProfileArgs {
    var profile: String = ""
    var key: String = ""
}

/**
 * Shell-facing commands; the screens themselves live in [AppScreenActivity].
 * Only the shell's `main` window has the capability for them. App screens are
 * plain WebViews without any bridge, so pages there cannot call these.
 */
@TauriPlugin
class AppScreenPlugin(private val activity: Activity) : Plugin(activity) {
    init {
        current = WeakReference(this)
    }

    @Command
    fun capabilities(invoke: Invoke) {
        invoke.resolve(
            JSObject().apply {
                put("pinShortcut", AppScreenShortcuts.isPinSupported(activity))
                put("clearProfile", AppScreenProfileData.isSupported())
            },
        )
    }

    @Command
    fun clearProfile(invoke: Invoke) {
        val args = invoke.parseArgs(ClearProfileArgs::class.java)
        activity.runOnUiThread {
            AppScreenProfileData.clear(activity.applicationContext, args.profile, args.key) { error ->
                if (error == null) invoke.resolve(JSObject().apply { put("cleared", true) })
                else invoke.reject(error)
            }
        }
    }

    @Command
    fun open(invoke: Invoke) {
        val request = screenRequestOrReject(invoke) ?: return
        if (AppScreenProfileData.isClearing(request.profile)) {
            invoke.reject("この接続先の保存データを消去しています。")
            return
        }
        try {
            AppScreenLaunch.remember(activity, request)
            activity.startActivity(AppScreenLaunch.documentIntent(activity, request))
            invoke.resolve()
        } catch (_: Exception) {
            invoke.reject("The app screen could not be opened.")
        }
    }

    @Command
    fun pinShortcut(invoke: Invoke) {
        val request = screenRequestOrReject(invoke) ?: return
        val requested = try {
            AppScreenShortcuts.requestPin(activity, request)
        } catch (_: Exception) {
            false
        }
        invoke.resolve(JSObject().apply { put("requested", requested) })
    }

    /** Hand a screen to its declared native app: `opened` or `not_installed`. */
    @Command
    fun openClient(invoke: Invoke) {
        val args = invoke.parseArgs(OpenClientArgs::class.java)
        val packageName = args.`package`
        if (args.platform != "android" || args.universalLink != null || packageName == null ||
            !AppScreenRules.isAndroidPackage(packageName) ||
            (args.appLink == null && args.open == null) ||
            (args.appLink != null && !AppScreenRules.isHttpsLink(args.appLink)) ||
            (args.open != null && !AppScreenRules.isCustomSchemeLink(args.open))
        ) {
            invoke.reject("This native app cannot be opened.")
            return
        }
        val result = AppClientLauncher.open(activity, packageName, args.appLink, args.open)
        invoke.resolve(JSObject().apply { put("result", result) })
    }

    /** Open an https page in a Custom Tab (or the normal browser without one). */
    @Command
    fun openBrowserView(invoke: Invoke) {
        val args = invoke.parseArgs(BrowserViewArgs::class.java)
        if (!AppScreenPolicy.isLaunchable(args.url) ||
            (args.toolbarColor != null && !AppScreenRules.isHexColor(args.toolbarColor))
        ) {
            invoke.reject("Only https URLs can be opened in the browser view.")
            return
        }
        val color = args.toolbarColor?.let { Color.parseColor(it) } ?: BROWSER_VIEW_COLOR
        try {
            val result = BrowserView.open(activity, args.url, color)
            invoke.resolve(JSObject().apply { put("result", result) })
        } catch (_: Exception) {
            invoke.reject("No browser can open this page.")
        }
    }

    /** Open an https page in the phone's browser, never in an app claiming it. */
    @Command
    fun openWeb(invoke: Invoke) {
        val args = invoke.parseArgs(BrowserViewArgs::class.java)
        if (!AppScreenPolicy.isLaunchable(args.url)) {
            invoke.reject("Only https URLs can be opened in the browser.")
            return
        }
        try {
            BrowserView.openInBrowser(activity, args.url)
            invoke.resolve(JSObject().apply { put("result", BrowserView.BROWSER) })
        } catch (_: Exception) {
            invoke.reject("No browser can open this page.")
        }
    }

    /** The action an app screen's bar left for the shell, once. */
    @Command
    fun takePendingAction(invoke: Invoke) {
        val action = AppScreenActions.take()
        invoke.resolve(
            JSObject().apply {
                if (action != null) put("action", JSObject(action.toJson().toString()))
            },
        )
    }

    private fun screenRequestOrReject(invoke: Invoke): ScreenRequest? {
        val args = invoke.parseArgs(OpenScreenArgs::class.java)
        if (!AppScreenProfiles.isValid(args.profile) || (!args.webHome && args.profile != SHARED_PROFILE)) {
            invoke.reject("Only Web homes may select an isolated browsing profile.")
            return null
        }
        if (!AppScreenProfiles.isSupported(args.profile)) {
            invoke.reject("この端末のWeb表示は、分離されたログイン領域に対応していません。")
            return null
        }
        if (args.key.isBlank() || args.key.length > KEY_MAX_LENGTH) {
            invoke.reject("App screen key is invalid.")
            return null
        }
        val liveProfile = AppScreenRegistry.get(args.key)?.profile
        if (liveProfile != null && liveProfile != args.profile) {
            invoke.reject("A live screen cannot change its browsing profile.")
            return null
        }
        if (!AppScreenPolicy.isLaunchable(args.url)) {
            invoke.reject("Only https URLs can be opened in an app screen.")
            return null
        }
        if (args.webHome && (!ScreenRequest.isWebHomeUrl(args.url) || args.client != null ||
                args.key != ScreenRequest.webHomeKey(args.url, args.profile))) {
            invoke.reject("A Web home must be an https site root without a native client.")
            return null
        }
        val accountsOrigins = ScreenRequest.checkedAccountsOrigins(args.accountsOrigins?.toList()) ?: run {
            invoke.reject("The Takosumi origins must be at most 32 https origins.")
            return null
        }
        val scope = args.scope?.let {
            ScreenRequest.checkedScope(it) ?: run {
                invoke.reject("App screen scope is invalid.")
                return null
            }
        }
        val client = args.client?.let {
            if (it.universalLink != null) null
            else BannerClient.of(it.name, it.verified, it.`package`, it.appLink, it.open)
        }
        if (args.client != null && client == null) {
            invoke.reject("The native app offered by the bar is invalid.")
            return null
        }
        val title = args.title.trim().take(TITLE_MAX_LENGTH)
            .ifEmpty { AppScreenPolicy.displayHost(args.url) }
        try {
            AppScreenIcons.store(activity, args.key, args.iconPng)
        } catch (_: Exception) {
            // The screen opens without its icon.
        }
        return ScreenRequest(
            key = args.key,
            url = args.url,
            title = title,
            accountsOrigins = accountsOrigins,
            scope = scope,
            resume = args.resume,
            webHome = args.webHome,
            profile = args.profile,
            client = client,
        )
    }

    companion object {
        private val BROWSER_VIEW_COLOR = Color.parseColor("#151517")

        @Volatile
        private var current: WeakReference<AppScreenPlugin>? = null

        /** Tell the shell an action waits (`actionPending`), when it is running. */
        fun notifyActionPending() {
            try {
                current?.get()?.trigger("actionPending", JSObject())
            } catch (_: Exception) {
                // The shell takes the action when it next starts.
            }
        }
    }
}
