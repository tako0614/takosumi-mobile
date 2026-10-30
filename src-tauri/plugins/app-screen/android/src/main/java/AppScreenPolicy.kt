package com.takosumi.appscreen

import android.net.Uri

/**
 * Mirror of `src/url-policy.ts`. The shared table is
 * `navigation-policy-cases.json`; keep the three implementations equal.
 * BROWSER_VIEW: an https page the screen does not show (another site such as
 * an external sign-in, or a Takosumi page outside sign-in: the dashboard,
 * account pages, `/api/v1`) goes to the browser view instead of this WebView.
 * `AppScreenPolicyTest` runs this file against the shared table (Robolectric).
 */
internal enum class NavigationKind { IN_APP, BROWSER_VIEW, EXTERNAL_CONFIRM, BLOCK }

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

    fun decide(
        rawUrl: String,
        mainFrame: Boolean,
        origins: ScreenOrigins? = null,
    ): NavigationDecision {
        val trimmed = rawUrl.trim()
        val uri = try {
            Uri.parse(trimmed)
        } catch (_: Exception) {
            return block("")
        }
        val scheme = uri.scheme?.lowercase() ?: return block("")
        if (!scheme.matches(Regex("[a-z][a-z0-9+.-]*"))) return block("")

        val web = scheme == "http" || scheme == "https"
        // The shell's own hosts, in every frame and both web schemes.
        if (web && AppScreenRules.isShellHost(uri.host)) return block(scheme)

        if (!mainFrame) {
            if (scheme !in subframeWebSchemes) return block(scheme)
            if (web && origins != null &&
                AppScreenRules.isTakosumiPageOutsideSignIn(AppScreenRules.originOf(trimmed), uri.encodedPath, origins)
            ) {
                return block(scheme)
            }
            return inApp(scheme)
        }
        if (scheme == "https") {
            if (!uri.userInfo.isNullOrEmpty() || AppScreenRules.hasRawUserInfo(trimmed)) return block(scheme)
            if (uri.host.isNullOrEmpty()) return block(scheme)
            if (origins != null) {
                val kind = AppScreenRules.mainFrameDecision(AppScreenRules.originOf(trimmed), uri.encodedPath, origins)
                if (kind != NavigationKind.IN_APP) return NavigationDecision(kind, scheme)
            }
            return inApp(scheme)
        }
        if (scheme == "about" && trimmed.lowercase() in inScreenAbout) return inApp(scheme)
        if (scheme == SHELL_SCHEME || scheme in localContentSchemes) return block(scheme)
        if (scheme == "intent") {
            // An intent: URI opens the scheme named in its fragment; judge that.
            val target = intentTargetScheme(trimmed)
            if (target == SHELL_SCHEME || (target != null && target in localContentSchemes)) {
                return block(scheme)
            }
        }
        if (scheme == "http") {
            if (!uri.userInfo.isNullOrEmpty() || AppScreenRules.hasRawUserInfo(trimmed)) return block(scheme)
            return NavigationDecision(NavigationKind.EXTERNAL_CONFIRM, scheme, insecureHttp = true)
        }
        return NavigationDecision(NavigationKind.EXTERNAL_CONFIRM, scheme)
    }

    /** The `scheme=` of `intent://...#Intent;scheme=x;end`, lowercased. */
    fun intentTargetScheme(rawUrl: String): String? {
        // Like Intent.parseUri: the parameters follow the last '#'.
        val hash = rawUrl.lastIndexOf('#')
        if (hash < 0 || !rawUrl.startsWith("Intent;", hash + 1)) return null
        val named = rawUrl.substring(hash + "#Intent;".length)
            .split(';')
            .firstOrNull { it.startsWith("scheme=", ignoreCase = true) }
            ?.substring("scheme=".length)
            ?.trim()
            ?.lowercase()
        if (named != null) return named
        // Without scheme=, `intent:x://...` opens the data part's own scheme.
        val data = rawUrl.substring(rawUrl.indexOf(':') + 1, hash)
        return Regex("^([a-zA-Z][a-zA-Z0-9+.-]*):").find(data)?.groupValues?.get(1)?.lowercase()
    }

    /** True when a screen with [origins] shows [rawUrl] itself. */
    fun isInScreen(rawUrl: String, origins: ScreenOrigins?): Boolean =
        isLaunchable(rawUrl) && decide(rawUrl, mainFrame = true, origins).kind == NavigationKind.IN_APP

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
