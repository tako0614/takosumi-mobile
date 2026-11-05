package com.takosumi.appscreen

import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.webkit.ProfileStore
import androidx.webkit.WebStorageCompat
import androidx.webkit.WebViewFeature
import java.util.concurrent.atomic.AtomicBoolean

/** The in-flight map is UI-thread only; preference generations are thread-safe.
 * Never uses the process's default WebStorage.
 */
internal object AppScreenProfileData {
    private const val GENERATIONS = "com.takosumi.appscreen.profile-resets.v1"
    private val clearing = mutableMapOf<String, Any>()

    fun isClearing(profile: String): Boolean = profile in clearing

    /** Opaque v1 resume hashes and OS-saved tasks predate generation zero. */
    fun generation(context: Context, profile: String): Long = if (profile == AppScreenProfiles.SHARED) 0L else
        runCatching { context.getSharedPreferences(GENERATIONS, Context.MODE_PRIVATE).getLong(profile, 0L) }
            .getOrDefault(-1L)

    fun advanceGeneration(context: Context, profile: String): Boolean {
        require(profile != AppScreenProfiles.SHARED && AppScreenProfiles.isValid(profile))
        val current = generation(context, profile)
        check(current >= 0L && current < Long.MAX_VALUE) { "Invalid profile reset generation." }
        return context.getSharedPreferences(GENERATIONS, Context.MODE_PRIVATE).edit().putLong(profile, current + 1L).commit()
    }

    fun isSupported(): Boolean = WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE) &&
        WebViewFeature.isFeatureSupported(WebViewFeature.DELETE_BROWSING_DATA)

    fun clear(context: Context, profile: String, key: String, done: (String?) -> Unit) {
        check(Looper.myLooper() == Looper.getMainLooper())
        if (!AppScreenProfiles.canClear(profile, key)) return done("この接続先の保存データは消去できません。")
        if (!isSupported()) return done("この端末のWeb表示は、接続先の保存データ消去に対応していません。")
        if (AppScreenRegistry.hasProfile(profile)) return done("この接続先の画面をすべて閉じてから消去してください。")
        if (isClearing(profile)) return done("この接続先の保存データを消去しています。")
        val operation = Any()
        clearing[profile] = operation
        val started = AtomicBoolean(false)
        val complete = {
            if (started.compareAndSet(false, true)) {
                // Preferences must acknowledge their writes, without blocking
                // the UI while the storage thread commits them.
                Thread({
                    val success = runCatching {
                        advanceGeneration(context, profile) &&
                            AppScreenResume.clearWebHome(context, profile, key) &&
                            AppScreenLaunch.clearWebHome(context, profile, key)
                    }.getOrDefault(false)
                    Handler(Looper.getMainLooper()).post {
                        if (clearing[profile] === operation) {
                            clearing.remove(profile)
                            done(if (success) null else "前の画面の記録を消去できませんでした。消去をやり直してください。")
                        }
                    }
                }, "sumi-profile-clear").start()
            }
        }
        try {
            val store = ProfileStore.getInstance().getProfile("takosumi-web-$profile")
            if (store == null) complete()
            else WebStorageCompat.deleteBrowsingData(store.webStorage) { complete() }
        } catch (_: Exception) {
            if (clearing[profile] === operation && started.compareAndSet(false, true)) {
                clearing.remove(profile)
                done("保存データを消去できませんでした。")
            }
        }
    }
}
