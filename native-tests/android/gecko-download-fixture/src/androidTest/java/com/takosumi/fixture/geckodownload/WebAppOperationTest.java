package com.takosumi.fixture.geckodownload;

import android.app.Instrumentation;
import android.content.ContentValues;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Rect;
import android.net.Uri;
import android.os.Environment;
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
import java.io.*;
import java.nio.file.Files;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.concurrent.TimeUnit;
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

    private Rect find(AccessibilityNodeInfo node, String text, boolean contains, boolean description, int depth) {
        if (depth > 24) return null;
        CharSequence raw = description ? node.getContentDescription() : node.getText();
        if (node.isVisibleToUser() && node.isEnabled() && raw != null &&
                (contains ? raw.toString().contains(text) : raw.toString().equalsIgnoreCase(text))) {
            Rect bounds = new Rect(); node.getBoundsInScreen(bounds);
            if (!bounds.isEmpty()) return bounds;
        }
        for (int i = 0; i < node.getChildCount(); i++) {
            AccessibilityNodeInfo child = node.getChild(i);
            if (child == null) continue;
            try {
                Rect found = find(child, text, contains, description, depth + 1);
                if (found != null) return found;
            } finally { child.recycle(); }
        }
        return null;
    }

    private Rect bounds(String text, boolean contains, boolean description) {
        AccessibilityNodeInfo root = instrumentation.getUiAutomation().getRootInActiveWindow();
        if (root == null) return null;
        try { return find(root, text, contains, description, 0); } finally { root.recycle(); }
    }

    private void tap(Rect bounds) {
        assertNotNull(bounds);
        long down = SystemClock.uptimeMillis();
        for (int action : new int[] {MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP}) {
            MotionEvent event = MotionEvent.obtain(down, SystemClock.uptimeMillis(), action,
                bounds.exactCenterX(), bounds.exactCenterY(), 0);
            try { instrumentation.sendPointerSync(event); } finally { event.recycle(); }
        }
    }

    private void tap(String text) {
        await("Missing native text " + text, () -> bounds(text, false, false) != null, 30);
        tap(bounds(text, false, false));
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
        await("OS picker did not show exact owned image", () -> picker() && bounds(inputName, false, false) != null, 15);
        assertEquals(0, activity.fileSelections);
        tap(bounds(inputName, false, false));
        await("Original file prompt did not receive the OS answer", () -> activity.fileSelections == 1, 15);
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
        Files.write(new File(evidenceDirectory(), "export.png").toPath(), data);
    }

    @Test public void photopeaNativeOpenAndExportPreserveTheEditorAndOtherProfile() throws Exception {
        createInput();
        scenario = ActivityScenario.launch(WebAppProbeActivity.class);
        scenario.onActivity(value -> activity = value);
        await("Actual anonymous Photopea File menu did not load", () -> bounds("File", false, false) != null, 60);
        key(KeyEvent.KEYCODE_O, KeyEvent.META_CTRL_ON | KeyEvent.META_CTRL_LEFT_ON);
        chooseInput();
        await("The actual editor did not open the selected image", () -> !picker() && bounds(tabName, true, false) != null, 30);
        WebAppProbeActivity.Page work = activity.pages.get(activity.work);
        assertNotNull(work);
        int document = work.generation;
        assertTrue(document > 0);
        String uri = work.uri;
        assertTrue(WebAppProbeActivity.site(uri));

        tap("Personal");
        await("Independent Photopea profile did not load", () -> bounds("File", false, false) != null, 60);
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

    private File evidenceDirectory() {
        File evidence = new File(instrumentation.getTargetContext().getExternalFilesDir(null), "web-app-evidence");
        assertTrue(evidence.isDirectory() || evidence.mkdirs());
        return evidence;
    }

    @After public void retireOnlyOwnedFilesAfterWriterCompletion() throws Exception {
        // Retain native UI evidence before either owned Activity/picker closes.
        try {
            File evidence = evidenceDirectory();
            // Encoding replaces a truncated surrogate; at most64k UTF-8 bytes.
            Files.write(new File(evidence, "hierarchy.txt").toPath(), tree().getBytes(StandardCharsets.UTF_8));
            Bitmap screen = instrumentation.getUiAutomation().takeScreenshot();
            if (screen != null) {
                try (OutputStream out = new FileOutputStream(new File(evidence, "screen.png"))) { screen.compress(Bitmap.CompressFormat.PNG, 100, out); }
                finally { screen.recycle(); }
            }
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
