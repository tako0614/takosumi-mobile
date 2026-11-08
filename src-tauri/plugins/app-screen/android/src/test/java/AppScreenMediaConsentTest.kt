package com.takosumi.appscreen

import android.Manifest
import android.net.Uri
import android.webkit.PermissionRequest
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], manifest = Config.NONE)
class AppScreenMediaConsentTest {
    private val video = PermissionRequest.RESOURCE_VIDEO_CAPTURE
    private val audio = PermissionRequest.RESOURCE_AUDIO_CAPTURE
    private val camera = Manifest.permission.CAMERA
    private val microphone = Manifest.permission.RECORD_AUDIO

    private class Request(val site: String, val wanted: Array<String>) : PermissionRequest() {
        val grants = mutableListOf<List<String>>()
        var denials = 0
        override fun getOrigin(): Uri = Uri.parse(site)
        override fun getResources(): Array<String> = wanted
        override fun grant(resources: Array<String>) { grants.add(resources.toList()) }
        override fun deny() { denials++ }
    }

    private class Fixture {
        val view = Any()
        val original = AppScreenMediaConsent.Context(view, "profile-a", 1, 1, "https://app.example/call")
        var context: AppScreenMediaConsent.Context? = original
        val permissions = mutableSetOf<String>()
        val prompts = mutableListOf<AppScreenMediaConsent.Access>()
        val android = mutableListOf<List<String>>()
        var allow: () -> Unit = {}
        var deny: () -> Unit = {}
        var dismissals = 0
        var launchFails = false
        var promptFails = false
        val consent = AppScreenMediaConsent(
            current = { context },
            prompt = { access, yes, no ->
                if (promptFails) throw IllegalStateException("fixture detached window")
                prompts.add(access)
                allow = yes
                deny = no
                ({ dismissals++ })
            },
            hasPermission = { it in permissions },
            askAndroid = {
                if (launchFails) throw IllegalStateException("fixture unavailable launcher")
                android.add(it.toList())
            },
        )
    }

    @Test fun rejectsUnknownResourcesAndNonWebOriginsWithoutPrompting() {
        val bad = listOf(
            Request("http://app.example", arrayOf(video)),
            Request("file:///private", arrayOf(video)),
            Request("https://user:secret@app.example", arrayOf(video)),
            Request("https://tauri.localhost", arrayOf(video)),
            Request("https://app.example/path", arrayOf(video)),
            Request("https://app.example?secret=value", arrayOf(video)),
            Request("https://app.example\nspoof", arrayOf(video)),
            Request("https://app.example", emptyArray()),
            Request("https://app.example", arrayOf(video, video)),
            Request("https://app.example", arrayOf(video, "future.protected.resource")),
        )
        for (request in bad) {
            val f = Fixture()
            f.consent.begin(request, f.view)
            assertEquals(request.site, 1, request.denials)
            assertTrue(f.prompts.isEmpty())
            assertTrue(f.android.isEmpty())
        }
        assertNull(AppScreenMediaConsent.access("https://frame.example", "http://app.example", arrayOf(audio)))
    }

    @Test fun detachedOrDifferentWebViewCannotAskForTheVisibleScreen() {
        val f = Fixture()
        val wrong = Request("https://app.example", arrayOf(video))
        f.consent.begin(wrong, Any())
        assertEquals(1, wrong.denials)
        f.context = null
        val detached = Request("https://app.example", arrayOf(video))
        f.consent.begin(detached, f.view)
        assertEquals(1, detached.denials)
        assertTrue(f.prompts.isEmpty())
    }

    @Test fun existingAndroidGrantStillNeedsSiteConsentForEachWebViewRequest() {
        val f = Fixture()
        f.permissions.addAll(listOf(camera, microphone))
        repeat(2) {
            val request = Request("https://frame.example/", arrayOf(video))
            f.consent.begin(request, f.view)
            assertTrue(request.grants.isEmpty())
            assertEquals("https://frame.example", f.prompts.last().origin)
            assertEquals("https://app.example", f.prompts.last().topOrigin)
            f.allow()
            f.allow() // stale UI callback cannot grant twice
            assertEquals(listOf(listOf(video)), request.grants)
        }
        assertEquals(2, f.prompts.size)
        assertTrue(f.android.isEmpty())
    }

    @Test fun siteDenialNeverRequestsAndroidPermission() {
        val f = Fixture()
        val request = Request("https://app.example", arrayOf(audio))
        f.consent.begin(request, f.view)
        f.deny()
        f.allow()
        assertEquals(1, request.denials)
        assertTrue(request.grants.isEmpty())
        assertTrue(f.android.isEmpty())
        val noWindow = Fixture().apply { promptFails = true }
        val unavailable = Request("https://app.example", arrayOf(video))
        noWindow.consent.begin(unavailable, noWindow.view)
        assertEquals(1, unavailable.denials)
        assertTrue(noWindow.android.isEmpty())
    }

    @Test fun partialAndroidGrantOrRevocationDoesNotGrantBothResources() {
        for (revoked in listOf(false, true)) {
            val f = Fixture()
            val request = Request("https://app.example", arrayOf(video, audio))
            f.consent.begin(request, f.view)
            f.allow()
            assertEquals(listOf(listOf(camera, microphone)), f.android)
            f.permissions.add(camera)
            f.consent.androidResult(mapOf(camera to true, microphone to revoked))
            assertEquals(1, request.denials)
            assertTrue(request.grants.isEmpty())
        }
    }

    @Test fun androidResultBeforeResumeWaitsForTheSameVisibleDocument() {
        val f = Fixture()
        val request = Request("https://app.example", arrayOf(audio))
        f.consent.begin(request, f.view)
        f.allow()
        f.context = null // system permission dialog paused this Activity
        f.consent.paused()
        f.permissions.add(microphone)
        f.consent.androidResult(mapOf(microphone to true))
        assertTrue(request.grants.isEmpty())
        assertEquals(0, request.denials)
        f.context = f.original
        f.consent.resumed()
        assertEquals(listOf(listOf(audio)), request.grants)
    }

    @Test fun changedViewProfileGenerationDocumentOrPageRefusesLateConsent() {
        val f = Fixture()
        val contexts = listOf(
            f.original.copy(view = Any()), f.original.copy(profile = "profile-b"),
            f.original.copy(profileGeneration = 2), f.original.copy(document = 2),
            f.original.copy(page = "https://other.example/call"),
        )
        for (changed in contexts) {
            f.context = f.original
            f.permissions.clear()
            val request = Request("https://app.example", arrayOf(video))
            f.consent.begin(request, f.view)
            f.allow()
            f.context = changed
            f.permissions.add(camera)
            f.consent.androidResult(mapOf(camera to true))
            assertEquals(changed.toString(), 1, request.denials)
            assertTrue(request.grants.isEmpty())
        }
    }

    @Test fun canceledAndroidRequestCannotSettleANewerRequest() {
        val f = Fixture()
        val old = Request("https://app.example", arrayOf(video))
        f.consent.begin(old, f.view)
        f.allow()
        f.consent.cancel() // background, detach, clear or navigation
        val whileWaiting = Request("https://app.example", arrayOf(audio))
        f.consent.begin(whileWaiting, f.view)
        assertEquals(1, whileWaiting.denials)
        f.permissions.add(camera)
        f.consent.androidResult(mapOf(camera to true))
        assertTrue(old.grants.isEmpty())
        assertEquals(1, old.denials)
        val next = Request("https://app.example", arrayOf(audio))
        f.consent.begin(next, f.view)
        assertEquals(2, f.prompts.size)
        assertTrue(next.grants.isEmpty())
        f.allow()
        assertEquals(listOf(microphone), f.android.last())
        f.permissions.add(microphone)
        f.consent.androidResult(mapOf(microphone to true))
        assertEquals(listOf(listOf(audio)), next.grants)
    }

    @Test fun webViewCancellationDismissesWithoutAnsweringAnAlreadyCanceledRequest() {
        val f = Fixture()
        val request = Request("https://app.example", arrayOf(video))
        f.consent.begin(request, f.view)
        f.consent.canceledByWebView(request)
        f.allow()
        assertEquals(1, f.dismissals)
        assertEquals(0, request.denials)
        assertTrue(request.grants.isEmpty())
    }

    @Test fun pauseNavigationAndUnavailableLauncherCloseThePrompt() {
        for (reason in listOf("pause", "navigate", "launch")) {
            val f = Fixture()
            val request = Request("https://app.example", arrayOf(video))
            f.consent.begin(request, f.view)
            when (reason) {
                "pause" -> f.consent.paused()
                "navigate" -> f.consent.pageChanged("https://app.example/next")
                else -> { f.launchFails = true; f.allow() }
            }
            assertEquals(reason, 1, request.denials)
            assertTrue(request.grants.isEmpty())
            assertEquals(1, f.dismissals)
        }
    }

    @Test fun mediaRequiresTheCurrentCommittedOrSuccessfullyKeptDocument() {
        val state = AppScreenResumeTracker()
        val url = "https://app.example/call"
        assertFalse(state.canRequestMedia(url))
        state.started(url)
        assertFalse(state.canRequestMedia(url))
        state.committed(url)
        assertTrue(state.canRequestMedia(url))
        state.finished(url)
        assertTrue(state.canRequestMedia(url))
        val first = state.document
        state.started(url)
        assertNotEquals(first, state.document)
        assertFalse(state.canRequestMedia(url))
        state.committed(url)
        state.failed(url)
        assertFalse(state.canRequestMedia(url))
    }
}
