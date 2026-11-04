package com.takosumi.appscreen

import android.app.Activity
import android.app.Application
import android.graphics.Bitmap
import android.os.Bundle
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Real Activity lifecycle and WebView; synthetic document, no real login. */
@RunWith(AndroidJUnit4::class)
class ProfileResetActivityTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext
    private val root = "https://sumi.example/"
    private val editor = "https://sumi.example/editor"
    private val stateUrl = "com.takosumi.appscreen.state.URL"
    private val stateGeneration = "com.takosumi.appscreen.state.GENERATION"

    private fun <T> main(block: () -> T): T {
        var result: Result<T>? = null
        instrumentation.runOnMainSync { result = runCatching(block) }
        return result!!.getOrThrow()
    }

    private fun entry(key: String): AppScreenRegistry.Entry = main { requireNotNull(AppScreenRegistry.get(key)) }

    /** Preserve the production callbacks, and await this document once. */
    private fun editorDocument(entry: AppScreenRegistry.Entry) {
        val finished = CountDownLatch(1)
        var original: WebViewClient? = null
        main {
            val delegate = entry.webView.webViewClient
            original = delegate
            entry.webView.stopLoading()
            entry.webView.webViewClient = object : WebViewClient() {
                override fun onPageStarted(view: WebView, url: String?, icon: Bitmap?) = delegate.onPageStarted(view, url, icon)
                override fun onPageCommitVisible(view: WebView, url: String?) = delegate.onPageCommitVisible(view, url)
                override fun doUpdateVisitedHistory(view: WebView, url: String?, reload: Boolean) = delegate.doUpdateVisitedHistory(view, url, reload)
                override fun onPageFinished(view: WebView, url: String?) {
                    delegate.onPageFinished(view, url)
                    if (url == editor) finished.countDown()
                }
            }
            entry.webView.loadDataWithBaseURL(editor, "<!doctype html><title>Editor fixture</title>", "text/html", "utf-8", null)
        }
        assertTrue("Editor fixture did not finish", finished.await(20, TimeUnit.SECONDS))
        main { entry.webView.webViewClient = requireNotNull(original) }
        instrumentation.waitForIdleSync()
    }

    /** Armed before the lifecycle transition; the first native start is frozen. */
    private class StartProbe {
        private val started = CountDownLatch(1)
        private var attached = false
        private var observed = false
        private var requested: String? = null

        fun attach(entry: AppScreenRegistry.Entry) {
            if (attached) return
            attached = true
            val delegate = entry.webView.webViewClient
            entry.webView.webViewClient = object : WebViewClient() {
                override fun onPageStarted(view: WebView, url: String?, icon: Bitmap?) {
                    // Restore before forwarding. Later callbacks use the real
                    // adapter and can never overwrite this first observation.
                    val first = !observed
                    if (first) {
                        observed = true
                        view.webViewClient = delegate
                        requested = url
                    }
                    delegate.onPageStarted(view, url, icon)
                    if (first) started.countDown()
                }
            }
        }

        fun awaitUrl(): String? {
            assertTrue("Restored WebView did not start a page", started.await(20, TimeUnit.SECONDS))
            return requested
        }
    }

    @Test
    fun completedResetDropsAnEvictedActivityUrlAndPreResetSavedState() {
        assertTrue("This engine must support clearing without a skip", main { AppScreenProfileData.isSupported() })
        val profile = UUID.randomUUID().toString()
        val key = "web-home:$profile:${root.dropLast(1)}"
        val request = ScreenRequest(key, root, "Work", webHome = true, profile = profile, scope = key, resume = true)
        val otherKey = "test:evict:${UUID.randomUUID()}"
        val application = context.applicationContext as Application
        val oldState = Bundle()
        var owner: AppScreenActivity? = null
        var injectOldState = false
        var injected = false
        var recreationEvicted = false
        var receivedOldState = false
        var startProbe: StartProbe? = null
        val callbacks = object : Application.ActivityLifecycleCallbacks {
            private fun observeStart(activity: Activity) {
                if (activity is AppScreenActivity && activity.intent.getStringExtra(AppScreenLaunch.EXTRA_KEY) == key) {
                    startProbe?.attach(requireNotNull(AppScreenRegistry.get(key)))
                }
            }
            // These hooks run after show creates the WebView, on the same UI
            // turn before the engine's queued navigation callbacks. The probe
            // is already armed when the lifecycle transition begins.
            override fun onActivityPostCreated(activity: Activity, state: Bundle?) = observeStart(activity)
            override fun onActivityPostStarted(activity: Activity) = observeStart(activity)
            override fun onActivityPostSaveInstanceState(activity: Activity, state: Bundle) {
                if (injectOldState && activity is AppScreenActivity && activity.intent.getStringExtra(AppScreenLaunch.EXTRA_KEY) == key) {
                    // Run after the Activity writes its own fields, so they
                    // cannot overwrite this pre-reset OS-state control.
                    state.putString(stateUrl, oldState.getString(stateUrl))
                    state.putLong(stateGeneration, oldState.getLong(stateGeneration, 0L))
                    injected = true
                }
            }
            override fun onActivityPreCreated(activity: Activity, state: Bundle?) {
                if (injectOldState && activity is AppScreenActivity && activity.intent.getStringExtra(AppScreenLaunch.EXTRA_KEY) == key) {
                    receivedOldState = state != null && state.getString(stateUrl) == editor && state.getLong(stateGeneration, -1L) == 0L
                }
            }
            override fun onActivitySaveInstanceState(activity: Activity, state: Bundle) { }
            override fun onActivityCreated(activity: Activity, state: Bundle?) { }
            override fun onActivityStarted(activity: Activity) { }
            override fun onActivityResumed(activity: Activity) { }
            override fun onActivityPaused(activity: Activity) { }
            override fun onActivityStopped(activity: Activity) { }
            override fun onActivityDestroyed(activity: Activity) {
                if (injectOldState && activity is AppScreenActivity && activity.intent.getStringExtra(AppScreenLaunch.EXTRA_KEY) == key) {
                    // Recreation normally reuses the live WebView. Reproduce
                    // OS loss of that view so it cannot hide a stale restore.
                    AppScreenRegistry.remove(key)
                    recreationEvicted = true
                }
            }
        }
        main { application.registerActivityLifecycleCallbacks(callbacks) }
        try {
            ActivityScenario.launch<AppScreenActivity>(AppScreenLaunch.documentIntent(context, request)).use { scenario ->
                scenario.onActivity { owner = it }
                editorDocument(entry(key))
                scenario.onActivity { instrumentation.callActivityOnSaveInstanceState(it, oldState) }
                assertEquals("The control must contain a real prior page", editor, oldState.getString(stateUrl))
                assertEquals(0L, oldState.getLong(stateGeneration, 0L))

                scenario.moveToState(Lifecycle.State.CREATED)
                main {
                    AppScreenRegistry.obtain(context, otherKey)
                    assertNull("The inactive WebView was not evicted", AppScreenRegistry.get(key))
                    assertFalse(AppScreenProfileData.isClearing(profile))
                }
                val complete = CountDownLatch(1)
                var error: String? = null
                main { AppScreenProfileData.clear(context, profile, key) { error = it; complete.countDown() } }
                assertTrue("Reset did not complete", complete.await(20, TimeUnit.SECONDS))
                assertNull(error)
                assertEquals(1L, AppScreenProfileData.generation(context, profile))

                val resumeProbe = StartProbe()
                main { startProbe = resumeProbe }
                scenario.moveToState(Lifecycle.State.RESUMED)
                scenario.onActivity { assertSame("This control must resume the old Activity", owner, it) }
                assertEquals("Recents restored the cleared Activity's old URL", root, resumeProbe.awaitUrl())

                val recreateProbe = StartProbe()
                main { startProbe = recreateProbe; injectOldState = true }
                scenario.recreate()
                assertTrue("The stale saved-state control did not run", injected)
                assertTrue("A live view could mask the saved-state restore", recreationEvicted)
                assertTrue("The recreated Activity did not receive the pre-reset URL and generation", receivedOldState)
                scenario.onActivity { assertNotSame(owner, it) }
                assertEquals("Recreation restored a pre-reset saved URL", root, recreateProbe.awaitUrl())
            }
        } finally {
            main {
                application.unregisterActivityLifecycleCallbacks(callbacks)
                AppScreenRegistry.remove(key)
                AppScreenRegistry.remove(otherKey)
            }
        }
    }
}
