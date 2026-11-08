package com.takosumi.appscreen

import android.content.ContentValues
import android.graphics.Bitmap
import android.os.Build
import android.os.Environment
import android.os.SystemClock
import android.provider.MediaStore
import android.view.MotionEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.ByteArrayInputStream
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Real WebView input, system picker and File.text(). Only a newly created
 * disposable Downloads entry is read/deleted; no WebChromeClient substitution. */
@RunWith(AndroidJUnit4::class)
class FileSelectionActivityTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext
    private val root = "https://files.sumi.example/"
    private val pickerPackages = setOf("com.google.android.documentsui", "com.android.documentsui")

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
        val url = "${root}upload?fixture=${UUID.randomUUID()}"
        val done = CountDownLatch(1)
        val original = main { entry.webView.webViewClient }
        main {
            entry.webView.stopLoading()
            entry.webView.webViewClient = object : WebViewClient() {
                private var committed = false
                private var finished = false
                override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                    if (request.isForMainFrame && request.url.toString() == url) {
                        val html = """<!doctype html><meta name="viewport" content="width=device-width, initial-scale=1">
                            <title>File fixture</title>
                            <input id="file" type="file" accept=".txt" style="margin:40px;width:250px;height:60px">
                            <script>
                              window.fileResult = null;
                              document.querySelector('input').addEventListener('change', async event => {
                                try {
                                  const files = event.target.files;
                                  const file = files[0];
                                  window.fileResult = {count:files.length, name:file.name, size:file.size,
                                    type:file.type, text:await file.text()};
                                } catch (error) { window.fileResult = {error:String(error)}; }
                              });
                            </script>""".trimIndent()
                        return WebResourceResponse("text/html", "utf-8", 200, "OK", emptyMap(),
                            ByteArrayInputStream(html.toByteArray(Charsets.UTF_8)))
                    }
                    return original.shouldInterceptRequest(view, request)
                }
                override fun onPageStarted(view: WebView, url: String?, icon: Bitmap?) = original.onPageStarted(view, url, icon)
                override fun doUpdateVisitedHistory(view: WebView, url: String?, reload: Boolean) = original.doUpdateVisitedHistory(view, url, reload)
                override fun onPageCommitVisible(view: WebView, visibleUrl: String?) {
                    original.onPageCommitVisible(view, visibleUrl)
                    if (visibleUrl == url && view.url == url) { committed = true; if (finished) done.countDown() }
                }
                override fun onPageFinished(view: WebView, finishedUrl: String?) {
                    original.onPageFinished(view, finishedUrl)
                    if (finishedUrl == url && view.url == url) { finished = true; if (committed) done.countDown() }
                }
            }
            entry.webView.loadUrl(url)
        }
        try {
            assertTrue("File fixture did not commit and finish", done.await(20, TimeUnit.SECONDS))
            main { assertTrue(entry.resumeState.canRequestMedia(url)) }
        } finally {
            main { entry.webView.webViewClient = original }
        }
    }

    private fun tapFileInput(view: WebView) {
        val point = JSONObject(requireNotNull(evaluate(view, """(() => {
            const r = document.querySelector('input').getBoundingClientRect();
            return {x:(r.left + 30) * devicePixelRatio, y:(r.top + r.height / 2) * devicePixelRatio};
        })()""")))
        val location = main { IntArray(2).also { view.getLocationOnScreen(it) } }
        val x = location[0] + point.getDouble("x").toFloat()
        val y = location[1] + point.getDouble("y").toFloat()
        val down = SystemClock.uptimeMillis()
        for (action in listOf(MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP)) {
            val event = MotionEvent.obtain(down, SystemClock.uptimeMillis(), action, x, y, 0)
            try { instrumentation.sendPointerSync(event) } finally { event.recycle() }
        }
    }

    private fun clickNode(node: AccessibilityNodeInfo): Boolean {
        var current: AccessibilityNodeInfo? = node
        repeat(5) {
            val candidate = current ?: return false
            if (candidate.isClickable && candidate.isEnabled) return candidate.performAction(AccessibilityNodeInfo.ACTION_CLICK)
            current = candidate.parent
        }
        return false
    }

    private fun chooseFixture(name: String, view: WebView) {
        val deadline = SystemClock.elapsedRealtime() + 20_000L
        var pickerSeen = false
        var drawerOpened = false
        var downloadsChosen = false
        while (SystemClock.elapsedRealtime() < deadline) {
            val tree = instrumentation.uiAutomation.rootInActiveWindow
            if (tree != null && tree.packageName?.toString() in pickerPackages) {
                pickerSeen = true
                val file = tree.findAccessibilityNodeInfosByText(name).firstOrNull { it.text?.toString() == name }
                if (file != null) {
                    assertEquals("A file must not reach the page before the picker answer", "null", evaluate(view, "window.fileResult"))
                    assertTrue("The fixture document could not be selected", clickNode(file))
                    return
                }
                // Use the actual DocumentsUI roots if Recents does not show
                // the newly published row. The CI AVD uses its default English UI.
                if (!drawerOpened) {
                    val nodes = ArrayDeque<AccessibilityNodeInfo>().apply { add(tree) }
                    var visited = 0
                    while (nodes.isNotEmpty() && visited++ < 128) {
                        val node = nodes.removeFirst()
                        if (node.contentDescription?.toString() == "Show roots" && clickNode(node)) {
                            drawerOpened = true
                            break
                        }
                        for (i in 0 until node.childCount) node.getChild(i)?.let { nodes.add(it) }
                    }
                } else if (!downloadsChosen) {
                    val downloads = tree.findAccessibilityNodeInfosByText("Downloads")
                        .firstOrNull { it.text?.toString() == "Downloads" }
                    if (downloads != null && clickNode(downloads)) downloadsChosen = true
                }
            }
            SystemClock.sleep(100)
        }
        fail("System picker did not show the exact fixture document; pickerSeen=$pickerSeen drawerOpened=$drawerOpened downloadsChosen=$downloadsChosen")
    }

    @Test
    fun actualPickerReturnsOnlyTheSelectedFixtureFileToTheSameDocument() {
        assertTrue("The owned Downloads fixture requires Android 10+", Build.VERSION.SDK_INT >= 29)
        assertEquals("Native tests must match the product target SDK", 36, context.applicationInfo.targetSdkVersion)
        val nonce = UUID.randomUUID().toString()
        val name = "sumi-file-$nonce.txt"
        val content = "Takosumi native file fixture $nonce\n"
        val resolver = context.contentResolver
        val uri = requireNotNull(resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, name)
            put(MediaStore.Downloads.MIME_TYPE, "text/plain")
            put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS)
            put(MediaStore.Downloads.IS_PENDING, 1)
        }))
        val profile = UUID.randomUUID().toString()
        val key = ScreenRequest.webHomeKey(root, profile)
        val request = ScreenRequest(key, root, "File fixture", webHome = true, profile = profile, scope = key)
        try {
            requireNotNull(resolver.openOutputStream(uri)).use { it.write(content.toByteArray(Charsets.UTF_8)) }
            assertEquals(1, resolver.update(uri, ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }, null, null))
            ActivityScenario.launch<AppScreenActivity>(AppScreenLaunch.documentIntent(context, request)).use { scenario ->
                val entry = main { requireNotNull(AppScreenRegistry.get(key)) }
                document(entry)
                tapFileInput(entry.webView)
                chooseFixture(name, entry.webView)
                val deadline = SystemClock.elapsedRealtime() + 15_000L
                var result: JSONObject? = null
                while (SystemClock.elapsedRealtime() < deadline) {
                    val value = evaluate(entry.webView, "window.fileResult")
                    if (value != null && value != "null") { result = JSONObject(value); break }
                    SystemClock.sleep(100)
                }
                val selected = requireNotNull(result) { "The selected file never reached the real WebView" }
                assertFalse(selected.has("error"))
                assertEquals(1, selected.getInt("count"))
                assertEquals(name, selected.getString("name"))
                assertEquals(content.toByteArray(Charsets.UTF_8).size, selected.getInt("size"))
                assertEquals("text/plain", selected.getString("type"))
                assertEquals(content, selected.getString("text"))
                instrumentation.waitForIdleSync()
                assertEquals(Lifecycle.State.RESUMED, scenario.state)
                main {
                    assertSame(entry, AppScreenRegistry.get(key))
                    assertEquals(profile, entry.profile)
                }
            }
        } finally {
            val tree = instrumentation.uiAutomation.rootInActiveWindow
            if (tree?.packageName?.toString() in pickerPackages) {
                instrumentation.uiAutomation.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK)
            }
            main { AppScreenRegistry.remove(key) }
            assertEquals("Delete only the fixture's own Downloads row", 1, resolver.delete(uri, null, null))
        }
    }
}
