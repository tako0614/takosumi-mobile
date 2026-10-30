package com.takosumi.push

import org.json.JSONObject
import java.net.URI
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction

/** What a Takosumi notification shows and opens. */
data class DeclarativeNotification(
    val title: String,
    val body: String?,
    /** The page to open, only when it is on one of the subscription's origins. */
    val navigate: String?,
    val lang: String?,
    val tag: String?,
)

/**
 * Apple's Declarative Web Push message, as Takosumi instances send it:
 * `{"web_push": 8030, "notification": {"title", "navigate", "body"?, "lang"?, "tag"?}}`.
 * Anything else is not a Takosumi notification and is shown as generic text.
 */
object DeclarativePush {
    const val MAGIC = 8030
    const val MAX_PLAINTEXT_BYTES = 4096
    private const val MAX_TITLE_CHARS = 120
    private const val MAX_BODY_CHARS = 1000
    private const val MAX_TAG_CHARS = 64
    private const val MAX_URL_LENGTH = 2048
    private val LANG = Regex("^[A-Za-z]{2,8}(?:-[A-Za-z0-9]{1,8}){0,7}$")
    private val WHITESPACE = Regex("\\s+")

    fun parse(plaintext: ByteArray, origins: Collection<String>): DeclarativeNotification? {
        if (plaintext.size > MAX_PLAINTEXT_BYTES) return null
        val text = utf8(plaintext) ?: return null
        val root = try {
            JSONObject(text)
        } catch (error: Exception) {
            return null
        }
        val magic = root.opt("web_push")
        if (magic !is Int || magic != MAGIC) return null
        val notification = root.opt("notification") as? JSONObject ?: return null
        val title = text(notification.opt("title"), MAX_TITLE_CHARS) ?: return null
        val navigate = (notification.opt("navigate") as? String)?.let { allowedUrl(it, origins) }
        return DeclarativeNotification(
            title = title,
            body = text(notification.opt("body"), MAX_BODY_CHARS),
            navigate = navigate,
            lang = (notification.opt("lang") as? String)?.takeIf { LANG.matches(it) },
            tag = text(notification.opt("tag"), MAX_TAG_CHARS),
        )
    }

    /**
     * `url` when it is an absolute http(s) URL without user info whose origin
     * is one of `origins` (the instance and its Accounts issuer).
     */
    fun allowedUrl(url: String, origins: Collection<String>): String? {
        if (url.length > MAX_URL_LENGTH) return null
        val parsed = try {
            URI(url)
        } catch (error: Exception) {
            return null
        }
        if (!parsed.isAbsolute || parsed.isOpaque || parsed.rawUserInfo != null) return null
        val origin = originOf(parsed) ?: return null
        return if (origin in origins) url else null
    }

    /** `scheme://host[:port]`, lowercase, the default port dropped. */
    fun originOf(uri: URI): String? {
        val scheme = uri.scheme?.lowercase() ?: return null
        if (scheme != "https" && scheme != "http") return null
        val host = uri.host?.lowercase() ?: return null
        if (host.isEmpty()) return null
        val port = uri.port
        val defaultPort = if (scheme == "https") 443 else 80
        val authority = if (host.contains(':') && !host.startsWith("[")) "[$host]" else host
        return if (port == -1 || port == defaultPort) "$scheme://$authority" else "$scheme://$authority:$port"
    }

    private fun text(value: Any?, maxChars: Int): String? {
        if (value !is String) return null
        val single = value.replace(WHITESPACE, " ").trim()
        if (single.isEmpty()) return null
        val codePoints = single.codePointCount(0, single.length)
        if (codePoints <= maxChars) return single
        val end = single.offsetByCodePoints(0, maxChars - 1)
        return single.substring(0, end).trimEnd() + "…"
    }

    private fun utf8(bytes: ByteArray): String? =
        try {
            Charsets.UTF_8.newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT)
                .decode(ByteBuffer.wrap(bytes))
                .toString()
        } catch (error: CharacterCodingException) {
            null
        }
}
