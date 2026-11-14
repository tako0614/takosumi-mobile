package com.takosumi.fixture.geckodownload;

import android.app.Instrumentation;
import android.content.Context;
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
import android.view.accessibility.AccessibilityWindowInfo;
import android.view.inputmethod.InputMethodManager;
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
import java.util.function.Supplier;
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

    // Text-match-only lookup with the interactive enabled/visible gates
    // bypassed: distinguishes a rendered-but-flagged node from one that is
    // genuinely absent from the a11y snapshot.
    private AccessibilityNodeInfo findAny(AccessibilityNodeInfo node, String text, int depth) {
        if (depth > 24) return null;
        CharSequence raw = node.getText();
        if (raw != null && raw.toString().contains(text)) {
            Rect b = new Rect(); node.getBoundsInScreen(b);
            if (!b.isEmpty()) return node;
        }
        for (int i = 0; i < node.getChildCount(); i++) {
            AccessibilityNodeInfo child = node.getChild(i);
            if (child == null) continue;
            AccessibilityNodeInfo found;
            try {
                found = findAny(child, text, depth + 1);
            } catch (RuntimeException | Error failure) {
                child.recycle();
                throw failure;
            }
            if (found == null) { child.recycle(); continue; }
            // A match returns the matching object itself; when that is the
            // child, recycling it here would hand the caller a dead node.
            if (found != child) child.recycle();
            return found;
        }
        return null;
    }

    // One a11y snapshot answering three hypotheses for a named node: which
    // package owns the active window (a dead picker's stale root blocks every
    // bounds read), whether the node exists at all, and if present whether
    // the interactive enabled/visibleToUser gates would see it.
    private String nodeState(String text) {
        AccessibilityNodeInfo root = instrumentation.getUiAutomation().getRootInActiveWindow();
        if (root == null) return "no-root";
        try {
            String pkg = String.valueOf(root.getPackageName());
            AccessibilityNodeInfo hit = findAny(root, text, 0);
            if (hit == null) return pkg + " absent";
            try {
                Rect b = new Rect(); hit.getBoundsInScreen(b);
                return pkg + " " + (hit.isEnabled() ? "enabled" : "disabled") +
                    "/" + (hit.isVisibleToUser() ? "visible" : "invisible") + " " + b.toShortString();
            } finally { if (hit != root) hit.recycle(); }
        } finally { root.recycle(); }
    }

    // Photopea's in-page dialog chrome ignores injected Escape keys: run
    // 42725834124 left the New Project overlay open for the whole bound after
    // KEYCODE_ESCAPE. A user closes it through the unnamed X at the right end
    // of its title row, so locate that row by the dialog title and return the
    // close corner. The X itself exposes no text, description or click flag.
    private Rect dialogClose() {
        AccessibilityNodeInfo root = instrumentation.getUiAutomation().getRootInActiveWindow();
        if (root == null) return null;
        try {
            // The dialog must exist in this same snapshot: after it unmounts
            // the landing " New Project" card still contains the title text
            // and would yield a close rect computed from unrelated content.
            if (find(root, "Create", false, false, true, 0) == null) return null;
            Rect win = new Rect(); root.getBoundsInScreen(win);
            AccessibilityNodeInfo title = findAny(root, "New Project", 0);
            if (title == null) return null;
            try {
                Rect t = new Rect(); title.getBoundsInScreen(t);
                return new Rect(win.right - 32, t.centerY() - 12,
                    win.right - 4, t.centerY() + 12);
            } finally { if (title != root) title.recycle(); }
        } finally { root.recycle(); }
    }

    // Flag-agnostic bounds for a node whose a11y visibility flag is wrong.
    // Run 42729892985 proved the opened document tab is painted on screen
    // while isVisibleToUser stays false for the whole bound, so a flag-gated
    // bounds() misses it and a flag-gated assertNull passes on a real leak.
    // Presence alone is never the oracle: callers pair this with painted()
    // for "really on screen" or assertNull for "truly absent".
    private Rect anyBounds(String text) {
        AccessibilityNodeInfo root = instrumentation.getUiAutomation().getRootInActiveWindow();
        if (root == null) return null;
        try {
            AccessibilityNodeInfo hit = findAny(root, text, 0);
            if (hit == null) return null;
            try {
                Rect b = new Rect(); hit.getBoundsInScreen(b);
                return b.isEmpty() ? null : b;
            } finally { if (hit != root) hit.recycle(); }
        } finally { root.recycle(); }
    }

    // The actual open oracle: the document tab node exists in the tree and
    // its region holds real painted content — strictly the evidence the
    // broken visibility flag was a proxy for, never the flag itself.
    private boolean tabOpened(String name) {
        Rect tab = anyBounds(name);
        return tab != null && painted(tab);
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
        long downDelivered = -1;
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
                        // A settled action that still moves by dispatch lands
                        // on the wrong card; an abort leaves honest evidence
                        // and the outer catch cancels a half-sent gesture.
                        if (currentAction != null && !currentAction.contains(bounds))
                            throw new AssertionError("open action moved after settle: sent="
                                + bounds.toShortString() + " current=" + currentAction.toShortString());
                    } catch (RuntimeException observation) {
                        activity.trace("open touch observation unavailable " + observation.getClass().getSimpleName());
                    }
                }
                // A real finger reports TOOL_TYPE_FINGER; obtain()'s plain overload
                // leaves TOOL_TYPE_UNKNOWN. Selection-driven lists gate their item
                // activation on finger/stylus tool types, so an unknown-tool tap
                // on a DocumentsUI row can be ignored while ordinary buttons
                // still click. Keep the synthesized touch faithful instead of
                // routing around the picker's own input handling.
                MotionEvent event = touch(down, action, bounds);
                try {
                    // sendPointerSync is restricted to the instrumented app UID.
                    // The real OS picker belongs to its own UID; UiAutomation is
                    // Android's cross-app test input route. No permission adoption,
                    // shell input or direct file-prompt callback substitutes for it.
                    if (systemPicker) assertTrue("DocumentsUI rejected native touch",
                        instrumentation.getUiAutomation().injectInputEvent(event, true));
                    else sendOwnUid(event);
                    gestureStarted = action == MotionEvent.ACTION_DOWN;
                    if (action == MotionEvent.ACTION_DOWN) downDelivered = SystemClock.uptimeMillis();
                    else if (activity != null)
                        activity.trace("tap delivery gap=" + (SystemClock.uptimeMillis() - downDelivered) +
                            "ms " + bounds.toShortString());
                } finally { event.recycle(); }
            }
        } catch (RuntimeException | Error failure) {
            if (gestureStarted) {
                // A changed target or rejected UP must not leave our accepted
                // DOWN held. Attempt CANCEL through the same input route, not
                // an UP on a replacement window. Preserve the original error.
                MotionEvent cancel = touch(down, MotionEvent.ACTION_CANCEL, bounds);
                try {
                    if (systemPicker) assertTrue("Owned picker touch stream could not be cancelled",
                        instrumentation.getUiAutomation().injectInputEvent(cancel, true));
                    else sendOwnUid(cancel);
                } catch (RuntimeException | Error cancellation) { failure.addSuppressed(cancellation); }
                finally { cancel.recycle(); }
            }
            throw failure;
        }
    }

    // sendPointerSync resolves the target window at injection time: a
    // transient system surface (sheet, prompt, shade) on top of our window
    // makes the OS refuse the event as "not directed at a window owned by
    // uid". That is an input race, not a missed tap — bounded retries on that
    // specific refusal only; every other failure keeps the original error.
    private void sendOwnUid(MotionEvent event) {
        for (int attempt = 1; ; attempt++) {
            try {
                instrumentation.sendPointerSync(event);
                return;
            } catch (Exception refused) {
                String detail = String.valueOf(refused.getMessage()) + " " +
                    (refused.getCause() == null ? "" : String.valueOf(refused.getCause().getMessage()));
                if (attempt >= 4 || !detail.contains("not directed at a window owned by uid")) {
                    String covering = coveringWindows(event);
                    if (activity != null) activity.trace("own-uid injection refused; covering windows: "
                        + covering);
                    // A plain rethrow loses the covering-window evidence:
                    // this error bypasses fail() and its diagnostics deque.
                    throw new IllegalStateException("own-uid input refused ("
                        + detail.trim() + "); covering windows: " + covering, refused);
                }
                if (activity != null)
                    activity.trace("own-uid injection refused by a transient foreign window; retry " + attempt);
                SystemClock.sleep(200);
            }
        }
    }

    // Interactive windows from other uids containing the point: a
    // self-targeted event is refused while one sits on top. Naming the
    // surface and its owner lets the next refusal classify itself instead of
    // guessing at a transient. A foreign TYPE_APPLICATION window counts too:
    // another app's dialog (2 = input method, system bars, sheets, or a
    // system permission prompt) intercepts the same tap while reporting a
    // type this check once skipped — an empty list then explained nothing.
    private String coveringWindows(MotionEvent event) {
        String own = instrumentation.getTargetContext().getPackageName();
        StringBuilder names = new StringBuilder();
        try {
            for (AccessibilityWindowInfo window : instrumentation.getUiAutomation().getWindows()) {
                Rect area = new Rect();
                window.getBoundsInScreen(area);
                if (!area.contains((int) event.getX(), (int) event.getY())) continue;
                String owner = null;
                AccessibilityNodeInfo root = window.getRoot();
                if (root != null) {
                    try { owner = String.valueOf(root.getPackageName()); }
                    finally { root.recycle(); }
                }
                if (own.equals(owner)) continue;
                names.append(names.length() == 0 ? "" : ", ")
                    .append("type=").append(window.getType())
                    .append('@').append(area.toShortString())
                    .append(' ').append(owner);
            }
        } catch (RuntimeException unavailable) { /* evidence only */ }
        // The IME can intercept the tap while missing from getWindows()
        // entirely (run 42953633716: empty list, keyboard visibly open in
        // the failure capture) — a text field accepting input names it by
        // elimination.
        return names + "; acceptingText=" + acceptingText();
    }

    // Whether a text field currently accepts input — the on-screen
    // keyboard is up or about to be, even when its window is absent from
    // the accessibility window list.
    private boolean acceptingText() {
        if (activity == null) return false;
        final boolean[] accepting = new boolean[1];
        instrumentation.runOnMainSync(() -> {
            InputMethodManager imm =
                (InputMethodManager) activity.getSystemService(Context.INPUT_METHOD_SERVICE);
            accepting[0] = imm != null && imm.isAcceptingText();
        });
        return accepting[0];
    }

    // The soft keyboard is a separate-uid window: while it covers a target,
    // self-targeted events are refused for as long as it stays open — no
    // retry bound helps. A real user closes it to reach the dialog; hide it
    // only when an input field is actually accepting text, then let the
    // caller's settled-bounds await measure the reflowed target.
    private void dismissCoveringIme(String what) {
        if (activity == null) return;
        final boolean[] dismissed = new boolean[1];
        instrumentation.runOnMainSync(() -> {
            InputMethodManager imm =
                (InputMethodManager) activity.getSystemService(Context.INPUT_METHOD_SERVICE);
            if (imm != null && imm.isAcceptingText() && activity.getWindow() != null) {
                imm.hideSoftInputFromWindow(activity.getWindow().getDecorView().getWindowToken(), 0);
                dismissed[0] = true;
            }
        });
        if (dismissed[0]) activity.trace("dismissed the soft input covering " + what);
    }

    private MotionEvent touch(long down, int action, Rect bounds) {
        MotionEvent.PointerProperties[] properties = new MotionEvent.PointerProperties[1];
        properties[0] = new MotionEvent.PointerProperties();
        properties[0].id = 0;
        properties[0].toolType = MotionEvent.TOOL_TYPE_FINGER;
        MotionEvent.PointerCoords[] coords = new MotionEvent.PointerCoords[1];
        coords[0] = new MotionEvent.PointerCoords();
        coords[0].x = bounds.exactCenterX();
        coords[0].y = bounds.exactCenterY();
        coords[0].pressure = 1;
        coords[0].size = 1;
        return MotionEvent.obtain(down, SystemClock.uptimeMillis(), action,
            1, properties, coords, 0, 0, 1, 1, 0, 0, InputDevice.SOURCE_TOUCHSCREEN, 0);
    }

    private void tap(String text) {
        await("Missing native text " + text, () -> bounds(text, false, false) != null, 30);
        // Every text tap faces the same re-layout race the open action hit:
        // a still-moving rect is refuse-to-tap, not best-effort.
        tap(awaitRect("Native text " + text + " did not settle",
            () -> stable("text " + text, bounds(text, false, false)), 15));
    }

    private void enterEditor(String profile) {
        enterEditor(profile, null);
    }

    private PageBaseline.Snapshot enterEditor(String profile, WebAppProbeActivity.Page expected) {
        boolean[] entryTapped = {false};
        boolean[] entryScrolled = {false};
        PageBaseline.Snapshot[] baseline = {null};
        long deadline = SystemClock.elapsedRealtime() + 60000;
        await("Actual anonymous Photopea File menu did not load for " + profile, () -> {
            if (bounds("File", false, false) != null) {
                if (expected == null) return true;
                baseline[0] = initialBaseline(expected, deadline);
                return baseline[0] != null;
            }
            if (!entryTapped[0]) {
                Rect viewport = visibleWebViewport(deadline);
                Rect start = bounds("Start using Photopea", false, false);
                // The landing CTA re-lays out like the other panels; defer the
                // tap until its rect survives half a second unchanged.
                if (start != null && viewport.contains(start) && stable("cta", start) != null) {
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
        return baseline[0];
    }

    private PageBaseline.Snapshot initialBaseline(WebAppProbeActivity.Page expected, long deadline) {
        Handler main = new Handler(Looper.getMainLooper());
        FutureTask<PageBaseline.Snapshot> read = new FutureTask<>(() -> expected.baseline.capture(
            activity.selected == expected && activity.pages.get(expected.context) == expected ? expected.session : null,
            activity.view.getSession(), expected.generation, expected.uri, true));
        try {
            long remaining = deadline - SystemClock.elapsedRealtime();
            assertTrue("Photopea entry deadline expired", remaining > 0);
            assertTrue("Main thread rejected the initial page baseline", main.post(read));
            return read.get(Math.min(1000, remaining), TimeUnit.MILLISECONDS);
        } catch (Exception error) {
            if (error instanceof InterruptedException) Thread.currentThread().interrupt();
            throw new AssertionError("Initial page baseline unavailable before the entry deadline", error);
        } finally {
            main.removeCallbacks(read);
            read.cancel(false);
        }
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


    // The profile buttons run select() on the main thread, and the released
    // surface's a11y tree can outlive the swap, so neither the delivered tap
    // nor a node match proves the displayed session changed. Read which
    // session the GeckoView actually attached.
    private boolean displayedIs(WebAppProbeActivity.Page expected) {
        Handler main = new Handler(Looper.getMainLooper());
        FutureTask<Boolean> read = new FutureTask<>(() -> activity.view.getSession() == expected.session);
        try {
            assertTrue("Main thread rejected the session read", main.post(read));
            return read.get(10, TimeUnit.SECONDS);
        } catch (Exception error) {
            if (error instanceof InterruptedException) Thread.currentThread().interrupt();
            throw new AssertionError("Displayed session read unavailable", error);
        } finally {
            // A queued or late callback only reads identity. It must never
            // perform delayed native input after this test has failed.
            main.removeCallbacks(read);
            read.cancel(false);
        }
    }

    // The switch settles only when the expected page owns the view and the
    // imported document node has left the active window's tree. A real leak
    // keeps this false: the tab would still match inside the freshly
    // attached session, so the wait times out and the capture shows it.
    private boolean switchedWithoutTab(String context) {
        WebAppProbeActivity.Page page = activity.pages.get(context);
        return page != null && displayedIs(page) && anyBounds(tabName) == null;
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
        if (bounds("Show roots", false, true) != null) {
            tap(awaitRect("Show roots control did not settle",
                () -> stable("roots", bounds("Show roots", false, true)), 10));
            tap(awaitRect("Missing settled Downloads root",
                () -> picker() ? stable("downloads", bounds("Downloads", false, false)) : null, 15));
        } else {
            // A missing drawer is not an alternate fake picker. Require the
            // actual provider to already show its Downloads title.
            assertNotNull("No Downloads destination in real picker", bounds("Downloads", false, false));
        }
    }

    private final Map<String, Rect> lastBounds = new HashMap<>();
    private final Map<String, Long> lastChanged = new HashMap<>();

    // A node whose screen rect is still moving between polls can eat an
    // injected tap during the same transition: the drawer slide-in moved a
    // captured row onto its scrim, and a settling grid consumed its own
    // List-view toggle. Only a rect that survives unchanged for half a
    // second may be tapped.
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

    private Rect awaitRect(String message, Supplier<Rect> read, int seconds) {
        Rect[] found = {null};
        await(message, () -> (found[0] = read.get()) != null, seconds);
        return found[0];
    }

    // The owned file as a selectable item: in list view its filename is a
    // text node; in the image grid only the item card's compound description
    // carries the name. Either is the real item; the preview hotspot node is
    // a different description and stays untouched.
    private Rect itemBounds() {
        Rect text = bounds(inputName, false, false);
        if (text != null) return stable("file-text", text);
        return stable("file-card", bounds(inputName, true, true));
    }

    // Tap inside the item's own activation region, shifted left of any
    // right-edge preview hotspot and right of the leading icon/select area.
    private Rect itemTap(Rect item) {
        int x = item.left + (int) (item.width() * 0.3f);
        int y = item.centerY();
        return new Rect(x, y, x + 2, y + 2);
    }

    // Returns the elapsedRealtime at which the OS answer was observed, so the
    // post-open await can measure tab appearance from the file confirm itself.
    private long chooseInput() {
        openDownloads();
        boolean[] listViewRequested = {false};
        await("OS picker did not show exact owned image", () -> {
            if (!picker()) return false;
            if (itemBounds() != null) return true;
            // DocumentsUI's image grid exposes the filename only in a compound
            // description. Its thumbnail center can hit the Preview overlay.
            // Use the real layout control once, only after it settles; the
            // grid card itself already exposes the owned file as an item.
            if (!listViewRequested[0]) {
                Rect listView = stable("listview", bounds("List view", false, true));
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
        long[] confirmedAt = {0};
        await("Original file prompt did not receive the OS answer", () -> {
            if (activity.fileSelections == 1) { confirmedAt[0] = SystemClock.elapsedRealtime(); return true; }
            if (!picker()) return false;
            Rect file = itemBounds();
            if (file == null) return false;
            long now = SystemClock.elapsedRealtime();
            if (inputTaps[0] < 3 && now - lastInputTap[0] >= 2000) {
                inputTaps[0]++;
                lastInputTap[0] = now;
                activity.trace("input tap attempt=" + inputTaps[0] + " " + file.toShortString());
                tap(itemTap(file));
            }
            return false;
        }, 30);
        assertEquals(activity.work, activity.inputContext);
        return confirmedAt[0];
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
        // The home panel re-lays out between an a11y bounds read and touch
        // delivery: a captured open-action rect once landed on the New
        // Project card instead. Only tap a rect that survived unchanged for
        // half a second, the same gate the picker rows already use.
        tap(awaitRect("Open action did not settle",
            () -> stable("open", bounds(openAction, false, false)), 15), openAction);
        // A settled rect is still not proof of a hit: the home panel's reveal
        // once left the reported Open bounds ~50px above the painted cards
        // (sent == current through both touch phases, filePrompts stayed 0
        // and the failure tree showed the New Project dialog). That is a
        // human-shaped miss, not a pass: when the tap produced no prompt
        // and the recognizable New Project dialog (its Create button)
        // appeared, dismiss it like a user — through its title-row X, never
        // BACK into page history — re-read the now-settled action and tap
        // once more. Any other silent miss keeps the original failure
        // evidence; the real OS-picker oracle is unchanged. Injected Escape
        // does not reach this dialog chrome, so dismissal is the X tap.
        boolean[] missDismissed = {false};
        try {
            await("Open tap produced no real file prompt", () -> {
                if (activity.filePrompts > 0) return true;
                if (bounds("Create", false, false) != null) {
                    missDismissed[0] = true;
                    activity.trace("open tap opened the New Project dialog; dismissing once and retrying");
                    return true;
                }
                return false;
            }, 12);
        } catch (AssertionError noPrompt) {
            // A prompt that lands just after the bound is still the prompt;
            // only a real zero keeps the failure alive.
            if (activity.filePrompts == 0) throw noPrompt;
        }
        if (missDismissed[0] && activity.filePrompts == 0) {
            int[] closeTaps = {0};
            long[] lastCloseTap = {0};
            await("Missed-tap New Project dialog did not dismiss", () -> {
                if (activity.filePrompts > 0 || bounds("Create", false, false) == null) return true;
                long now = SystemClock.elapsedRealtime();
                if (closeTaps[0] < 3 && now - lastCloseTap[0] >= 2000) {
                    Rect close = dialogClose();
                    if (close != null) {
                        closeTaps[0]++;
                        lastCloseTap[0] = now;
                        activity.trace("dialog close tap " + close.toShortString());
                        tap(close);
                    }
                }
                return false;
            }, 10);
            if (activity.filePrompts == 0) {
                tap(awaitRect("Retried open action did not settle",
                    () -> stable("open-retry", bounds(openAction, false, false)), 15), openAction);
            }
        }
        long fileConfirmedAt = chooseInput();
        String[] lastNodeState = {null};
        await("The actual editor did not open the selected image", () -> {
            // Classify the tab node each poll, tracing only transitions: a
            // persistent dead-picker package invalidates every bounds read,
            // an invisible/disabled node is a flag miss, and absence until
            // the deadline is honest lateness — kept distinguishable without
            // weakening the visible+enabled oracle. Times are elapsed since
            // the OS file confirm, matching the parent's discrimination ask.
            String state = nodeState(tabName);
            if (!state.equals(lastNodeState[0])) {
                lastNodeState[0] = state;
                activity.trace("open-await t=+" + (SystemClock.elapsedRealtime() - fileConfirmedAt) + "ms " + state);
            }
            return !picker() && tabOpened(tabName);
        }, 30);
        WebAppProbeActivity.Page work = activity.pages.get(activity.work);
        assertNotNull(work);
        int document = work.generation;
        assertTrue(document > 0);
        String uri = work.uri;
        assertTrue(WebAppProbeActivity.site(uri));

        tap("Personal");
        // The switch is asynchronous: Work's released surface still answers
        // File and still lists the imported tab until its tree is reaped, so
        // neither the tap nor enterEditor proves Personal is displayed.
        await("Personal editor did not settle after the native switch",
            () -> switchedWithoutTab(activity.personal), 30);
        WebAppProbeActivity.Page personal = activity.pages.get(activity.personal);
        assertNotNull(personal);
        assertNotSame(work.session, personal.session);
        // File can outlive Work's released surface. Admit the initial baseline
        // only after this exact attached Personal session has completed a
        // successful site navigation, within the original editor-entry wait.
        PageBaseline.Snapshot personalBaseline = enterEditor("Personal", personal);
        assertNull("The imported Work document leaked to Personal", anyBounds(tabName));
        int personalDocument = personalBaseline.generation;
        String personalUri = personalBaseline.uri;
        tap("Work");
        await("Work editor was lost on native reattach", () -> tabOpened(tabName), 15);
        assertEquals(document, work.generation);
        assertEquals(uri, work.uri);

        // Injected chords do not reliably reach Photopea's web layer on this
        // stack (the same class of miss as the earlier New Project Escape).
        // Drive the real menu instead: File -> Export As -> PNG, then the
        // dialog's own export action — whichever of Save/OK the site renders.
        tap("File");
        // The menu's real label is "Export as" (lowercase, no ellipsis) —
        // a contains "Export As" never matches the opened menu.
        tap(awaitRect("Photopea Export as entry did not open or settle",
            () -> stable("export-as", bounds("Export as", false, false)), 15));
        tap(awaitRect("Photopea PNG export item did not open or settle",
            () -> stable("export-png", bounds("PNG", false, false)), 15));
        await("Actual Photopea Save for web dialog did not open", () -> bounds("Save for web", false, false) != null, 20);
        // The dialog's number fields summon the soft keyboard — a
        // foreign-uid window over the lower half that swallowed this export
        // tap under both earlier signatures. A user closes it first; the
        // settled-bounds await below then samples the reflowed button, so
        // hiding now never lands a stale tap.
        dismissCoveringIme("Save for web");
        // A settled Save rect is still not proof of a hit: the dialog can
        // still be expanding (the preview paints late), so the delivered
        // position can predate the button's final row — the missed tap once
        // landed inside the Width/Height row instead. The real effect is
        // the dialog closing or the response arriving; a surviving dialog
        // means the tap missed and earns exactly one fresh-bounds retry —
        // the original-response oracle is unchanged.
        for (int saveAttempt = 0; saveAttempt < 2; saveAttempt++) {
            // The first tap may have landed while the close ran late —
            // never retap into a dialog that already left or a response
            // that already arrived; either means the export went out.
            if (saveAttempt > 0 && (activity.responses.peek() != null
                    || bounds("Save for web", false, false) == null)) break;
            Rect saveRect = awaitRect("Save for web export action did not settle", () -> {
                Rect save = bounds("Save", false, false);
                return stable("export-save", save != null ? save : bounds("OK", false, false));
            }, 15);
            // The focused number field re-opens the keyboard over the lower
            // half whenever the dialog settles into it — run 42953633716
            // saw the refusal outlive every retry with the keyboard on the
            // Save button. A user closes it again to reach the button, so
            // dismissal belongs inside the attempt: each reopen earns the
            // same close, and the bounds above were already sampled for
            // this layout.
            dismissCoveringIme("Save for web export action");
            tap(saveRect);
            try {
                await("Save for web dialog did not close after its export action",
                    () -> activity.responses.peek() != null
                        || bounds("Save for web", false, false) == null, 10);
                break;
            } catch (AssertionError stillOpen) {
                if (saveAttempt == 1) throw stillOpen;
                activity.trace("export action left the Save for web dialog open; retrying on fresh bounds");
            }
        }
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
        assertNotNull(anyBounds(tabName));
        // The export dialog keeps its filename field focused, so the soft
        // keyboard re-opens over the lower half (IME_INSETS_SHOW_ANIMATION was
        // still running at the observed refusal). Hide it before the top-bar
        // tap or the point resolves to the foreign window again.
        dismissCoveringIme("Personal tab");
        tap("Personal");
        await("Personal editor did not settle after the native switch",
            () -> switchedWithoutTab(activity.personal), 30);
        // Personal is a resident editor here, so enterEditor's File wait is
        // also the proof its tree is live: without it, the gate can pass on
        // the empty interregnum and assertNull would measure a vacuum.
        enterEditor("Personal");
        assertSame(personal, activity.pages.get(activity.personal));
        assertEquals(personalDocument, personal.generation);
        assertEquals(personalUri, personal.uri);
        assertNull(anyBounds(tabName));
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
            .append(" description=").append(bounded(node.getContentDescription()))
            .append(flags(node)).append('\n');
        if (out.length() > 16000) out.setLength(16000);
        for (int i = 0; i < node.getChildCount() && left[0] > 0 && out.length() < 16000; i++) {
            AccessibilityNodeInfo child = node.getChild(i);
            if (child != null) try { describe(child, out, depth + 1, left); } finally { child.recycle(); }
        }
    }

    private String flags(AccessibilityNodeInfo node) {
        StringBuilder out = new StringBuilder();
        if (node.isClickable()) out.append(",clickable");
        if (node.isLongClickable()) out.append(",long-clickable");
        if (node.isCheckable()) out.append(",checkable");
        if (node.isChecked()) out.append(",checked");
        if (node.isSelected()) out.append(",selected");
        if (node.isFocused()) out.append(",focused");
        if (!node.isEnabled()) out.append(",disabled");
        if (!node.isVisibleToUser()) out.append(",invisible");
        return out.length() == 0 ? "" : " flags=" + out.substring(1);
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
                        // Downloads' provider gates delete on the record's
                        // owner, not the read/write grant its save returned:
                        // a denied delete leaves a fresh-UUID file in the
                        // throwaway guest's Downloads, so record the leftover
                        // instead of failing what the flow already proved.
                        try {
                            if (!DocumentsContract.deleteDocument(instrumentation.getTargetContext().getContentResolver(), transfer.output)) {
                                activity.trace("OS export delete reported false; left " + transfer.output);
                            }
                        } catch (SecurityException denied) {
                            activity.trace("OS export delete denied; left " + transfer.output + " " + denied.getClass().getSimpleName());
                        }
                    }
                }
            }
            if (input != null) assertEquals(1, instrumentation.getTargetContext().getContentResolver().delete(input, null, null));
        }
    }
}
