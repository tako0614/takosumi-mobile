package com.takosumi.fixture.webhome;

import static org.junit.Assert.*;

import android.content.Context;
import android.graphics.Rect;
import android.os.Bundle;
import android.os.SystemClock;
import android.provider.Settings;
import androidx.test.ext.junit.runners.AndroidJUnit4;
import androidx.test.platform.app.InstrumentationRegistry;
import androidx.test.uiautomator.By;
import androidx.test.uiautomator.BySelector;
import androidx.test.uiautomator.Direction;
import androidx.test.uiautomator.StaleObjectException;
import androidx.test.uiautomator.UiDevice;
import androidx.test.uiautomator.UiObject2;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.regex.Pattern;
import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;

/** External real-UI operations only: no WebView, JS injection or product callbacks. */
@RunWith(AndroidJUnit4.class)
public final class WebHomeOperationTest {
    private static final String TARGET = "com.takosumi.fixture.android";
    private static final String DRIVER = "com.takosumi.fixture.webhome";
    private UiDevice device;
    private Context context;
    private String origin, editor, launcher;
    private final JSONObject result = new JSONObject();
    private final JSONArray cases = new JSONArray();
    private int connectionAttempts;
    private BackWaitObservation backHomeObservation;
    private boolean backObservationIncomplete;
    // INITIAL_SAVE_OBSERVATION_FIELDS_BEGIN
    private InitialSaveObservation initialSaveObservation;
    private boolean initialSaveObservationIncomplete;
    // INITIAL_SAVE_OBSERVATION_FIELDS_END

    @Before public void setup() throws Exception {
        context = InstrumentationRegistry.getInstrumentation().getTargetContext();
        assertEquals("The driver must not replace the product APK", DRIVER, context.getPackageName());
        Bundle args = InstrumentationRegistry.getArguments();
        origin = args.getString("instance");
        editor = args.getString("editor");
        launcher = args.getString("launcher");
        assertNotNull(origin);
        assertNotNull(editor);
        assertNotNull(launcher);
        assertTrue(origin.matches("https://127\\.0\\.0\\.1:[0-9]{1,5}"));
        assertTrue(editor.matches("https://localhost:[0-9]{1,5}"));
        assertTrue(launcher.matches(Pattern.quote(TARGET) + "/" + Pattern.quote(TARGET) + "\\.[A-Za-z0-9_.$]+"));
        result.put("passed", false).put("cases", cases);
        device = UiDevice.getInstance(InstrumentationRegistry.getInstrumentation());
        launch();
        // INITIAL_SAVE_OBSERVATION_WAIT_BEGIN
        beginInitialSaveObservation();
        try { one(By.text("保存して開く")); }
        finally { if (initialSaveObservation != null) initialSaveObservation.waiting = false; }
        // INITIAL_SAVE_OBSERVATION_WAIT_END
    }

    private UiObject2 one(BySelector selector) {
        return one(selector, null);
    }

    private UiObject2 one(BySelector selector, JSONObject trace) {
        return one(selector, trace, false);
    }

    private UiObject2 one(BySelector selector, JSONObject trace, boolean scrollConnectionForm) {
        BySelector inApp = selector.pkg(TARGET);
        long end = SystemClock.elapsedRealtime() + 15000;
        ConnectionSaveKeyboard keyboard = trace == null ? null : new ConnectionSaveKeyboard();
        ConnectionSaveKeyboard.Probe keyboardProbe = trace == null ? null : new ConnectionSaveKeyboard.Probe() {
            public boolean isShown() { return keyboardShown(trace); }
            public boolean dismiss() {
                diagnostic(trace, "backRequested", true);
                boolean returned = false;
                try {
                    returned = device.pressBack();
                    return returned;
                } finally {
                    diagnostic(trace, "backReturned", returned);
                    diagnostic(trace, "keyboardReturnedAtMs", SystemClock.elapsedRealtime());
                }
            }
        };
        boolean formScrollRequested = false;
        do {
            if (keyboard != null && !keyboard.ready(keyboardProbe)) {
                SystemClock.sleep(100);
                continue;
            }
            UiObject2 value = device.findObject(inApp);
            if (value != null && value.isEnabled()) {
                Rect bounds = value.getVisibleBounds();
                if (!bounds.isEmpty()) {
                    if (keyboard != null && SystemClock.elapsedRealtime() >= end) break;
                    // Finding Save may take long enough for the IME to cover
                    // it. Discard this object after a dismissal and reacquire.
                    if (keyboard != null && !keyboard.ready(keyboardProbe)) {
                        SystemClock.sleep(100);
                        continue;
                    }
                    if (keyboard != null && SystemClock.elapsedRealtime() >= end) break;
                    if (trace != null) {
                        diagnostic(trace, "saveReadyAtMs", SystemClock.elapsedRealtime());
                        diagnostic(trace, "saveBounds", new JSONArray().put(bounds.left).put(bounds.top)
                            .put(bounds.right).put(bounds.bottom));
                    }
                    observeHomeQuery(BackWaitObservation.Outcome.VISIBLE);
                    return value;
                } else {
                    observeHomeQuery(BackWaitObservation.Outcome.EMPTY_BOUNDS);
                }
            } else {
                observeHomeQuery(value == null ? BackWaitObservation.Outcome.QUERY_NULL
                    : BackWaitObservation.Outcome.DISABLED);
            }
            if (scrollConnectionForm && !formScrollRequested
                    && (value == null || value.getVisibleBounds().isEmpty())) {
                UiObject2 heading = device.findObject(By.text("接続先を追加").pkg(TARGET));
                List<UiObject2> views = device.findObjects(
                    By.clazz("android.webkit.WebView").pkg(TARGET).scrollable(true));
                if (heading != null && !heading.getVisibleBounds().isEmpty() && views.size() == 1
                        && views.get(0).isEnabled() && !views.get(0).getVisibleBounds().isEmpty()) {
                    formScrollRequested = true;
                    // One real gesture. The return value says whether more
                    // scrolling is possible, not whether Save is ready.
                    views.get(0).scroll(Direction.DOWN, 0.5f);
                }
            }
            SystemClock.sleep(100);
        } while (SystemClock.elapsedRealtime() < end);
        throw new AssertionError("Actual App control did not become ready: " + selector);
    }

    private void expect(String text) { one(By.text(text)); }
    private AccessibleName.Candidate nameSnapshot(UiObject2 value) {
        Rect bounds = value.getVisibleBounds();
        return new AccessibleName.Candidate(value.getApplicationPackage(), value.getText(),
            value.getContentDescription(), value.isEnabled(), value.isClickable(), !bounds.isEmpty(),
            AccessibleName.MISSING, bounds.left, bounds.top, bounds.right, bounds.bottom);
    }

    private void click(String text) {
        long end = SystemClock.elapsedRealtime() + 15000;
        do {
            UiObject2 ready = null;
            try {
                // One snapshot avoids counting a node twice when text and
                // description both expose the same exact action name.
                List<UiObject2> values = device.findObjects(By.pkg(TARGET).enabled(true));
                List<AccessibleName.Candidate> names = new ArrayList<>();
                for (UiObject2 value : values) names.add(nameSnapshot(value));
                // UiObject2 equality/hash use the underlying accessibility
                // node identity. Only an actual direct parent can own a label;
                // matching strings or overlapping rectangles are insufficient.
                Map<UiObject2, Integer> owners = new HashMap<>();
                for (int n = 0; n < names.size(); n++) {
                    if (names.get(n).clickable && AccessibleName.matches(names.get(n), TARGET, text)) {
                        owners.put(values.get(n), n);
                    }
                }
                for (int n = 0; n < names.size(); n++) {
                    if (!names.get(n).clickable && AccessibleName.matches(names.get(n), TARGET, text)) {
                        UiObject2 parent = values.get(n).getParent();
                        Integer owner = parent == null ? null : owners.get(parent);
                        if (owner != null) names.set(n, names.get(n).withParent(owner));
                    }
                }
                int selected = AccessibleName.unique(names, TARGET, text);
                assertTrue("Ambiguous actual App action: " + text, selected != AccessibleName.AMBIGUOUS);
                if (selected >= 0 && AccessibleName.matches(nameSnapshot(values.get(selected)), TARGET, text)) {
                    ready = values.get(selected);
                }
            } catch (StaleObjectException ignored) {
                // No gesture occurred. Reacquire within the same deadline.
            }
            if (ready != null && SystemClock.elapsedRealtime() < end) {
                ready.click(); // Never catch/retry a possibly delivered gesture.
                return;
            }
            SystemClock.sleep(100);
        } while (SystemClock.elapsedRealtime() < end);
        throw new AssertionError("Actual App action did not become ready: " + text);
    }
    private List<ConnectionLabel.Node> connectionSnapshot() {
        List<ConnectionLabel.Node> nodes = new ArrayList<>();
        // Include every observed package so a foreign or contradictory child
        // cannot disappear from the selected row's direct-child validation.
        for (UiObject2 value : device.findObjects(By.pkg(Pattern.compile(".*")))) {
            nodes.add(new ConnectionLabel.Node(value, value.getParent(), value.getClassName(), nameSnapshot(value)));
        }
        return nodes;
    }

    private void clearConnectionData(String name) {
        long end = SystemClock.elapsedRealtime() + 15000;
        do {
            UiObject2 ready = null;
            try {
                ConnectionLabel.Selection held = ConnectionLabel.selectClear(connectionSnapshot(), TARGET, name, origin);
                if (held != null) {
                    // Reacquire and revalidate the card, named remove action,
                    // clear action and actual direct-parent identities together.
                    ConnectionLabel.Selection fresh = ConnectionLabel.rebind(
                        held, connectionSnapshot(), TARGET, name, origin);
                    if (fresh != null) ready = (UiObject2) fresh.clear.identity;
                }
            } catch (StaleObjectException ignored) {
                // No gesture occurred. Reacquire within the same deadline.
            }
            if (ready != null && SystemClock.elapsedRealtime() < end) {
                ready.click(); // A possibly delivered gesture is never retried.
                return;
            }
            SystemClock.sleep(100);
        } while (SystemClock.elapsedRealtime() < end);
        throw new AssertionError("Actual App clear action did not remain profile-bound: " + name);
    }

    private void nativeButton(String description) { one(By.desc(description)).click(); }
    private void menu(String text) { nativeButton("メニュー"); click(text); }
    private void picker() { menu("接続先を切り替える"); expect("接続先を追加"); }

    private void hideKeyboardIfShown() {
        if (keyboardShown(null)) device.pressBack();
    }

    private boolean keyboardShown(JSONObject trace) {
        if (trace != null) diagnostic(trace, "keyboardStartedAtMs", SystemClock.elapsedRealtime());
        String ime = Settings.Secure.getString(context.getContentResolver(), Settings.Secure.DEFAULT_INPUT_METHOD);
        boolean queried = false, shown = false;
        if (ime != null && ime.contains("/")) {
            String owner = ime.substring(0, ime.indexOf('/'));
            queried = true;
            shown = device.hasObject(By.pkg(owner));
        }
        if (trace != null) {
            diagnostic(trace, "imeQueried", queried);
            diagnostic(trace, "imePresent", shown);
            diagnostic(trace, "keyboardReturnedAtMs", SystemClock.elapsedRealtime());
        }
        return shown;
    }

    private static void diagnostic(JSONObject target, String key, Object value) {
        // Closed scalar metadata only; loss must not replace the UI failure.
        try { target.put(key, value); } catch (org.json.JSONException ignored) { }
    }

    // Observation is separate from one()'s Save/IME trace control input.
    private void beginAccountBackObservation() {
        try {
            backHomeObservation = new BackWaitObservation();
            backHomeObservation.beginBack(SystemClock.elapsedRealtime());
        } catch (RuntimeException ignored) { backObservationIncomplete = true; }
    }

    private void finishAccountBackObservation(boolean returned) {
        try {
            if (backHomeObservation != null) backHomeObservation.endBack(SystemClock.elapsedRealtime(), returned);
        } catch (RuntimeException ignored) { backObservationIncomplete = true; }
    }

    private void beginHomeWaitObservation() {
        try {
            if (backHomeObservation != null) backHomeObservation.beginWait(SystemClock.elapsedRealtime());
        } catch (RuntimeException ignored) { backObservationIncomplete = true; }
    }

    private void finishHomeWaitObservation(boolean returned) {
        try {
            if (backHomeObservation != null) backHomeObservation.endWait(SystemClock.elapsedRealtime(), returned);
        } catch (RuntimeException ignored) { backObservationIncomplete = true; }
        finally { if (backHomeObservation != null) backHomeObservation.waiting = false; }
    }

    private void observeHomeQuery(BackWaitObservation.Outcome outcome) {
        observeInitialSaveQuery(outcome); // initial-save-observation:existing-outcome
        if (backHomeObservation == null || !backHomeObservation.waiting) return;
        try { backHomeObservation.sample(outcome, SystemClock.elapsedRealtime()); }
        catch (RuntimeException ignored) { backObservationIncomplete = true; }
    }

    private void captureTime(String key) {
        try { diagnostic(result, key, SystemClock.elapsedRealtime()); }
        catch (RuntimeException ignored) { backObservationIncomplete = true; }
    }

    // INITIAL_SAVE_OBSERVATION_METHODS_BEGIN
    private void beginInitialSaveObservation() {
        try {
            initialSaveObservation = new InitialSaveObservation();
            initialSaveObservation.waiting = true;
        } catch (RuntimeException ignored) { initialSaveObservationIncomplete = true; }
    }

    private void observeInitialSaveQuery(BackWaitObservation.Outcome outcome) {
        InitialSaveObservation value = initialSaveObservation;
        if (value == null || !value.waiting) return;
        if (!InitialSaveObservation.safely(() -> value.sample(outcome))) initialSaveObservationIncomplete = true;
    }

    private void saveInitialSaveObservation() {
        try {
            InitialSaveObservation value = initialSaveObservation;
            if (value == null) {
                diagnostic(result, "initialSaveObservationIncomplete", true);
                return;
            }
            JSONObject metadata = new JSONObject();
            if (value.writeTo((key, item) -> metadata.put(key, item), initialSaveObservationIncomplete)) {
                diagnostic(result, "initialSaveWait", metadata);
            } else {
                initialSaveObservationIncomplete = true;
                diagnostic(result, "initialSaveObservationIncomplete", true);
            }
        } catch (RuntimeException ignored) {
            initialSaveObservationIncomplete = true;
            diagnostic(result, "initialSaveObservationIncomplete", true);
        }
    }
    // INITIAL_SAVE_OBSERVATION_METHODS_END

    private void saveBackObservation() {
        try {
            BackWaitObservation value = backHomeObservation;
            if (value == null) {
                diagnostic(result, "backObservationIncomplete", backObservationIncomplete);
                return;
            }
            JSONObject metadata = new JSONObject();
            diagnostic(result, "accountBackHome", metadata);
            diagnostic(metadata, "diagnosticOnly", true);
            diagnostic(metadata, "phase", "account_back_home");
            diagnostic(metadata, "clockDomain", "android_elapsed_realtime");
            diagnostic(metadata, "clockUnit", "ms");
            diagnostic(metadata, "recordingIncomplete", backObservationIncomplete || value.incomplete);
            diagnostic(metadata, "sampleLimit", BackWaitObservation.LIMIT);
            diagnostic(metadata, "backCallStartedAtMs", value.backStartedAtMs);
            diagnostic(metadata, "backCallFinishedAtMs", value.backFinishedAtMs);
            diagnostic(metadata, "backCallReturned", value.backReturned);
            diagnostic(metadata, "waitStartedAtMs", value.waitStartedAtMs);
            diagnostic(metadata, "waitFinishedAtMs", value.waitFinishedAtMs);
            diagnostic(metadata, "waitReturned", value.waitReturned);
            diagnostic(metadata, "samples", value.samples);
            diagnostic(metadata, "queryNull", value.queryNull);
            diagnostic(metadata, "disabled", value.disabled);
            diagnostic(metadata, "emptyBounds", value.emptyBounds);
            diagnostic(metadata, "visible", value.visible);
            diagnostic(metadata, "firstOutcome", value.first == null ? "unknown" : value.first.name());
            diagnostic(metadata, "lastOutcome", value.last == null ? "unknown" : value.last.name());
            diagnostic(metadata, "firstAtMs", value.firstAtMs);
            diagnostic(metadata, "lastAtMs", value.lastAtMs);
            diagnostic(metadata, "capReached", value.capReached);
            diagnostic(metadata, "droppedCapped", value.dropped);
        } catch (RuntimeException ignored) { diagnostic(result, "backObservationIncomplete", true); }
    }

    private void add(String name) {
        JSONObject trace = new JSONObject();
        diagnostic(trace, "diagnosticOnly", true);
        diagnostic(trace, "attempt", ++connectionAttempts);
        diagnostic(trace, "backRequested", false);
        diagnostic(trace, "backReturned", false);
        // Only the latest add is retained; no input text, URL or selector.
        diagnostic(result, "connectionAdd", trace);
        one(By.text("保存して開く"), null, true);
        List<UiObject2> fields = device.findObjects(By.clazz("android.widget.EditText").pkg(TARGET));
        assertEquals("The real connection form must contain URL and name", 2, fields.size());
        fields.get(0).click(); fields.get(0).setText(origin);
        fields.get(1).click(); fields.get(1).setText(name);
        diagnostic(trace, "setTextReturnedAtMs", SystemClock.elapsedRealtime());
        UiObject2 save = one(By.text("保存して開く"), trace);
        boolean returned = false;
        diagnostic(trace, "clickStartedAtMs", SystemClock.elapsedRealtime());
        try {
            save.click();
            returned = true;
        } finally {
            diagnostic(trace, "clickReturned", returned);
            diagnostic(trace, "clickFinishedAtMs", SystemClock.elapsedRealtime());
        }
        expect("Fixture signed out");
        expect("Remote has no App IPC");
    }

    private void reopen(String name) {
        long end = SystemClock.elapsedRealtime() + 15000;
        do {
            List<UiObject2> matches = new ArrayList<>();
            for (UiObject2 row : device.findObjects(By.clazz("android.widget.Button").pkg(TARGET))) {
                if (row.isEnabled() && row.isClickable() && !row.getVisibleBounds().isEmpty()
                        && (ConnectionLabel.matches(row.getText(), name, origin)
                            || ConnectionLabel.matches(row.getContentDescription(), name, origin))) {
                    matches.add(row);
                }
            }
            if (matches.size() == 1) { matches.get(0).click(); return; }
            assertTrue("Ambiguous saved connection " + name, matches.size() <= 1);
            SystemClock.sleep(100);
        } while (SystemClock.elapsedRealtime() < end);
        throw new AssertionError("Saved App connection button did not become ready: " + name);
    }

    private void host(String address) {
        one(By.desc("表示中のサイト " + address.substring("https://".length())));
    }

    private void draft(String value) {
        long end = SystemClock.elapsedRealtime() + 15000;
        do {
            List<UiObject2> fields = device.findObjects(By.clazz("android.widget.EditText").pkg(TARGET));
            if (fields.size() == 1 && value.equals(fields.get(0).getText())) {
                expect("Remote has no App IPC"); return;
            }
            SystemClock.sleep(100);
        } while (SystemClock.elapsedRealtime() < end);
        throw new AssertionError("Wrong profile's actual editor draft: " + value);
    }

    private void writeDraft(String value) {
        draft("");
        UiObject2 field = one(By.clazz("android.widget.EditText"));
        field.click(); field.setText(value);
        hideKeyboardIfShown();
        draft(value);
    }

    private String document() {
        String text = one(By.text(Pattern.compile("Document [0-9a-fA-F-]{36}"))).getText();
        UUID.fromString(text.substring("Document ".length()));
        return text;
    }

    private String pid() throws Exception {
        String value = device.executeShellCommand("pidof " + TARGET).trim();
        assertTrue("Ambiguous App process", value.isEmpty() || value.matches("[1-9][0-9]*"));
        return value;
    }

    private void launch() throws Exception {
        String reply = device.executeShellCommand("am start -W -n " + launcher);
        assertTrue("Actual App launcher refused", reply.contains("Status: ok"));
    }

    private void closeAndOpenMain() throws Exception {
        nativeButton("閉じる");
        // Android removes this profile's Recents task. The ordinary App icon
        // may be used to return to the connection picker, independent of the
        // platform's choice of the next foreground Recents task.
        launch(); expect("接続先を追加");
    }

    @Test public void wholeAppConnectionsWebHomeAndResumeStayProfileBound() throws Exception {
        add("Work"); host(origin);
        click("Sign in Work"); expect("Fixture home Work"); expect("Instance data instance-Work");
        click("Account settings"); expect("Account settings Work");
        beginAccountBackObservation();
        boolean accountBackReturned = false;
        try {
            nativeButton("戻る");
            accountBackReturned = true;
        } finally { finishAccountBackObservation(accountBackReturned); }
        beginHomeWaitObservation();
        boolean accountHomeReturned = false;
        try {
            expect("Fixture home Work");
            accountHomeReturned = true;
        } finally { finishHomeWaitObservation(accountHomeReturned); }
        click("📝 Notes"); expect("Notes editor"); host(editor);
        writeDraft("work-draft");
        click("Page two"); expect("Notes page two");
        nativeButton("戻る"); expect("Notes editor"); draft("work-draft");
        menu("アプリホームへ"); expect("Fixture home Work");
        nativeButton("戻る"); draft("work-draft");
        cases.put("connection-web-login-home-back").put("cross-origin-no-ipc");
        String warm = document(), warmPid = pid();
        assertFalse(warmPid.isEmpty());
        picker(); reopen("Work"); draft("work-draft"); assertEquals(warm, document());
        device.pressHome(); launch(); expect("接続先を追加"); reopen("Work");
        draft("work-draft"); assertEquals(warmPid, pid()); assertEquals(warm, document());
        cases.put("warm-document-retained");
        picker(); add("Personal"); click("Sign in Personal"); expect("Fixture home Personal");
        click("📝 Notes"); writeDraft("personal-draft");
        picker(); reopen("Work"); draft("work-draft");
        menu("アプリホームへ"); expect("Fixture home Work");
        nativeButton("戻る"); draft("work-draft");
        cases.put("profile-auth-and-draft-isolated");
        String before = document(), oldPid = pid();
        picker();
        device.executeShellCommand("am force-stop " + TARGET);
        long end = SystemClock.elapsedRealtime() + 10000;
        while (!pid().isEmpty() && SystemClock.elapsedRealtime() < end) SystemClock.sleep(100);
        assertEquals("Original App must terminate", "", pid());
        launch(); expect("接続先を追加"); reopen("Work");
        expect("Notes editor"); draft("work-draft"); host(editor);
        String fresh = document(), freshPid = pid();
        assertFalse(freshPid.isEmpty()); assertNotEquals(oldPid, freshPid); assertNotEquals(before, fresh);
        result.put("oldPid", oldPid).put("newPid", freshPid).put("beforeColdDocument", before).put("afterColdDocument", fresh);
        menu("アプリホームへ"); expect("Fixture home Work");
        picker(); reopen("Personal"); draft("personal-draft");
        menu("アプリホームへ"); expect("Fixture home Personal");
        cases.put("cold-process-and-profiles-restored");
        closeAndOpenMain(); reopen("Work"); closeAndOpenMain();
        clearConnectionData("Work"); expect("Workの保存データを消去しますか？");
        click("保存データを消去");
        // Completion feedback is below the connection form. Reuse its one
        // owned-page scroll while still requiring the exact visible receipt.
        one(By.text("Workのサイトデータを消去しました。"), null, true);
        reopen("Work"); expect("Fixture signed out"); host(origin);
        click("Sign in Work"); expect("Fixture home Work"); click("📝 Notes"); draft("");
        closeAndOpenMain(); reopen("Personal");
        expect("Fixture home Personal"); expect("Instance data instance-Personal");
        click("📝 Notes"); draft("personal-draft"); closeAndOpenMain();
        cases.put("cleared-profile-reset").put("other-profile-preserved").put("native-views-closed");
        result.put("passed", true);
    }

    @After public void evidence() throws Exception {
        File files = context == null ? null : context.getFilesDir();
        if (files == null) return;
        boolean saved = false;
        try {
            if (device != null) {
                boolean screenshot;
                captureTime("screenshotStartedAtMs");
                try { screenshot = device.takeScreenshot(new File(files, "screen.png")); }
                finally { captureTime("screenshotFinishedAtMs"); }
                captureTime("hierarchyStartedAtMs");
                try { device.dumpWindowHierarchy(new File(files, "hierarchy.xml")); }
                finally { captureTime("hierarchyFinishedAtMs"); }
                saved = screenshot;
            }
        } catch (Exception error) {
            result.put("diagnosticError", error.getClass().getSimpleName());
        }
        saveBackObservation();
        saveInitialSaveObservation(); // initial-save-observation:result-delivery
        result.put("diagnosticsSaved", saved);
        try (FileOutputStream stream = new FileOutputStream(new File(files, "result.json"))) {
            stream.write(result.toString().getBytes(StandardCharsets.UTF_8));
        }
    }
}
