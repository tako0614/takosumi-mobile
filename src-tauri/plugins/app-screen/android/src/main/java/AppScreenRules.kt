package com.takosumi.appscreen

import java.net.URLDecoder

/**
 * Rules without Android types, so `src/test` runs them on the JVM. They mirror
 * `src/url-policy.ts` and `src/app-clients.ts`; the shared fixtures are
 * `navigation-policy-cases.json` (origins) and `resume-url-cases.json`.
 *
 * URLs reaching these functions come from the WebView or the shell already
 * canonical (lowercase scheme and host, punycode, default port dropped), so
 * origins are read from the text rather than with a second URL parser.
 */
/**
 * What one app screen may show: its app's origin in full, and on the Takosumi
 * origins (the instance's Accounts and the instance itself) only
 * [AppScreenRules.ACCOUNTS_SIGN_IN_PATHS].
 */
internal data class ScreenOrigins(val appOrigin: String?, val accountsOrigins: Set<String>)

internal object AppScreenRules {
    /** Hosts the Tauri shell serves its own UI and IPC from (and any `*.localhost`). */
    val SHELL_HOSTS = setOf("tauri.localhost", "ipc.localhost", "asset.localhost")

    /**
     * The pages an app's Takosumi sign-in passes through on the Takosumi
     * origins, compared exactly on the percent-encoded path.
     */
    val ACCOUNTS_SIGN_IN_PATHS = setOf(
        "/oauth/authorize", "/oauth/upstream/authorize", "/oauth/upstream/callback",
        "/sign-in", "/sign-in/callback",
    )

    /** Takosumi's `hasCredentialQueryParams` names plus OAuth/SAML callback values. */
    val CREDENTIAL_QUERY_NAMES = setOf(
        "accesskey", "accesstoken", "apikey", "auth", "authorization", "bearertoken",
        "clientsecret", "cookie", "credential", "credentials", "credentialvalue", "jwt",
        "passwd", "password", "privatekey", "proxyauthorization", "refreshtoken", "secret",
        "session", "sessionid", "sessiontoken", "sig", "signature", "signingkey", "token",
        "xamzcredential", "xamzsecuritytoken", "xamzsignature", "xgoogcredential",
        "xgoogsignature", "xapikey", "code", "idtoken", "oauthtoken", "oauthverifier",
        "samlresponse", "ticket",
    )

    /** Mirror of takosumi `INTERFACE_CLIENT_BLOCKED_SCHEMES`. */
    val BLOCKED_CLIENT_SCHEMES = setOf(
        "http", "https", "javascript", "vbscript", "data", "file", "filesystem", "blob",
        "about", "content", "intent", "android-app", "takosumi", "tel", "callto", "sms",
        "smsto", "mms", "mmsto", "mailto", "facetime", "facetime-audio", "facetime-prompt",
        "facetime-audio-prompt", "itms", "itmss", "itms-apps", "itms-appss", "itms-services",
        "itms-beta", "market", "app-settings", "app-prefs", "prefs", "shortcuts",
        "x-web-search", "x-safari-http", "x-safari-https", "googlechrome", "googlechromes",
        "googlechrome-x-callback", "firefox", "firefox-focus", "focus", "microsoft-edge",
        "microsoft-edge-http", "microsoft-edge-https", "opera", "opera-http", "opera-https",
        "touch-http", "touch-https", "brave", "duckduckgo", "ddgquicklink", "samsunginternet",
        "yandexbrowser-open-url", "firefox-klar", "onionhttp", "onionhttps", "ws", "wss", "ftp",
    )

    const val RESUME_URL_MAX_LENGTH = 4_096
    private const val CLIENT_LINK_MAX_LENGTH = 2_048
    private const val CLIENT_IDENTIFIER_MAX_LENGTH = 155
    private const val OWN_APP_ID = "com.takosumi"
    private val AUTHORITY = Regex("^([A-Za-z][A-Za-z0-9+.-]*)://([^/?#\\\\]*)")
    private val CUSTOM_SCHEME = Regex("^[a-z][a-z0-9+.-]{1,31}$")
    private val PACKAGE_SEGMENT = Regex("^[A-Za-z][A-Za-z0-9_]*$")
    private val HEX_COLOR = Regex("^#[0-9A-Fa-f]{6}$")

    /**
     * `scheme://host[:port]` as `URL.origin` writes it: lowercase, default
     * port dropped. Null without an authority or with user info.
     */
    fun originOf(rawUrl: String): String? {
        val match = AUTHORITY.find(rawUrl.trim()) ?: return null
        val scheme = match.groupValues[1].lowercase()
        val authority = match.groupValues[2]
        if (authority.isEmpty() || authority.contains('@')) return null
        val host: String
        val port: String?
        if (authority.startsWith("[")) {
            val end = authority.indexOf(']')
            if (end < 0) return null
            host = authority.substring(0, end + 1)
            val rest = authority.substring(end + 1)
            if (rest.isNotEmpty() && !rest.startsWith(":")) return null
            port = rest.removePrefix(":").ifEmpty { null }
        } else {
            val colon = authority.lastIndexOf(':')
            host = if (colon >= 0) authority.substring(0, colon) else authority
            port = if (colon >= 0) authority.substring(colon + 1).ifEmpty { null } else null
        }
        if (host.isEmpty() || (port != null && !port.all { it in '0'..'9' })) return null
        val defaultPort = when (scheme) {
            "https" -> 443
            "http" -> 80
            else -> -1
        }
        val number = port?.toIntOrNull()
        if (port != null && (number == null || number > 65_535)) return null
        val shownPort = if (number == null || number == defaultPort) "" else ":$number"
        return "$scheme://${host.lowercase()}$shownPort"
    }

    /** An `@` in the authority, even with empty user info (`https://:@host/`). */
    fun hasRawUserInfo(rawUrl: String): Boolean =
        AUTHORITY.find(rawUrl.trim())?.groupValues?.get(2)?.contains('@') == true

    fun isShellHost(host: String?): Boolean {
        val name = host?.lowercase()?.trimEnd('.') ?: return false
        return name in SHELL_HOSTS || name.endsWith(".localhost")
    }

    /**
     * An https main-frame page of a screen: IN_APP on the app's origin and on
     * the sign-in paths of the Takosumi origins, BROWSER_VIEW otherwise (the
     * dashboard, account pages, `/api/v1`, other sites).
     */
    fun mainFrameDecision(origin: String?, encodedPath: String?, origins: ScreenOrigins): NavigationKind {
        if (origin == null) return NavigationKind.BROWSER_VIEW
        if (origin in origins.accountsOrigins) {
            return if (encodedPath in ACCOUNTS_SIGN_IN_PATHS) NavigationKind.IN_APP else NavigationKind.BROWSER_VIEW
        }
        return if (origin == origins.appOrigin) NavigationKind.IN_APP else NavigationKind.BROWSER_VIEW
    }

    /** A subframe on a Takosumi origin outside sign-in is refused. */
    fun isTakosumiPageOutsideSignIn(origin: String?, encodedPath: String?, origins: ScreenOrigins): Boolean =
        origin != null && origin in origins.accountsOrigins && encodedPath !in ACCOUNTS_SIGN_IN_PATHS

    /**
     * [rawUrl] as it may be shared or handed to the browser: credential-named
     * query parameters with a value, undecodable pairs, empty pairs and a
     * fragment holding parameters are removed; the rest is kept as written.
     */
    fun shareable(rawUrl: String): String {
        val hash = rawUrl.indexOf('#')
        val beforeHash = if (hash < 0) rawUrl else rawUrl.substring(0, hash)
        val fragment = if (hash < 0) null else rawUrl.substring(hash + 1)
        val question = beforeHash.indexOf('?')
        val base = if (question < 0) beforeHash else beforeHash.substring(0, question)
        val kept = (if (question < 0) "" else beforeHash.substring(question + 1))
            .split('&')
            .filter { pair ->
                if (pair.isEmpty()) return@filter false
                val name = decode(pair.substringBefore('=')) ?: return@filter false
                val value = decode(if (pair.contains('=')) pair.substringAfter('=') else "") ?: return@filter false
                !isCredential(name, value)
            }
        return base +
            (if (kept.isEmpty()) "" else "?" + kept.joinToString("&")) +
            (if (fragment != null && !fragment.contains('=')) "#$fragment" else "")
    }

    private fun isCredential(name: String, value: String): Boolean =
        value.isNotBlank() && name.lowercase().filter { it in 'a'..'z' || it in '0'..'9' } in CREDENTIAL_QUERY_NAMES

    /** Exactly an https origin (`https://host[:port]`), not a shell host. */
    fun isHttpsOrigin(raw: String): Boolean {
        val origin = originOf(raw) ?: return false
        return origin == raw && raw.startsWith("https://") && !isShellHost(hostOf(origin))
    }

    /**
     * A page the screen may reopen at: https on the app's own origin, no user
     * info, no fragment, no credential-named query parameter with a value.
     */
    fun isRememberable(rawUrl: String, appOrigin: String): Boolean {
        if (rawUrl.length > RESUME_URL_MAX_LENGTH || rawUrl.contains('#')) return false
        if (!rawUrl.startsWith("https://", ignoreCase = true)) return false
        val origin = originOf(rawUrl) ?: return false
        if (origin != appOrigin || isShellHost(hostOf(origin))) return false
        if (rawUrl.any { it.isWhitespace() }) return false
        val query = rawUrl.substringAfter('?', "")
        if (query.isEmpty()) return true
        for (pair in query.split('&')) {
            if (pair.isEmpty()) continue
            val name = decode(pair.substringBefore('=')) ?: return false
            val value = decode(if (pair.contains('=')) pair.substringAfter('=') else "") ?: return false
            if (isCredential(name, value)) return false
        }
        return true
    }

    /** An Android applicationId, never Takosumi Mobile's own. */
    fun isAndroidPackage(raw: String?): Boolean {
        if (raw.isNullOrEmpty() || raw.length > CLIENT_IDENTIFIER_MAX_LENGTH) return false
        if (raw.equals(OWN_APP_ID, ignoreCase = true)) return false
        val segments = raw.split('.')
        return segments.size >= 2 && segments.all { PACKAGE_SEGMENT.matches(it) }
    }

    /** A publisher-domain https link (App Link). */
    fun isHttpsLink(raw: String?): Boolean {
        if (raw.isNullOrEmpty() || raw.length > CLIENT_LINK_MAX_LENGTH) return false
        if (!raw.startsWith("https://") || raw.any { it.isWhitespace() || it.isISOControl() }) {
            return false
        }
        val origin = originOf(raw) ?: return false
        return !isShellHost(hostOf(origin))
    }

    /** A custom-scheme link whose scheme Takosumi does not refuse. */
    fun isCustomSchemeLink(raw: String?): Boolean {
        if (raw.isNullOrEmpty() || raw.length > CLIENT_LINK_MAX_LENGTH) return false
        if (raw.any { it.isWhitespace() || it.isISOControl() }) return false
        val scheme = raw.substringBefore(':', "")
        return CUSTOM_SCHEME.matches(scheme) && scheme !in BLOCKED_CLIENT_SCHEMES
    }

    /** A declared app's name: short, no control, bidi or zero-width characters. */
    fun isClientName(raw: String?): Boolean {
        val name = raw?.trim() ?: return false
        if (name.isEmpty() || name.codePointCount(0, name.length) > 64) return false
        return name.none { character ->
            character.isISOControl() ||
                character in '\u200B'..'\u200F' ||
                character in '\u202A'..'\u202E' ||
                character in '\u2066'..'\u2069' ||
                character == '\uFEFF'
        }
    }

    fun isHexColor(raw: String?): Boolean = raw != null && HEX_COLOR.matches(raw)

    private fun hostOf(origin: String): String =
        origin.substringAfter("://").let { authority ->
            if (authority.startsWith("[")) authority.substringBefore(']') + "]"
            else authority.substringBefore(':')
        }

    private fun decode(part: String): String? = try {
        URLDecoder.decode(part, "UTF-8")
    } catch (_: IllegalArgumentException) {
        null
    }
}
