package com.takosumi.appscreen

import android.Manifest
import android.accessibilityservice.AccessibilityServiceInfo
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.os.SystemClock
import android.view.accessibility.AccessibilityNodeInfo
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.ByteArrayInputStream
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Actual WebView and both permission UIs. No camera, recording or real site. */
@RunWith(AndroidJUnit4::class)
class MediaPermissionActivityTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext
    private val root = "https://media.sumi.example/"

    private fun <T> main(block: () -> T): T {
        var result: Result<T>? = null
        instrumentation.runOnMainSync { result = runCatching(block) }
        return result!!.getOrThrow()
    }

    private fun evaluate(view: WebView, script: String): String? {
        val done = CountDownLatch(1)
        var value: String? = null
        main { view.evaluateJavascript(script) { value = it; done.countDown() } }
        assertTrue("WebView script did not return", done.await(10, TimeUnit.SECONDS))
        return value
    }

    private fun document(entry: AppScreenRegistry.Entry) {
        val url = "${root}call?fixture=${UUID.randomUUID()}"
        val done = CountDownLatch(1)
        var original: WebViewClient? = null
        main {
            val delegate = entry.webView.webViewClient
            original = delegate
            entry.webView.stopLoading()
            entry.webView.webViewClient = object : WebViewClient() {
                private var committed = false
                private var finished = false
                override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                    if (request.isForMainFrame && request.url.toString() == url) {
                        val html = """<!doctype html><title>Media fixture</title><script>
                            window.mediaResult = 'idle';
                            window.askForMicrophone = () => {
                              window.mediaResult = 'pending';
                              navigator.mediaDevices.getUserMedia({audio:true}).then(stream => {
                                stream.getTracks().forEach(track => track.stop());
                                window.mediaResult = 'unexpected-grant';
                              }, error => { window.mediaResult = error.name; });
                            };
                        </script>""".trimIndent()
                        return WebResourceResponse("text/html", "utf-8", 200, "OK", emptyMap(),
                            ByteArrayInputStream(html.toByteArray(Charsets.UTF_8)))
                    }
                    return delegate.shouldInterceptRequest(view, request)
                }
                override fun onPageStarted(view: WebView, url: String?, icon: Bitmap?) = delegate.onPageStarted(view, url, icon)
                override fun doUpdateVisitedHistory(view: WebView, url: String?, reload: Boolean) = delegate.doUpdateVisitedHistory(view, url, reload)
                override fun onPageCommitVisible(view: WebView, visibleUrl: String?) {
                    delegate.onPageCommitVisible(view, visibleUrl)
                    if (visibleUrl == url && view.url == url) {
                        committed = true
                        if (finished) done.countDown()
                    }
                }
                override fun onPageFinished(view: WebView, finishedUrl: String?) {
                    delegate.onPageFinished(view, finishedUrl)
                    if (finishedUrl == url && view.url == url) {
                        finished = true
                        if (committed) done.countDown()
                    }
                }
            }
            entry.webView.loadUrl(url)
        }
        try {
            assertTrue("Media fixture did not commit and finish", done.await(20, TimeUnit.SECONDS))
            main {
                assertEquals(url, entry.webView.url)
                assertTrue(entry.resumeState.canRequestMedia(url))
            }
            assertEquals("\"function\"", evaluate(entry.webView, "typeof window.askForMicrophone"))
        } finally {
            main { entry.webView.webViewClient = requireNotNull(original) }
        }
    }

    private fun clickSiteButton(view: WebView, text: String) {
        val deadline = SystemClock.elapsedRealtime() + 15_000L
        while (SystemClock.elapsedRealtime() < deadline) {
            val tree = instrumentation.uiAutomation.rootInActiveWindow
            if (tree != null && tree.packageName?.toString() == context.packageName) {
                val origins = tree.findAccessibilityNodeInfosByText("media.sumi.example")
                val button = tree.findAccessibilityNodeInfosByText(text).firstOrNull {
                    it.text?.toString() == text && it.isClickable && it.isEnabled
                }
                if (origins.isNotEmpty() && button != null) {
                    assertEquals("A visible site prompt must not already deny or grant the request",
                        "\"pending\"", evaluate(view, "window.mediaResult"))
                    assertTrue("Site button did not accept click", button.performAction(AccessibilityNodeInfo.ACTION_CLICK))
                    return
                }
            }
            SystemClock.sleep(100)
        }
        fail("Origin-named site permission button did not appear: $text")
    }

    private fun denyAndroidPermission(view: WebView) {
        val deadline = SystemClock.elapsedRealtime() + 15_000L
        while (SystemClock.elapsedRealtime() < deadline) {
            val tree = instrumentation.uiAutomation.rootInActiveWindow
            val owner = tree?.packageName?.toString()
            if (tree != null && (owner == "com.google.android.permissioncontroller" || owner == "com.android.permissioncontroller")) {
                val button = listOf("com.android.permissioncontroller", "com.google.android.permissioncontroller")
                    .flatMap { tree.findAccessibilityNodeInfosByViewId("$it:id/permission_deny_button") }
                    .firstOrNull { it.isClickable && it.isEnabled }
                if (button != null) {
                    assertEquals("Site consent must wait for Android's answer before settling getUserMedia",
                        "\"pending\"", evaluate(view, "window.mediaResult"))
                    assertTrue("Android denial button did not accept click", button.performAction(AccessibilityNodeInfo.ACTION_CLICK))
                    return
                }
            }
            SystemClock.sleep(100)
        }
        fail("The real Android permission dialog did not appear after site consent")
    }

    private fun awaitDenial(view: WebView) {
        val deadline = SystemClock.elapsedRealtime() + 15_000L
        while (SystemClock.elapsedRealtime() < deadline) {
            val result = evaluate(view, "window.mediaResult")
            if (result != "\"pending\"") {
                assertEquals("Actual getUserMedia must reject after denial", "\"NotAllowedError\"", result)
                return
            }
            SystemClock.sleep(100)
        }
        fail("getUserMedia did not settle after native denial")
    }

    @Test
    fun webViewSiteConsentAndAndroidDenialReturnToTheSameScreen() {
        assertEquals("This fixture must start without an OS microphone grant", PackageManager.PERMISSION_DENIED,
            context.checkSelfPermission(Manifest.permission.RECORD_AUDIO))
        val profile = UUID.randomUUID().toString()
        val key = ScreenRequest.webHomeKey(root, profile)
        val request = ScreenRequest(key, root, "Media fixture", webHome = true, profile = profile, scope = key)
        val automation = instrumentation.uiAutomation
        val originalFlags = automation.serviceInfo.flags
        automation.serviceInfo = automation.serviceInfo.apply { flags = flags or AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS }
        try {
            ActivityScenario.launch<AppScreenActivity>(AppScreenLaunch.documentIntent(context, request)).use { scenario ->
                val entry = main { requireNotNull(AppScreenRegistry.get(key)) }
                document(entry)
                evaluate(entry.webView, "window.askForMicrophone()")
                clickSiteButton(entry.webView, "許可しない")
                awaitDenial(entry.webView)
                assertEquals(PackageManager.PERMISSION_DENIED, context.checkSelfPermission(Manifest.permission.RECORD_AUDIO))

                // A fresh native document does not reuse any WebView-cached
                // denial. The real OS UI must follow this explicit consent.
                document(entry)
                evaluate(entry.webView, "window.askForMicrophone()")
                clickSiteButton(entry.webView, "許可")
                denyAndroidPermission(entry.webView)
                awaitDenial(entry.webView)
                instrumentation.waitForIdleSync()
                assertEquals(Lifecycle.State.RESUMED, scenario.state)
                main {
                    assertSame(entry, AppScreenRegistry.get(key))
                    assertTrue(entry.webView.isAttachedToWindow && entry.webView.isShown)
                    assertEquals(profile, entry.profile)
                }
                assertEquals(PackageManager.PERMISSION_DENIED, context.checkSelfPermission(Manifest.permission.RECORD_AUDIO))
            }
        } finally {
            automation.serviceInfo = automation.serviceInfo.apply { flags = originalFlags }
            main { AppScreenRegistry.remove(key) }
        }
    }
}
