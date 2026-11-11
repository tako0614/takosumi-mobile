package com.takosumi.fixture.geckodownload;

import android.app.Instrumentation;
import android.graphics.Rect;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.accessibility.AccessibilityNodeInfo;
import androidx.test.core.app.ActivityScenario;
import androidx.test.ext.junit.runners.AndroidJUnit4;
import androidx.test.platform.app.InstrumentationRegistry;
import java.io.File;
import java.nio.file.Files;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import java.util.function.BooleanSupplier;
import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import static org.junit.Assert.*;

/** Actual engine callbacks, native input and original bytes. No delegate/body replacement. */
@RunWith(AndroidJUnit4.class)
public final class ResponseCapabilityTest {
    final Instrumentation instrumentation = InstrumentationRegistry.getInstrumentation();
    ActivityScenario<ResponseProbeActivity> scenario;
    ResponseProbeActivity activity;

    private void await(String message, BooleanSupplier ready, int seconds) {
        long end = SystemClock.elapsedRealtime() + seconds * 1000L;
        while (SystemClock.elapsedRealtime() < end) {
            assertNull(activity.failure);
            if (ready.getAsBoolean()) return;
            SystemClock.sleep(100);
        }
        fail(message + "\n" + visibleTree());
    }

    /** A bounded snapshot before @After closes the owned fixture, not a substitute selector. */
    private String visibleTree() {
        AccessibilityNodeInfo root = instrumentation.getUiAutomation().getRootInActiveWindow();
        if (root == null) return "No active accessibility root";
        StringBuilder result = new StringBuilder();
        try { describe(root, result, 0, new int[] {200}); } finally { root.recycle(); }
        return result.toString();
    }

    private void describe(AccessibilityNodeInfo node, StringBuilder result, int depth, int[] remaining) {
        if (depth > 16 || remaining[0]-- <= 0) return;
        Rect bounds = new Rect();
        node.getBoundsInScreen(bounds);
        String text = String.valueOf(node.getText());
        result.append(depth).append(' ').append(node.getClassName())
            .append(" visible=").append(node.isVisibleToUser()).append(" enabled=").append(node.isEnabled())
            .append(' ').append(bounds.toShortString()).append(" text=")
            .append(text, 0, Math.min(text.length(), 200)).append('\n');
        if (depth == 16) return;
        for (int index = 0; index < node.getChildCount() && remaining[0] > 0; index++) {
            AccessibilityNodeInfo child = node.getChild(index);
            if (child == null) continue;
            try { describe(child, result, depth + 1, remaining); } finally { child.recycle(); }
        }
    }

    private Rect textBounds(String text) {
        AccessibilityNodeInfo root = instrumentation.getUiAutomation().getRootInActiveWindow();
        if (root == null) return null;
        try { return findText(root, text); } finally { root.recycle(); }
    }

    private Rect findText(AccessibilityNodeInfo node, String text) {
        if (node.isVisibleToUser() && node.isEnabled() && node.getText() != null
                && text.equals(node.getText().toString())) {
            Rect bounds = new Rect();
            node.getBoundsInScreen(bounds);
            if (!bounds.isEmpty()) return bounds;
        }
        for (int index = 0; index < node.getChildCount(); index++) {
            AccessibilityNodeInfo child = node.getChild(index);
            if (child == null) continue;
            try {
                Rect bounds = findText(child, text);
                if (bounds != null) return bounds;
            } finally { child.recycle(); }
        }
        return null;
    }

    private void visible(String text) { await("Missing actual page text: " + text, () -> textBounds(text) != null, 20); }

    private void tap(String text) {
        visible(text);
        // A present label can still be mid-reflow right after a profile
        // switch reloads the WebView; an injected tap on the rect just read
        // lands where the node no longer is. Only a rect that survives
        // unchanged for half a second may be tapped (same rule as
        // WebAppOperationTest.stable).
        Rect[] settled = {null};
        await("Page text did not settle: " + text,
            () -> (settled[0] = stable("tap " + text, textBounds(text))) != null, 15);
        Rect bounds = settled[0];
        long down = SystemClock.uptimeMillis();
        for (int action : new int[] {MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP}) {
            MotionEvent event = MotionEvent.obtain(down, SystemClock.uptimeMillis(), action, bounds.exactCenterX(), bounds.exactCenterY(), 0);
            try { instrumentation.sendPointerSync(event); } finally { event.recycle(); }
        }
    }

    private final Map<String, Rect> lastBounds = new HashMap<>();
    private final Map<String, Long> lastChanged = new HashMap<>();

    private Rect stable(String key, Rect rect) {
        if (rect == null) { lastBounds.remove(key); lastChanged.remove(key); return null; }
        Rect previous = lastBounds.get(key);
        if (!rect.equals(previous)) {
            lastBounds.put(key, new Rect(rect));
            lastChanged.put(key, SystemClock.elapsedRealtime());
            return null;
        }
        return SystemClock.elapsedRealtime() - lastChanged.get(key) >= 500 ? rect : null;
    }

    private ResponseProbeActivity.Transfer response(String link) throws Exception {
        assertTrue("Unexpected extra original response", activity.responses.isEmpty());
        tap(link);
        ResponseProbeActivity.Transfer transfer = activity.responses.poll(20, TimeUnit.SECONDS);
        if (transfer == null) fail("No actual external response from Gecko for " + link + "\n"
            + activity.diagnostics() + "\n" + visibleTree());
        assertEquals(activity.work, transfer.context);
        return transfer;
    }

    private void exact(ResponseProbeActivity.Transfer transfer, byte[] expected) throws Exception {
        assertTrue("Original response did not finish", transfer.finished.await(10, TimeUnit.SECONDS));
        assertNull(transfer.error);
        assertFalse(transfer.cancelled);
        assertTrue(transfer.closed);
        assertEquals(expected.length, transfer.bytes);
        assertArrayEquals(expected, Files.readAllBytes(transfer.file.toPath()));
        assertTrue(transfer.file.delete());
    }

    private void interrupt(String button) throws Exception {
        ResponseProbeActivity.Transfer transfer = response("Slow attachment");
        await("No actual partial write before cancel", () -> transfer.bytes > 0 && transfer.file != null && transfer.file.length() > 0, 10);
        assertEquals(1, transfer.finished.getCount());
        tap(button);
        assertTrue("Interrupted stream still owns its file", transfer.finished.await(5, TimeUnit.SECONDS));
        assertTrue(transfer.cancelled);
        assertTrue(transfer.closed);
        assertTrue(transfer.bytes > 0 && transfer.bytes < FixtureServer.SLOW_BYTES);
        assertFalse("Partial file remains after stream close", transfer.file.exists());
        assertNotEquals("Owned partial file was not deleted", transfer.error);
    }

    @Test public void originalCookieBlobAndCancellationStayWithinTheirContext() throws Exception {
        scenario = ActivityScenario.launch(ResponseProbeActivity.class);
        scenario.onActivity(value -> activity = value);
        // Exercise the native control labels before any engine result can obscure them.
        for (String button : new String[] {"Work", "Personal", "Reopen Work", "Cancel transfer", "Close Work"}) visible(button);
        visible("Fixture signed out");
        tap("Fixture sign in");
        visible("Fixture signed in");
        exact(response("Cookie attachment"), FixtureServer.COOKIE);
        assertEquals(1, activity.server.authenticatedDownloads.get());

        tap("Personal");
        visible("Fixture signed out");
        tap("Cookie attachment");
        visible("Cookie denied");
        assertEquals(1, activity.server.deniedDownloads.get());
        assertTrue("A denied page must not become a saved response", activity.responses.isEmpty());

        tap("Reopen Work");
        visible("Fixture signed in");
        exact(response("Cookie attachment"), FixtureServer.COOKIE);
        assertEquals("No URL replay or substitute HTTP client", 2, activity.server.authenticatedDownloads.get());
        exact(response("Blob export"), FixtureServer.BLOB);

        interrupt("Cancel transfer");
        interrupt("Close Work");
        await("The server still sends the canceled responses", () -> activity.server.interruptedSlowResponses.get() == 2, 10);
        tap("Personal");
        visible("Fixture signed out");
        assertTrue(activity.responses.isEmpty());
        assertArrayEquals(new byte[] {11, 5}, Files.readAllBytes(new File(activity.directory, "keep.txt").toPath()));
        assertEquals("Only the unrelated sentinel may remain", 1, activity.directory.list().length);
        System.out.println("PASS: Gecko159 original cookie/Blob bytes; separate/reopened contexts; actual stream cancel and session close. Loopback HTTP only, not product/TLS/reset/SSO qualification.");
    }

    @After public void retireOwnedFixture() throws Exception {
        if (scenario != null) scenario.close();
        if (activity == null) return;
        // Never delete files while their writer might still be alive.
        boolean stopped = activity.writer.awaitTermination(5, TimeUnit.SECONDS);
        if (activity.server != null) activity.server.close();
        assertTrue("Owned response writer did not stop; directory retained", stopped);
        for (ResponseProbeActivity.Transfer transfer : activity.transfers) {
            assertEquals(0, transfer.finished.getCount());
            assertTrue("Original stream close unproven; retain owned files", transfer.closed);
            if (transfer.file != null && transfer.file.exists()) Files.delete(transfer.file.toPath());
        }
        Files.delete(new File(activity.directory, "keep.txt").toPath());
        Files.delete(activity.directory.toPath());
    }
}
