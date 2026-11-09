package com.takosumi.fixture.geckodownload;

import android.app.Instrumentation;
import android.content.ContentValues;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Rect;
import android.net.Uri;
import android.os.Environment;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.provider.MediaStore;
import android.provider.DocumentsContract;
import android.view.InputDevice;
import android.view.KeyCharacterMap;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.accessibility.AccessibilityNodeInfo;
import androidx.test.core.app.ActivityScenario;
import androidx.test.ext.junit.runners.AndroidJUnit4;
import androidx.test.platform.app.InstrumentationRegistry;
import androidx.test.platform.io.PlatformTestStorageRegistry;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.FutureTask;
import java.util.function.BooleanSupplier;
import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import static org.junit.Assert.*;

/** Real anonymous editor + native Open/Save pickers. Never replace its page,
 * picker callback, download response or file bytes to satisfy the assertions. */
@RunWith(AndroidJUnit4.class)
public final class WebAppOperationTest {
    final Instrumentation instrumentation = InstrumentationRegistry.getInstrumentation();
    final Set<String> pickers = Set.of("com.android.documentsui", "com.google.android.documentsui");
    final String inputName = "SG-" + UUID.randomUUID().toString().substring(0, 18) + ".png";
    final String tabName = inputName.substring(0, inputName.length() - 4);
    ActivityScenario<WebAppProbeActivity> scenario;
    WebAppProbeActivity activity;
    Uri input;

    private void await(String message, BooleanSupplier predicate, int seconds) {
        long end = SystemClock.elapsedRealtime() + seconds * 1000L;
        while (SystemClock.elapsedRealtime() < end) {
            if (activity != null) assertNull(activity.failure);
            if (predicate.getAsBoolean()) return;
            SystemClock.sleep(100);
        }
        fail(message + "\n" + (activity == null ? "No Activity" : activity.diagnostics()) + "\n" + tree());
    }

    private boolean picker() {
        AccessibilityNodeInfo root = instrumentation.getUiAutomation().getRootInActiveWindow();
        if (root == null) return false;
        try { return pickers.contains(String.valueOf(root.getPackageName())); }
        finally { root.recycle(); }
    }

    private Rect find(AccessibilityNodeInfo node, String text, boolean contains, boolean description,
            boolean visibleOnly, int depth) {
        if (depth > 24) return null;
        CharSequence raw = description ? node.getContentDescription() : node.getText();
        if ((!visibleOnly || node.isVisibleToUser()) && node.isEnabled() && raw != null &&
                (contains ? raw.toString().contains(text) : raw.toString().equalsIgnoreCase(text))) {
            Rect bounds = new Rect(); node.getBoundsInScreen(bounds);
            if (!bounds.isEmpty()) return bounds;
        }
        for (int i = 0; i < node.getChildCount(); i++) {
            AccessibilityNodeInfo child = node.getChild(i);
            if (child == null) continue;
            try {
                Rect found = find(child, text, contains, description, visibleOnly, depth + 1);
                if (found != null) return found;
            } finally { child.recycle(); }
        }
        return null;
    }

    private Rect bounds(String text, boolean contains, boolean description) {
        return bounds(text, contains, description, true);
    }

    private Rect bounds(String text, boolean contains, boolean description, boolean visibleOnly) {
        AccessibilityNodeInfo root = instrumentation.getUiAutomation().getRootInActiveWindow();
        if (root == null) return null;
        try { return find(root, text, contains, description, visibleOnly, 0); } finally { root.recycle(); }
    }

    private void tap(Rect bounds) {
        tap(bounds, null);
    }

    private void tap(Rect bounds, String observedAction) {
        assertNotNull(bounds);
        AccessibilityNodeInfo root = instrumentation.getUiAutomation().getRootInActiveWindow();
        assertNotNull("Native tap needs the active window", root);
        Rect window = new Rect();
        String targetPackage;
        int targetWindow;
        try {
            root.getBoundsInScreen(window);
            targetPackage = String.valueOf(root.getPackageName());
            targetWindow = root.getWindowId();
        } finally { root.recycle(); }
        boolean systemPicker = pickers.contains(targetPackage);
        assertTrue("Native tap must target this fixture or the actual DocumentsUI: " + targetPackage,
            systemPicker || instrumentation.getTargetContext().getPackageName().equals(targetPackage));
        assertTrue("Native tap must stay inside the active window: " + bounds, window.contains(bounds));
        long down = SystemClock.uptimeMillis();
        boolean gestureStarted = false;
        try {
            for (int action : new int[] {MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP}) {
                AccessibilityNodeInfo current = instrumentation.getUiAutomation().getRootInActiveWindow();
                assertNotNull("Native tap target disappeared", current);
                try {
                    assertEquals("Native tap target package changed", targetPackage, String.valueOf(current.getPackageName()));
                    assertEquals("Native tap target window changed", targetWindow, current.getWindowId());
                    current.getBoundsInScreen(window);
                    assertTrue("Native tap moved outside its window", window.contains(bounds));
                } finally { current.recycle(); }
                if (observedAction != null) {
                    try {
                        Rect currentAction = bounds(observedAction, false, false);
                        activity.trace("open touch phase=" + action + " sent=" + bounds.toShortString() +
                            " current=" + (currentAction == null ? "missing" : currentAction.toShortString()) +
                            " window=" + window.toShortString() + " filePrompts=" + activity.filePrompts);
                    } catch (RuntimeException observation) {
                        activity.trace("open touch observation unavailable " + observation.getClass().getSimpleName());
                    }
                }
                MotionEvent event = MotionEvent.obtain(down, SystemClock.uptimeMillis(), action,
                    bounds.exactCenterX(), bounds.exactCenterY(), 0);
                event.setSource(InputDevice.SOURCE_TOUCHSCREEN);
                try {
                    // sendPointerSync is restricted to the instrumented app UID.
                    // The real OS picker belongs to its own UID; UiAutomation is
                    // Android's cross-app test input route. No permission adoption,
                    // shell input or direct file-prompt callback substitutes for it.
                    if (systemPicker) assertTrue("DocumentsUI rejected native touch",
                        instrumentation.getUiAutomation().injectInputEvent(event, true));
                    else instrumentation.sendPointerSync(event);
                    gestureStarted = action == MotionEvent.ACTION_DOWN;
                } finally { event.recycle(); }
            }
        } catch (RuntimeException | Error failure) {
            if (gestureStarted) {
                // A changed target or rejected UP must not leave our accepted
                // DOWN held. Attempt CANCEL through the same input route, not
                // an UP on a replacement window. Preserve the original error.
                MotionEvent cancel = MotionEvent.obtain(down, SystemClock.uptimeMillis(),
                    MotionEvent.ACTION_CANCEL, bounds.exactCenterX(), bounds.exactCenterY(), 0);
                cancel.setSource(InputDevice.SOURCE_TOUCHSCREEN);
                try {
                    if (systemPicker) assertTrue("Owned picker touch stream could not be cancelled",
                        instrumentation.getUiAutomation().injectInputEvent(cancel, true));
                    else instrumentation.sendPointerSync(cancel);
                } catch (RuntimeException | Error cancellation) { failure.addSuppressed(cancellation); }
                finally { cancel.recycle(); }
            }
            throw failure;
        }
    }

    private void tap(String text) {
        await("Missing native text " + text, () -> bounds(text, false, false) != null, 30);
        tap(bounds(text, false, false));
    }

    private void enterEditor(String profile) {
        boolean[] entryTapped = {false};
        boolean[] entryScrolled = {false};
        long deadline = SystemClock.elapsedRealtime() + 60000;
        await("Actual anonymous Photopea File menu did not load for " + profile, () -> {
            if (bounds("File", false, false) != null) return true;
            if (!entryTapped[0]) {
                Rect viewport = visibleWebViewport(deadline);
                Rect start = bounds("Start using Photopea", false, false);
                if (start != null && viewport.contains(start)) {
                    entryTapped[0] = true;
                    activity.trace("landing tap " + profile + " viewport=" + viewport.toShortString() + " action=" + start.toShortString());
                    tap(start);
                } else if (!entryScrolled[0]) {
                    // The original 320x640 capture places the first CTA below
                    // the WebView. Its offscreen bounds authorize only a native
                    // scroll, never a tap or a synthesized page action.
                    Rect below = bounds("Start using Photopea", false, false, false);
                    if (below != null && below.top >= viewport.top && below.bottom > viewport.bottom) {
                        entryScrolled[0] = true;
                        activity.trace("landing scroll " + profile + " viewport=" + viewport.toShortString() + " action=" + below.toShortString());
                        swipeUp(viewport);
                    }
                }
            }
            return false;
        }, 60);
    }

    private Rect visibleWebViewport(long deadline) {
        Handler main = new Handler(Looper.getMainLooper());
        FutureTask<Rect> read = new FutureTask<>(() -> {
            Rect viewport = new Rect();
            if (!activity.view.getGlobalVisibleRect(viewport)) throw new IllegalStateException("GeckoView is not visible");
            return viewport;
        });
        try {
            long remaining = deadline - SystemClock.elapsedRealtime();
            assertTrue("Photopea entry deadline expired", remaining > 0);
            assertTrue("Main thread rejected viewport read", main.post(read));
            return read.get(Math.min(1000, remaining), TimeUnit.MILLISECONDS);
        } catch (Exception error) {
            if (error instanceof InterruptedException) Thread.currentThread().interrupt();
            throw new AssertionError("Visible GeckoView bounds unavailable before the entry deadline", error);
        } finally {
            // A queued or late callback only reads geometry. It must never
            // perform delayed native input after this test has failed.
            main.removeCallbacks(read);
            read.cancel(false);
        }
    }

    private void swipeUp(Rect viewport) {
        assertTrue(viewport.width() > 0 && viewport.height() > 0);
        long down = SystemClock.uptimeMillis();
        for (int step = 0; step <= 12; step++) {
            int action = step == 0 ? MotionEvent.ACTION_DOWN : step == 12 ? MotionEvent.ACTION_UP : MotionEvent.ACTION_MOVE;
            float y = viewport.top + viewport.height() * (0.75f - 0.5f * step / 12);
            MotionEvent event = MotionEvent.obtain(down, SystemClock.uptimeMillis(), action,
                viewport.exactCenterX(), y, 0);
            try { instrumentation.sendPointerSync(event); } finally { event.recycle(); }
            if (step < 12) SystemClock.sleep(25);
        }
    }

    private void key(int code, int modifiers) {
        long down = SystemClock.uptimeMillis();
        for (int action : new int[] {KeyEvent.ACTION_DOWN, KeyEvent.ACTION_UP}) {
            instrumentation.sendKeySync(new KeyEvent(down, SystemClock.uptimeMillis(), action, code, 0, modifiers,
                KeyCharacterMap.VIRTUAL_KEYBOARD, 0, 0, InputDevice.SOURCE_KEYBOARD));
        }
    }

    private void openDownloads() {
        await("Actual DocumentsUI is required", this::picker, 15);
        Rect roots = bounds("Show roots", false, true);
        if (roots != null) {
            tap(roots);
            await("Missing Downloads root", () -> picker() && bounds("Downloads", false, false) != null, 10);
            tap(bounds("Downloads", false, false));
        } else {
            // A missing drawer is not an alternate fake picker. Require the
            // actual provider to already show its Downloads title.
            assertNotNull("No Downloads destination in real picker", bounds("Downloads", false, false));
        }
    }

    private void chooseInput() {
        openDownloads();
        boolean[] listViewRequested = {false};
        await("OS picker did not show exact owned image", () -> {
            if (!picker()) return false;
            if (bounds(inputName, false, false) != null) return true;
            // DocumentsUI's image grid exposes the filename only in a compound
            // description. Its thumbnail center can hit the Preview overlay.
            // Use the real layout control once, then require the exact visible
            // filename text in list view; do not match or tap the preview node.
            if (!listViewRequested[0]) {
                Rect listView = bounds("List view", false, true);
                if (listView != null) {
                    listViewRequested[0] = true;
                    activity.trace("picker list view " + listView.toShortString());
                    tap(listView);
                }
            }
            return false;
        }, 15);
        assertEquals(0, activity.fileSelections);
        // A row can be visible before it accepts input while the picker is
        // still settling after a view-mode switch: a lone tap inside that
        // window is lost without any picker-side error. Allow bounded
        // spaced re-taps only while the picker still shows the owned
        // filename unselected, tracing each attempt for the run's
        // evidence. The preview icon is a different node and never the
        // tap target.
        int[] inputTaps = {0};
        long[] lastInputTap = {0};
        await("Original file prompt did not receive the OS answer", () -> {
            if (activity.fileSelections == 1) return true;
            if (!picker()) return false;
            Rect file = bounds(inputName, false, false);
            if (file == null) return false;
            long now = SystemClock.elapsedRealtime();
            if (inputTaps[0] < 3 && now - lastInputTap[0] >= 2000) {
                inputTaps[0]++;
                lastInputTap[0] = now;
                activity.trace("input tap attempt=" + inputTaps[0] + " " + file.toShortString());
                tap(file);
            }
            return false;
        }, 30);
        assertEquals(activity.work, activity.inputContext);
    }

    private void saveOutput() {
        await("Native original-response consent missing", () -> bounds("Save original response", false, false) != null, 15);
        tap("Choose location");
        openDownloads();
        await("OS save filename differs from the owned export", () -> picker() && bounds(activity.exportName, false, false) != null, 10);
        assertTrue(picker());
        tap("Save");
        await("OS save result did not return to the candidate", () -> !picker(), 15);
    }

    private void createInput() throws Exception {
        ContentValues values = new ContentValues();
        values.put(MediaStore.Downloads.DISPLAY_NAME, inputName);
        values.put(MediaStore.Downloads.MIME_TYPE, "image/png");
        values.put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS);
        values.put(MediaStore.Downloads.IS_PENDING, 1);
        input = instrumentation.getTargetContext().getContentResolver().insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values);
        assertNotNull(input);
        Bitmap bitmap = Bitmap.createBitmap(128, 96, Bitmap.Config.ARGB_8888);
        for (int y = 0; y < 96; y++) for (int x = 0; x < 128; x++) bitmap.setPixel(x, y, x < 64 ? 0xffff0000 : 0xff0000ff);
        try (OutputStream out = instrumentation.getTargetContext().getContentResolver().openOutputStream(input)) {
            assertNotNull(out);
            assertTrue(bitmap.compress(Bitmap.CompressFormat.PNG, 100, out));
        } finally { bitmap.recycle(); }
        ContentValues published = new ContentValues(); published.put(MediaStore.Downloads.IS_PENDING, 0);
        assertEquals(1, instrumentation.getTargetContext().getContentResolver().update(input, published, null, null));
    }

    private void verifyPixels(WebAppProbeActivity.Export transfer) throws Exception {
        assertNotNull(transfer.output);
        assertTrue(transfer.bytes > 0 && transfer.bytes <= WebAppProbeActivity.MAX_EXPORT_BYTES);
        byte[] data;
        try (InputStream in = instrumentation.getTargetContext().getContentResolver().openInputStream(transfer.output)) {
            assertNotNull(in);
            data = in.readNBytes((int) WebAppProbeActivity.MAX_EXPORT_BYTES + 1);
        }
        assertEquals("Read the actual OS export once, after original stream/output completion", transfer.bytes, data.length);
        assertTrue(data.length >= 8);
        assertArrayEquals(new byte[] {(byte) 137, 80, 78, 71, 13, 10, 26, 10}, Arrays.copyOf(data, 8));
        BitmapFactory.Options dimensions = new BitmapFactory.Options();
        dimensions.inJustDecodeBounds = true;
        BitmapFactory.decodeByteArray(data, 0, data.length, dimensions);
        assertEquals(128, dimensions.outWidth); assertEquals(96, dimensions.outHeight);
        Bitmap decoded = BitmapFactory.decodeByteArray(data, 0, data.length);
        assertNotNull(decoded);
        try {
            assertEquals(128, decoded.getWidth()); assertEquals(96, decoded.getHeight());
            for (int y = 0; y < 96; y++) for (int x = 0; x < 128; x++) {
                assertEquals("Exported editor pixel at " + x + "," + y, x < 64 ? 0xffff0000 : 0xff0000ff, decoded.getPixel(x, y));
            }
        } finally { decoded.recycle(); }
        // Only bytes read back from the actual OS output become evidence.
        try (OutputStream out = evidence("photopea-export.png")) { out.write(data); }
    }

    @Test public void photopeaNativeOpenAndExportPreserveTheEditorAndOtherProfile() throws Exception {
        createInput();
        scenario = ActivityScenario.launch(WebAppProbeActivity.class);
        scenario.onActivity(value -> activity = value);
        enterEditor("Work");
        // Use the mobile editor's visible Open action. The original native
        // hierarchy includes this leading EN QUAD after its computer icon.
        // Ctrl+O did not open DocumentsUI; do not send both and risk two prompts.
        String openAction = "\u2000Open From Computer";
        await("Missing native text " + openAction, () -> bounds(openAction, false, false) != null, 30);
        // The accessibility node exists while the screen still shows the
        // site's own Loading overlay: a tap then lands on the overlay and
        // produces no prompt. Any non-white pixel is not enough either, since
        // the dimmed background already paints. Require real rendered content
        // inside the action's own bounds (a button's fill/border/text spans
        // more than one luminance band); then one capture, fresh bounds and a
        // single tap as before.
        try {
            await("Open action did not paint", () -> painted(bounds(openAction, false, false)), 60);
        } catch (AssertionError unpainted) {
            // Keep the screen that failed the paint gate: whether the site
            // still shows its Loading overlay decides the next diagnostic.
            try { captureUi("photopea-unpainted-open"); }
            catch (IOException | RuntimeException observation) {
                activity.trace("unpainted-open capture unavailable " + observation.getClass().getSimpleName());
            }
            throw unpainted;
        }
        try { captureUi("photopea-before-open"); }
        catch (IOException | RuntimeException observation) {
            activity.trace("before-open capture unavailable " + observation.getClass().getSimpleName());
        }
        tap(bounds(openAction, false, false), openAction);
        chooseInput();
        await("The actual editor did not open the selected image", () -> !picker() && bounds(tabName, true, false) != null, 30);
        WebAppProbeActivity.Page work = activity.pages.get(activity.work);
        assertNotNull(work);
        int document = work.generation;
        assertTrue(document > 0);
        String uri = work.uri;
        assertTrue(WebAppProbeActivity.site(uri));

        tap("Personal");
        enterEditor("Personal");
        assertNull("The imported Work document leaked to Personal", bounds(tabName, true, false));
        WebAppProbeActivity.Page personal = activity.pages.get(activity.personal);
        assertNotNull(personal);
        assertNotSame(work.session, personal.session);
        int personalDocument = personal.generation;
        String personalUri = personal.uri;
        tap("Work");
        await("Work editor was lost on native reattach", () -> bounds(tabName, true, false) != null, 15);
        assertEquals(document, work.generation);
        assertEquals(uri, work.uri);

        key(KeyEvent.KEYCODE_S, KeyEvent.META_CTRL_ON | KeyEvent.META_CTRL_LEFT_ON |
            KeyEvent.META_ALT_ON | KeyEvent.META_ALT_LEFT_ON | KeyEvent.META_SHIFT_ON | KeyEvent.META_SHIFT_LEFT_ON);
        await("Actual Photopea Save for web dialog did not open", () -> bounds("Save for web", false, false) != null, 20);
        key(KeyEvent.KEYCODE_ENTER, 0);
        WebAppProbeActivity.Export transfer = activity.responses.poll(20, TimeUnit.SECONDS);
        assertNotNull("No original Photopea export response\n" + activity.diagnostics() + "\n" + tree(), transfer);
        assertSame(work, transfer.page);
        assertEquals(document, transfer.generation);
        assertTrue("The real editor export must be its original HTTPS Blob", WebAppProbeActivity.blob(transfer.response.uri));
        saveOutput();
        assertTrue("Original export did not finish", transfer.finished.await(20, TimeUnit.SECONDS));
        assertNull(transfer.error);
        assertFalse(transfer.cancelled);
        assertTrue(transfer.closed);
        verifyPixels(transfer);
        key(KeyEvent.KEYCODE_ESCAPE, 0);
        assertEquals(document, work.generation);
        assertEquals(uri, work.uri);
        assertNotNull(bounds(tabName, true, false));
        tap("Personal");
        assertSame(personal, activity.pages.get(activity.personal));
        assertEquals(personalDocument, personal.generation);
        assertEquals(personalUri, personal.uri);
        assertNull(bounds(tabName, true, false));
        assertEquals(1, activity.filePrompts);
        assertEquals(1, activity.fileSelections);
        assertEquals(1, activity.exports.size());
        assertTrue(activity.responses.isEmpty());
        System.out.println("PASS: anonymous Photopea native Open, original Blob response, actual OS PNG export/all pixels, separate editor and native reattach. Not product/reset/IdP/notifications qualification.");
    }

    private String tree() {
        AccessibilityNodeInfo root = instrumentation.getUiAutomation().getRootInActiveWindow();
        if (root == null) return "No active accessibility root";
        StringBuilder out = new StringBuilder();
        try { describe(root, out, 0, new int[] {250}); } finally { root.recycle(); }
        return out.toString();
    }

    private void describe(AccessibilityNodeInfo node, StringBuilder out, int depth, int[] left) {
        if (depth > 24 || left[0]-- <= 0 || out.length() >= 16000) return;
        Rect bounds = new Rect(); node.getBoundsInScreen(bounds);
        out.append(depth).append(' ').append(bounded(node.getClassName())).append(' ').append(bounds.toShortString())
            .append(" text=").append(bounded(node.getText()))
            .append(" description=").append(bounded(node.getContentDescription())).append('\n');
        if (out.length() > 16000) out.setLength(16000);
        for (int i = 0; i < node.getChildCount() && left[0] > 0 && out.length() < 16000; i++) {
            AccessibilityNodeInfo child = node.getChild(i);
            if (child != null) try { describe(child, out, depth + 1, left); } finally { child.recycle(); }
        }
    }

    private String bounded(CharSequence raw) {
        String value = String.valueOf(raw);
        return value.substring(0, Math.min(200, value.length()));
    }

    private OutputStream evidence(String name) throws IOException {
        // The connected-test runner collects these before retiring its APKs.
        // App-specific external files can disappear before a later adb pull.
        return PlatformTestStorageRegistry.getInstance().openOutputFile(name);
    }

    private boolean painted(Rect target) {
        if (target == null) return false;
        Bitmap screen = instrumentation.getUiAutomation().takeScreenshot();
        if (screen == null) return false;
        try {
            // Screenshot and accessibility bounds share full-screen
            // coordinates. A rendered button spans at least two luminance
            // bands (fill, border, text); a blank page or a dimmed Loading
            // backdrop concentrates in one. Band colors are not asserted,
            // only that real content occupies the rect.
            Rect inside = new Rect(target);
            if (!inside.intersect(0, 0, screen.getWidth(), screen.getHeight())) return false;
            int[] bands = new int[8];
            int total = 0;
            for (int y = inside.top; y < inside.bottom; y += 2) {
                for (int x = inside.left; x < inside.right; x += 2) {
                    int pixel = screen.getPixel(x, y);
                    int luminance = (((pixel >> 16) & 0xFF) + ((pixel >> 8) & 0xFF) + (pixel & 0xFF)) / 3;
                    bands[luminance >> 5]++;
                    total++;
                }
            }
            int occupied = 0;
            for (int band = 0; band < bands.length; band++) {
                if (bands[band] * 100 >= total) occupied++;
            }
            return occupied >= 2;
        } finally { screen.recycle(); }
    }

    private void captureUi(String prefix) throws IOException {
        try (OutputStream out = evidence(prefix + "-hierarchy.txt")) {
            out.write(tree().getBytes(StandardCharsets.UTF_8));
        }
        Bitmap screen = instrumentation.getUiAutomation().takeScreenshot();
        if (screen != null) {
            try (OutputStream out = evidence(prefix + "-screen.png")) { screen.compress(Bitmap.CompressFormat.PNG, 100, out); }
            finally { screen.recycle(); }
        }
    }

    @After public void retireOnlyOwnedFilesAfterWriterCompletion() throws Exception {
        // Retain native UI evidence before either owned Activity/picker closes.
        try {
            // Encoding replaces a truncated surrogate; at most64k UTF-8 bytes.
            captureUi("photopea");
        } finally {
            if (scenario != null) scenario.close();
            if (activity != null) {
                assertTrue("Owned writer remains live; retain files", activity.writer.awaitTermination(5, TimeUnit.SECONDS));
                for (WebAppProbeActivity.Export transfer : activity.exports) {
                    assertEquals(0, transfer.finished.getCount());
                    assertTrue("Original response closure unproven; retain files", transfer.closed);
                    if (transfer.output != null) {
                        assertTrue(DocumentsContract.isDocumentUri(instrumentation.getTargetContext(), transfer.output));
                        assertTrue(DocumentsContract.deleteDocument(instrumentation.getTargetContext().getContentResolver(), transfer.output));
                    }
                }
            }
            if (input != null) assertEquals(1, instrumentation.getTargetContext().getContentResolver().delete(input, null, null));
        }
    }
}
