package com.takosumi.appscreen

import android.webkit.WebView
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature

/** A profile is selected before settings, clients, scripts or any navigation. */
internal object AppScreenProfiles {
    const val SHARED = "shared"
    private val isolated = Regex("[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}")

    fun isValid(profile: String): Boolean = profile == SHARED || isolated.matches(profile)

    fun isSupported(profile: String): Boolean =
        isValid(profile) && (profile == SHARED || WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE))

    fun select(webView: WebView, profile: String) {
        require(isValid(profile)) { "Invalid Web profile." }
        check(!AppScreenProfileData.isClearing(profile)) { "この接続先の保存データを消去しています。" }
        check(isSupported(profile)) { "この端末のWeb表示は、分離されたログイン領域に対応していません。" }
        if (profile != SHARED) WebViewCompat.setProfile(webView, "takosumi-web-$profile")
    }

    fun canClear(profile: String, key: String): Boolean {
        if (profile == SHARED || !isValid(profile) || key.toByteArray(Charsets.UTF_8).size > 512) return false
        val prefix = "web-home:$profile:"
        if (!key.startsWith(prefix)) return false
        val origin = key.removePrefix(prefix)
        return ScreenRequest.isWebHomeUrl("$origin/") && ScreenRequest.webHomeKey("$origin/", profile) == key
    }
}
