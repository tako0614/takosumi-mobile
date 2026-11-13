package com.takosumi.appscreen

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.webkit.CookieManager
import androidx.webkit.ProfileStore
import androidx.webkit.WebStorageCompat
import androidx.webkit.WebViewFeature
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

/** The in-flight map is UI-thread only; preference generations are thread-safe.
 * Never uses the process's default WebStorage.
 */
internal object AppScreenProfileData {
    private const val GENERATIONS = "com.takosumi.appscreen.profile-resets.v1"
    private val clearing = mutableMapOf<String, Any>()
    private val cookieWorker by lazy {
        ThreadPoolExecutor(0, 1, 5L, TimeUnit.SECONDS,
            ArrayBlockingQueue<Runnable>(AppScreenCookiePersistence.MAX_PROFILES),
            { job -> Thread(job, "sumi-profile-cookies").apply { isDaemon = true } })
    }
    private var cookiePersistence: AppScreenCookiePersistence? = null

    private fun persistence(context: Context): AppScreenCookiePersistence {
        check(Looper.myLooper() == Looper.getMainLooper())
        return cookiePersistence ?: run {
            val app = context.applicationContext
            val main = Handler(Looper.getMainLooper())
            AppScreenCookiePersistence(cookieWorker, { main.post(it) },
                { check(Looper.myLooper() == Looper.getMainLooper()) },
                { profile -> generation(app, profile) },
                { owner, outcome ->
                    // No URL, cookie value, credential or exception payload.
                    Log.i("SumiProfileCookies", "profile=${owner.profile} generation=${owner.generation} result=$outcome")
                }).also { cookiePersistence = it }
        }
    }

    fun cookieOwner(context: Context, profile: String, cookies: CookieManager): AppScreenCookiePersistence.Owner {
        check(Looper.myLooper() == Looper.getMainLooper())
        val atGeneration = generation(context, profile)
        return AppScreenCookiePersistence.Owner(profile, atGeneration, cookies::flush)
    }

    fun persistCookies(context: Context, owner: AppScreenCookiePersistence.Owner) {
        persistence(context).request(owner)
    }

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
        val app = context.applicationContext
        val persistence = persistence(app)
        val operation = Any()
        clearing[profile] = operation
        val started = AtomicBoolean(false)
        val accepted = persistence.beginClear(profile, generation(app, profile)) { lease ->
            val finish = { error: String? ->
                if (clearing[profile] === operation) {
                    persistence.finishClear(lease)
                    clearing.remove(profile)
                    done(error)
                }
            }
            val complete = {
                if (started.compareAndSet(false, true)) {
                    // Use the same bounded I/O worker after the owned flush
                    // drains. Never block UI or start one thread per clear.
                    try {
                        cookieWorker.execute {
                            val success = runCatching {
                                advanceGeneration(app, profile) &&
                                    AppScreenResume.clearWebHome(app, profile, key) &&
                                    AppScreenLaunch.clearWebHome(app, profile, key)
                            }.getOrDefault(false)
                            Handler(Looper.getMainLooper()).post {
                                finish(if (success) null else "前の画面の記録を消去できませんでした。消去をやり直してください。")
                            }
                        }
                    } catch (_: RejectedExecutionException) {
                        finish("前の画面の記録を消去できませんでした。消去をやり直してください。")
                    }
                }
            }
            try {
                val store = ProfileStore.getInstance().getProfile("takosumi-web-$profile")
                if (store == null) complete()
                else WebStorageCompat.deleteBrowsingData(store.webStorage) {
                    Handler(Looper.getMainLooper()).post { complete() }
                }
            } catch (_: Exception) {
                if (started.compareAndSet(false, true)) finish("保存データを消去できませんでした。")
            }
        }
        if (!accepted && clearing[profile] === operation) {
            clearing.remove(profile)
            done("保存データを消去できませんでした。")
        }
    }
}
