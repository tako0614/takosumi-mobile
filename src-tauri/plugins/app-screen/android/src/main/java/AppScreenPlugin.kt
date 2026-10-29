package com.takosumi.appscreen

import android.app.Activity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

private const val SHARED_PROFILE = "shared"
private const val KEY_MAX_LENGTH = 512
private const val TITLE_MAX_LENGTH = 128

@InvokeArg
class OpenScreenArgs {
    var key: String = ""
    var url: String = ""
    var title: String = ""
    var iconPng: String? = null
    var profile: String = ""
}

/** Shell-facing commands; the screens themselves live in [AppScreenActivity]. */
@TauriPlugin
class AppScreenPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun capabilities(invoke: Invoke) {
        invoke.resolve(
            JSObject().apply {
                put("pinShortcut", AppScreenShortcuts.isPinSupported(activity))
            },
        )
    }

    @Command
    fun open(invoke: Invoke) {
        val request = screenRequestOrReject(invoke) ?: return
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

    private fun screenRequestOrReject(invoke: Invoke): ScreenRequest? {
        val args = invoke.parseArgs(OpenScreenArgs::class.java)
        if (args.profile != SHARED_PROFILE) {
            invoke.reject("App screens share one browsing profile.")
            return null
        }
        if (args.key.isBlank() || args.key.length > KEY_MAX_LENGTH) {
            invoke.reject("App screen key is invalid.")
            return null
        }
        if (!AppScreenPolicy.isLaunchable(args.url)) {
            invoke.reject("Only https URLs can be opened in an app screen.")
            return null
        }
        val title = args.title.trim().take(TITLE_MAX_LENGTH)
            .ifEmpty { AppScreenPolicy.displayHost(args.url) }
        try {
            AppScreenIcons.store(activity, args.key, args.iconPng)
        } catch (_: Exception) {
            // The screen opens without its icon.
        }
        return ScreenRequest(args.key, args.url, title)
    }
}
