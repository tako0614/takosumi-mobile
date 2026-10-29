package com.takosumi.appscreen

import android.net.Uri

/**
 * Mirror of `src/url-policy.ts`. The shared table is
 * `navigation-policy-cases.json`; keep the three implementations equal.
 */
internal enum class NavigationKind { IN_APP, EXTERNAL_CONFIRM, BLOCK }

internal data class NavigationDecision(
    val kind: NavigationKind,
    val scheme: String,
    val insecureHttp: Boolean = false,
)

internal object AppScreenPolicy {
    private const val SHELL_SCHEME = "takosumi"
    private val inScreenAbout = setOf("about:blank", "about:srcdoc")
    private val localContentSchemes = setOf(
        "about", "blob", "chrome", "chrome-extension", "content", "data", "file",
        "filesystem", "javascript", "view-source", "ws", "wss",
    )
    private val subframeWebSchemes = setOf("about", "blob", "data", "http", "https", "javascript")

    fun decide(rawUrl: String, mainFrame: Boolean): NavigationDecision {
        val trimmed = rawUrl.trim()
        val uri = try {
            Uri.parse(trimmed)
        } catch (_: Exception) {
            return block("")
        }
        val scheme = uri.scheme?.lowercase() ?: return block("")
        if (!scheme.matches(Regex("[a-z][a-z0-9+.-]*"))) return block("")

        if (!mainFrame) {
            return if (scheme in subframeWebSchemes) inApp(scheme) else block(scheme)
        }
        if (scheme == "https") {
            if (!uri.userInfo.isNullOrEmpty()) return block(scheme)
            return if (uri.host.isNullOrEmpty()) block(scheme) else inApp(scheme)
        }
        if (scheme == "about" && trimmed.lowercase() in inScreenAbout) return inApp(scheme)
        if (scheme == SHELL_SCHEME || scheme in localContentSchemes) return block(scheme)
        if (scheme == "http") {
            if (!uri.userInfo.isNullOrEmpty()) return block(scheme)
            return NavigationDecision(NavigationKind.EXTERNAL_CONFIRM, scheme, insecureHttp = true)
        }
        return NavigationDecision(NavigationKind.EXTERNAL_CONFIRM, scheme)
    }

    /** https with a host and no user info: the only URLs a screen opens with. */
    fun isLaunchable(rawUrl: String): Boolean {
        val uri = try {
            Uri.parse(rawUrl.trim())
        } catch (_: Exception) {
            return false
        }
        return uri.scheme?.lowercase() == "https" &&
            decide(rawUrl, mainFrame = true).kind == NavigationKind.IN_APP
    }

    /** ASCII host (with port) for the top bar; empty when there is none. */
    fun displayHost(rawUrl: String?): String {
        if (rawUrl.isNullOrBlank()) return ""
        val uri = try {
            Uri.parse(rawUrl)
        } catch (_: Exception) {
            return ""
        }
        val host = uri.host ?: return if (rawUrl.startsWith("about:")) rawUrl else ""
        return if (uri.port > 0) "$host:${uri.port}" else host
    }

    fun origin(rawUrl: String): String? {
        val uri = try {
            Uri.parse(rawUrl)
        } catch (_: Exception) {
            return null
        }
        val scheme = uri.scheme?.lowercase() ?: return null
        val host = uri.host?.lowercase() ?: return null
        return if (uri.port > 0) "$scheme://$host:${uri.port}" else "$scheme://$host"
    }

    private fun inApp(scheme: String) = NavigationDecision(NavigationKind.IN_APP, scheme)

    private fun block(scheme: String) = NavigationDecision(NavigationKind.BLOCK, scheme)
}
