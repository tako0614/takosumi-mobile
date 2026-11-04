package com.takosumi.appscreen

import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import org.json.JSONObject
import org.json.JSONTokener
import org.junit.After
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Actual Android WebView cookies/storage, using synthetic https origins and no network or login. */
@RunWith(AndroidJUnit4::class)
class ProfileIsolationTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val keys = mutableListOf<String>()
    private val site = "https://sumi.example"
    private val otherSite = "https://other.example"

    private fun <T> main(block: () -> T): T {
        var result: Result<T>? = null
        instrumentation.runOnMainSync { result = runCatching(block) }
        return result!!.getOrThrow()
    }

    private fun open(profile: String, origin: String = site): AppScreenRegistry.Entry {
        val key = "test:${UUID.randomUUID()}"
        val entry = main {
            AppScreenRegistry.obtain(instrumentation.targetContext, key, profile = profile)
        }
        keys.add(key)
        load(entry, origin)
        return entry
    }

    private fun load(entry: AppScreenRegistry.Entry, origin: String) {
        val loaded = CountDownLatch(1)
        main {
            entry.webView.webViewClient = object : WebViewClient() {
                override fun onPageFinished(view: WebView, url: String) { loaded.countDown() }
            }
            entry.webView.loadDataWithBaseURL("$origin/", "<!doctype html><title>Profile fixture</title>", "text/html", "utf-8", null)
        }
        assertTrue("Fixture document did not load", loaded.await(20, TimeUnit.SECONDS))
    }

    private fun read(entry: AppScreenRegistry.Entry, write: String? = null): JSONObject {
        val done = CountDownLatch(1)
        var result: String? = null
        val mutation = write?.let {
            "document.cookie='account=$it; Path=/; SameSite=Lax; Secure'; localStorage.setItem('account','$it');"
        } ?: ""
        main {
            entry.webView.evaluateJavascript(mutation + "JSON.stringify({origin:location.origin,cookie:document.cookie,account:localStorage.getItem('account'),ipc:typeof window.__TAURI_INTERNALS__})") {
                result = it
                done.countDown()
            }
        }
        assertTrue("JavaScript did not complete", done.await(20, TimeUnit.SECONDS))
        return JSONObject(JSONTokener(requireNotNull(result)).nextValue() as String)
    }

    private fun account(entry: AppScreenRegistry.Entry, value: String?, origin: String = site) {
        val actual = read(entry)
        assertEquals(origin, actual.getString("origin"))
        assertEquals("undefined", actual.getString("ipc"))
        if (value == null) {
            assertTrue(actual.isNull("account"))
            assertFalse(actual.getString("cookie").contains("account="))
        } else {
            assertEquals(value, actual.getString("account"))
            assertTrue(actual.getString("cookie").contains("account=$value"))
        }
    }

    @After
    fun closeOwnViews() { main { keys.forEach { AppScreenRegistry.remove(it) } } }

    @Test
    fun twoProfilesTwoSitesPopupAndRecreatedViewsKeepIndependentState() {
        assertTrue("This engine must support profile isolation; a skip is not evidence.",
            main { WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE) })
        val first = UUID.randomUUID().toString()
        val second = UUID.randomUUID().toString()
        val old = open("shared")
        read(old, "legacy")
        val work = open(first)
        account(work, null)
        read(work, "work")
        val personal = open(second)
        account(personal, null)
        read(personal, "personal")
        val recreated = open(first)
        account(recreated, "work")
        load(recreated, otherSite)
        account(recreated, null, otherSite)
        read(recreated, "other")
        load(recreated, site)
        account(recreated, "work")
        val popupKey = "popup:${UUID.randomUUID()}"
        val popup = main {
            AppScreenRegistry.obtainPopup(instrumentation.targetContext, popupKey, recreated)
        }
        keys.add(popupKey)
        load(popup, site)
        account(popup, "work")
        assertEquals(first, popup.profile)
        assertEquals("takosumi-web-$first", main { WebViewCompat.getProfile(popup.webView).name })
        account(open(second), "personal")
        account(open("shared"), "legacy")
    }

    @Test
    fun clearingAnIdleProfileWaitsForCompletionAndPreservesOtherProfiles() {
        assertTrue("This engine must qualify profile-data deletion without a skip.", main { AppScreenProfileData.isSupported() })
        val first = UUID.randomUUID().toString()
        val second = UUID.randomUUID().toString()
        val key = "web-home:$first:$site"
        read(open("shared"), "legacy-clear-control")
        read(open(second), "personal-clear-control")
        val work = open(first)
        read(work, "work-clear-control")
        load(work, otherSite)
        read(work, "other-clear-control")
        val request = ScreenRequest(key, "$site/", "Work", profile = first, webHome = true, scope = key, resume = true)
        main {
            AppScreenResume.record(instrumentation.targetContext, request, "$otherSite/editor")
            AppScreenLaunch.remember(instrumentation.targetContext, request)
            var refused: String? = null
            AppScreenProfileData.clear(instrumentation.targetContext, first, key) { refused = it }
            assertTrue(requireNotNull(refused).contains("すべて閉じて"))
            assertFalse(AppScreenProfileData.isClearing(first))
        }
        account(work, "other-clear-control", otherSite)
        main { keys.forEach { AppScreenRegistry.remove(it) } }
        val finished = CountDownLatch(1)
        var failure: String? = null
        main {
            AppScreenProfileData.clear(instrumentation.targetContext, first, key) {
                failure = it
                finished.countDown()
            }
            assertTrue(AppScreenProfileData.isClearing(first))
            try {
                AppScreenRegistry.obtain(instrumentation.targetContext, "test:pending:$first", profile = first)
                fail("A clearing profile was reopened")
            } catch (_: IllegalStateException) { }
        }
        assertTrue("Native profile deletion did not complete", finished.await(20, TimeUnit.SECONDS))
        assertNull(failure)
        assertFalse(main { AppScreenProfileData.isClearing(first) })
        assertNull(main { AppScreenResume.recall(instrumentation.targetContext, request) })
        val cleared = open(first)
        account(cleared, null)
        load(cleared, otherSite)
        account(cleared, null, otherSite)
        account(open(second), "personal-clear-control")
        account(open("shared"), "legacy-clear-control")
    }

    @Test
    fun liveScreenCannotBeRetargetedToAnotherProfilesCookieJar() {
        assertTrue(main { WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE) })
        val first = UUID.randomUUID().toString()
        val entry = open(first)
        read(entry, "original")
        try {
            main { AppScreenRegistry.obtain(instrumentation.targetContext, entry.key, profile = UUID.randomUUID().toString()) }
            fail("A live view changed profile")
        } catch (_: IllegalStateException) { }
        account(entry, "original")
        assertEquals(first, entry.profile)
    }
}
