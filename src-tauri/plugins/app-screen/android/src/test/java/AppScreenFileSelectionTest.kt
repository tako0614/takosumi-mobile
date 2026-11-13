package com.takosumi.appscreen

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.webkit.ValueCallback
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], manifest = Config.NONE)
class AppScreenFileSelectionTest {
    private class Reply : ValueCallback<Array<Uri>> {
        val values = mutableListOf<List<Uri>?>()
        override fun onReceiveValue(value: Array<Uri>?) { values.add(value?.toList()) }
    }

    private class Fixture {
        val view = Any()
        val original = AppScreenFileSelection.Context(view, "profile-a", 2, 3, "https://app.example/edit")
        var context: AppScreenFileSelection.Context? = original
        var interactive = true
        var launchFails = false
        val intents = mutableListOf<Intent>()
        val selection = AppScreenFileSelection(
            current = { context },
            interactive = { interactive },
            launch = {
                if (launchFails) throw ActivityNotFoundException("fixture has no picker")
                intents.add(it)
            },
        )
        fun begin(reply: Reply, owner: Any = view) = selection.begin(owner, reply) { Intent(Intent.ACTION_GET_CONTENT) }
    }

    private val chosen = arrayOf(Uri.parse("content://fixture.documents/chosen"))

    @Test fun ordinaryPickerReturnSurvivesPauseAndStopWithoutRequiringResume() {
        val f = Fixture()
        val reply = Reply()
        f.begin(reply)
        f.interactive = false // opening an external picker stops its host Activity
        assertTrue(reply.values.isEmpty())
        f.selection.result(chosen)
        assertEquals(listOf(chosen.toList()), reply.values)
        assertFalse(f.selection.awaitingResult)
        f.selection.result(chosen)
        assertEquals(1, reply.values.size)
    }

    @Test fun secondRequestCannotReplaceTheOwnerOfAnOutstandingPicker() {
        val f = Fixture()
        val first = Reply()
        val second = Reply()
        f.begin(first)
        f.begin(first) // duplicate delivery must not cancel its own request
        f.begin(second)
        assertEquals(listOf<List<Uri>?>(null), second.values)
        assertEquals(1, f.intents.size)
        f.selection.result(chosen)
        assertEquals(listOf(chosen.toList()), first.values)
        assertEquals(listOf<List<Uri>?>(null), second.values)
    }

    @Test fun canceledDocumentKeepsAnEmptySlotUntilItsLateResultIsConsumed() {
        val f = Fixture()
        val first = Reply()
        f.begin(first)
        f.selection.cancel()
        f.selection.cancel()
        assertEquals(listOf<List<Uri>?>(null), first.values)
        assertTrue(f.selection.awaitingResult)
        val whileWaiting = Reply()
        f.begin(whileWaiting)
        assertEquals(listOf<List<Uri>?>(null), whileWaiting.values)
        assertEquals(1, f.intents.size)
        f.selection.result(chosen)
        val afterReturn = Reply()
        f.begin(afterReturn)
        f.selection.result(chosen)
        assertEquals(listOf(chosen.toList()), afterReturn.values)
        assertEquals(listOf<List<Uri>?>(null), first.values)
    }

    @Test fun returningResultRequiresTheSameViewProfileGenerationDocumentAndPage() {
        val template = Fixture()
        val changed = listOf(
            template.original.copy(view = Any()),
            template.original.copy(profile = "profile-b"),
            template.original.copy(generation = 3),
            template.original.copy(document = 4),
            template.original.copy(page = "https://app.example/another"),
            null,
        )
        for (context in changed) {
            val f = Fixture()
            val reply = Reply()
            f.begin(reply)
            f.context = context?.copy(view = if (context.view === template.view) f.view else context.view)
            f.selection.result(chosen)
            assertEquals("Changed context: $context", listOf<List<Uri>?>(null), reply.values)
        }
    }

    @Test fun samePageHistoryNotificationKeepsPickerButChangedPageCancels() {
        val f = Fixture()
        val reply = Reply()
        f.begin(reply)
        f.selection.pageChanged(f.original.page)
        assertTrue(reply.values.isEmpty())
        f.selection.pageChanged("https://app.example/next")
        f.selection.result(chosen)
        assertEquals(listOf<List<Uri>?>(null), reply.values)
    }

    @Test fun restoredActivityConsumesTheOldPickerResultWithoutAssigningANewCallback() {
        val old = Fixture()
        val originalReply = Reply()
        old.begin(originalReply)
        val saved = old.selection.awaitingResult
        old.selection.cancel() // onDestroy cancels the old WebView callback
        val restored = Fixture()
        restored.selection.restoreAwaitingResult(saved)
        val early = Reply()
        restored.begin(early)
        assertTrue(restored.intents.isEmpty())
        assertEquals(listOf<List<Uri>?>(null), early.values)
        restored.selection.result(chosen)
        val fresh = Reply()
        restored.begin(fresh)
        restored.selection.result(chosen)
        assertEquals(listOf(chosen.toList()), fresh.values)
        assertEquals(listOf<List<Uri>?>(null), originalReply.values)
    }

    @Test fun launchAndIntentFailuresAnswerOnceAndDoNotLeaveAnOutstandingPicker() {
        val f = Fixture()
        f.launchFails = true
        val noPicker = Reply()
        f.begin(noPicker)
        assertEquals(listOf<List<Uri>?>(null), noPicker.values)
        assertFalse(f.selection.awaitingResult)
        val badIntent = Reply()
        f.selection.begin(f.view, badIntent) { throw SecurityException("fixture denied intent") }
        assertEquals(listOf<List<Uri>?>(null), badIntent.values)
        assertFalse(f.selection.awaitingResult)
        f.launchFails = false
        val fresh = Reply()
        f.begin(fresh)
        f.selection.result(null) // native picker cancellation
        assertEquals(listOf<List<Uri>?>(null), fresh.values)
    }

    @Test fun noninteractiveOrWrongOwnerRequestsDoNotLaunchAPicker() {
        val f = Fixture()
        val wrong = Reply()
        f.begin(wrong, Any())
        f.interactive = false
        val stopped = Reply()
        f.begin(stopped)
        f.interactive = true
        f.context = null
        val detached = Reply()
        f.begin(detached)
        for (reply in listOf(wrong, stopped, detached)) assertEquals(listOf<List<Uri>?>(null), reply.values)
        assertTrue(f.intents.isEmpty())
    }
}
